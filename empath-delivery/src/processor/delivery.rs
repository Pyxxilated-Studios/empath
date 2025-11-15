//! Message delivery logic and error handling

use std::sync::Arc;

use empath_common::{
    DeliveryStatus,
    context::{Context, DeliveryContext},
    internal,
    tracing::{error, info, warn},
};
use empath_ffi::modules::{self, Ev, Event};
use empath_spool::SpooledMessageId;

use crate::{
    dns::MailServer,
    error::{DeliveryError, PermanentError, SystemError},
    processor::DeliveryProcessor,
    queue::retry::calculate_next_retry_time,
    types::DeliveryInfo,
};

/// Prepare a message for delivery using SMTP client (but don't actually send it yet)
///
/// This method:
/// 1. Reads the message from the spool
/// 2. Performs DNS MX lookup for the recipient domain
/// 3. Connects to the MX server via SMTP
/// 4. Performs EHLO/HELO handshake
/// 5. Validates MAIL FROM and RCPT TO
/// 6. Does NOT send DATA (that's for actual delivery)
///
/// # Errors
/// Returns an error if the message cannot be read, DNS lookup fails, or SMTP connection fails
#[allow(
    clippy::too_many_lines,
    reason = "Persistence logic adds necessary lines"
)]
pub async fn prepare_message(
    processor: &DeliveryProcessor,
    message_id: &SpooledMessageId,
    spool: &Arc<dyn empath_spool::BackingStore>,
) -> Result<(), DeliveryError> {
    processor
        .queue
        .update_status(message_id, DeliveryStatus::InProgress);

    // Persist the InProgress status to spool
    if let Err(e) = persist_delivery_state(processor, message_id, spool).await {
        warn!(
            message_id = ?message_id,
            error = %e,
            "Failed to persist delivery state after status update to InProgress"
        );
        // Continue anyway - this is not critical for delivery
    }

    let mut context = spool
        .read(message_id)
        .await
        .map_err(|e| SystemError::SpoolRead(e.to_string()))?;
    let info = processor.queue.get(message_id).ok_or_else(|| {
        SystemError::MessageNotFound(format!("Message {message_id:?} not in queue"))
    })?;

    // Dispatch DeliveryAttempt event to modules
    {
        context.delivery = Some(DeliveryContext {
            message_id: message_id.to_string(),
            domain: info.recipient_domain.clone(),
            server: None, // Server not yet determined at this point
            error: None,
            attempts: Some(info.attempt_count()),
            status: info.status.clone(),
            attempt_history: info.attempts.clone(),
            queued_at: info.queued_at,
            next_retry_at: info.next_retry_at,
            current_server_index: info.current_server_index,
        });

        modules::dispatch(Event::Event(Ev::DeliveryAttempt), &mut context);
    }

    // Check for domain-specific MX override first (for testing/debugging)
    let mail_servers = if let Some(domain_config) = processor.domains.get(&info.recipient_domain)
        && let Some(mx_override) = domain_config.mx_override_address()
    {
        internal!(
            "Using MX override for {}: {}",
            info.recipient_domain,
            mx_override
        );

        // Parse host:port or use default port 25
        let (host, port) = if let Some((h, p)) = mx_override.split_once(':') {
            (h.to_string(), p.parse::<u16>().unwrap_or(25))
        } else {
            (mx_override.to_string(), 25)
        };

        Arc::new(vec![MailServer {
            host,
            port,
            priority: 0,
        }])
    } else {
        // Get the DNS resolver
        let Some(dns_resolver) = &processor.dns_resolver else {
            return Err(SystemError::NotInitialized(
                "DNS resolver not initialized. Call init() first.".to_string(),
            )
            .into());
        };

        // Perform real DNS MX lookup for the recipient domain
        // DNS errors are automatically converted to DeliveryError via From<DnsError>
        let resolved = dns_resolver
            .resolve_mail_servers(&info.recipient_domain)
            .await?;

        if resolved.is_empty() {
            return Err(PermanentError::NoMailServers(info.recipient_domain.to_string()).into());
        }

        resolved
    };

    // Store the resolved mail servers
    processor
        .queue
        .set_mail_servers(message_id, mail_servers.clone());

    // Use the first (highest priority) mail server
    let primary_server = &mail_servers[0];
    let mx_address = primary_server.address();

    internal!(
        "Sending message to {:?} with MX host {} (priority {})",
        message_id,
        mx_address,
        primary_server.priority
    );

    // Dispatch DeliveryAttempt event before attempting delivery
    context.delivery = Some(DeliveryContext {
        message_id: message_id.to_string(),
        domain: info.recipient_domain.clone(),
        server: Some(mx_address.clone()),
        error: None,
        attempts: Some(info.attempt_count()),
        status: info.status.clone(),
        attempt_history: info.attempts.clone(),
        queued_at: info.queued_at,
        next_retry_at: info.next_retry_at,
        current_server_index: info.current_server_index,
    });
    modules::dispatch(Event::Event(Ev::DeliveryAttempt), &mut context);

    // Deliver the message via SMTP (including DATA command)
    let result = deliver_message(processor, &mx_address, &context, &info).await;

    match result {
        Ok(()) => {
            processor
                .queue
                .update_status(message_id, DeliveryStatus::Completed);

            // Persist the Completed status to spool before deletion
            // Note: This will be immediately deleted, but it's important for consistency
            // in case the deletion fails
            if let Err(e) = persist_delivery_state(processor, message_id, spool).await {
                warn!(
                    message_id = ?message_id,
                    error = %e,
                    "Failed to persist delivery state after successful delivery"
                );
            }

            // Delete the message from the spool after successful delivery
            if let Err(e) = spool.delete(message_id).await {
                error!(
                    message_id = ?message_id,
                    error = %e,
                    "Failed to delete message from spool after successful delivery"
                );
                // Don't fail the delivery just because we couldn't delete the spool file
                // The message was delivered successfully
            }

            // Dispatch DeliverySuccess event to modules
            context.delivery = Some(DeliveryContext {
                message_id: message_id.to_string(),
                domain: info.recipient_domain.clone(),
                server: Some(mx_address.clone()),
                error: None,
                attempts: Some(info.attempt_count()),
                status: info.status.clone(),
                attempt_history: info.attempts.clone(),
                queued_at: info.queued_at,
                next_retry_at: info.next_retry_at,
                current_server_index: info.current_server_index,
            });
            modules::dispatch(Event::Event(Ev::DeliverySuccess), &mut context);

            Ok(())
        }
        Err(e) => {
            let error =
                handle_delivery_error(processor, message_id, &mut context, e, mx_address).await;
            Err(error)
        }
    }
}

/// Handle a failed delivery attempt and update status based on retry policy
///
/// Records the attempt and determines whether to retry or mark as permanently failed.
/// Implements MX server fallback: tries lower-priority MX servers before counting as a retry.
/// Dispatches `DeliveryFailure` event to modules.
///
/// # Errors
/// Returns the original error after recording it
#[allow(
    clippy::too_many_lines,
    reason = "Persistence logic adds necessary lines"
)]
pub async fn handle_delivery_error(
    processor: &DeliveryProcessor,
    message_id: &SpooledMessageId,
    context: &mut Context,
    error: DeliveryError,
    server: String,
) -> DeliveryError {
    // Record the attempt
    let attempt = empath_common::DeliveryAttempt {
        timestamp: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        error: Some(error.to_string()),
        server: server.clone(),
    };

    processor.queue.record_attempt(message_id, attempt);

    // Get updated info to check attempt count
    // Use proper error handling instead of unwrap
    let Some(updated_info) = processor.queue.get(message_id) else {
        warn!(
            "Message {:?} disappeared from queue during error handling",
            message_id
        );
        return error; // Preserve original error
    };

    // Check if this is a temporary failure that warrants trying another MX server
    // (e.g., connection refused, timeout, temporary SMTP error)
    let is_temporary_failure = error.is_temporary();

    // Try next MX server if this was a temporary failure
    if is_temporary_failure
        && processor.queue.try_next_server(message_id)
        && let Some(info) = processor.queue.get(message_id)
        && let Some(next_server) = info.current_mail_server()
    {
        info!(
            "Trying next MX server for {:?}: {} (priority {})",
            message_id, next_server.host, next_server.priority
        );
        // Set status back to Pending to retry immediately with next server
        processor
            .queue
            .update_status(message_id, DeliveryStatus::Pending);

        // Persist the Pending status for next MX server attempt
        if let Some(spool) = &processor.spool
            && let Err(e) = persist_delivery_state(processor, message_id, spool).await
        {
            warn!(
                message_id = ?message_id,
                error = %e,
                "Failed to persist delivery state after MX server fallback"
            );
        }

        return error;
    }

    // All MX servers exhausted or permanent failure, use normal retry logic
    // Determine new status based on attempt count
    let new_status = if updated_info.attempt_count() >= processor.max_attempts {
        DeliveryStatus::Failed(error.to_string())
    } else {
        DeliveryStatus::Retry {
            attempts: updated_info.attempt_count(),
            last_error: error.to_string(),
        }
    };

    processor
        .queue
        .update_status(message_id, new_status.clone());

    // Calculate and set next retry time using exponential backoff
    if matches!(new_status, DeliveryStatus::Retry { .. }) {
        let next_retry_at = calculate_next_retry_time(
            updated_info.attempt_count(),
            processor.base_retry_delay_secs,
            processor.max_retry_delay_secs,
            processor.retry_jitter_factor,
        );

        processor
            .queue
            .set_next_retry_at(message_id, next_retry_at);

        // Calculate delay for logging
        let current_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let delay_secs = next_retry_at.saturating_sub(current_time);

        info!(
            message_id = ?message_id,
            attempt = updated_info.attempt_count(),
            retry_delay_secs = delay_secs,
            next_retry_at = next_retry_at,
            "Scheduled retry with exponential backoff"
        );
    }

    // Persist the updated status (Retry or Failed) to spool
    if let Some(spool) = &processor.spool
        && let Err(e) = persist_delivery_state(processor, message_id, spool).await
    {
        warn!(
            message_id = ?message_id,
            error = %e,
            "Failed to persist delivery state after handling delivery error"
        );
    }

    context.delivery = Some(DeliveryContext {
        message_id: message_id.to_string(),
        domain: updated_info.recipient_domain.clone(),
        server: Some(server),
        error: Some(error.to_string()),
        attempts: Some(updated_info.attempt_count()),
        status: updated_info.status.clone(),
        attempt_history: updated_info.attempts.clone(),
        queued_at: updated_info.queued_at,
        next_retry_at: updated_info.next_retry_at,
        current_server_index: updated_info.current_server_index,
    });

    modules::dispatch(Event::Event(Ev::DeliveryFailure), context);

    error
}

/// Persist the current delivery queue state to the spool's Context.delivery field
///
/// This method synchronizes the in-memory queue state (status, attempts, retry timing)
/// to the spool's persistent storage. This ensures queue state survives restarts.
///
/// # Errors
/// Returns an error if the message is not in the queue or if spool update fails
pub async fn persist_delivery_state(
    processor: &DeliveryProcessor,
    message_id: &SpooledMessageId,
    spool: &Arc<dyn empath_spool::BackingStore>,
) -> Result<(), DeliveryError> {
    // Get current queue info
    let info = processor.queue.get(message_id).ok_or_else(|| {
        SystemError::MessageNotFound(format!("Message {message_id:?} not in queue"))
    })?;

    // Read context from spool
    let mut context = spool
        .read(message_id)
        .await
        .map_err(|e| SystemError::SpoolRead(e.to_string()))?;

    // Update the delivery field with current queue state
    context.delivery = Some(DeliveryContext {
        message_id: message_id.to_string(),
        domain: info.recipient_domain.clone(),
        server: info.current_mail_server().map(MailServer::address),
        error: match &info.status {
            DeliveryStatus::Failed(e) | DeliveryStatus::Retry { last_error: e, .. } => {
                Some(e.clone())
            }
            _ => None,
        },
        attempts: Some(info.attempt_count()),
        status: info.status.clone(),
        attempt_history: info.attempts.clone(),
        queued_at: info.queued_at,
        next_retry_at: info.next_retry_at,
        current_server_index: info.current_server_index,
    });

    // Atomically update spool
    spool
        .update(message_id, &context)
        .await
        .map_err(|e| SystemError::SpoolWrite(e.to_string()))?;

    Ok(())
}

/// Deliver a message via SMTP (complete transaction including DATA)
///
/// This method performs the full SMTP transaction by delegating to `SmtpTransaction`.
///
/// # Errors
/// Returns an error if any part of the SMTP transaction fails
async fn deliver_message(
    processor: &DeliveryProcessor,
    server_address: &str,
    context: &Context,
    delivery_info: &DeliveryInfo,
) -> Result<(), DeliveryError> {
    // Check if TLS is required for this domain
    let require_tls = processor
        .domains
        .get(&delivery_info.recipient_domain)
        .is_some_and(|config| config.require_tls);

    // Determine if we should accept invalid certificates
    // Priority: per-domain override > global configuration
    let accept_invalid_certs = processor
        .domains
        .get(&delivery_info.recipient_domain)
        .and_then(|config| config.accept_invalid_certs)
        .unwrap_or(processor.accept_invalid_certs);

    // Create and execute the SMTP transaction
    let transaction = crate::smtp_transaction::SmtpTransaction::new(
        context,
        server_address.to_string(),
        require_tls,
        accept_invalid_certs,
        &processor.smtp_timeouts,
    );

    transaction.execute().await
}
