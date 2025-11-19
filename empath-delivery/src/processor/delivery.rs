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
    error::{DeliveryError, SystemError},
    processor::DeliveryProcessor,
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
            message_id = %message_id,
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

    // Create delivery pipeline to orchestrate DNS → Rate Limit → SMTP stages
    let dns_resolver = processor.dns_resolver.as_ref().ok_or_else(|| {
        SystemError::NotInitialized("DNS resolver not initialized. Call init() first.".to_string())
    })?;
    let domain_resolver = processor.domain_resolver.as_ref().ok_or_else(|| {
        SystemError::NotInitialized(
            "Domain policy resolver not initialized. Call init() first.".to_string(),
        )
    })?;

    let pipeline = crate::policy::DeliveryPipeline::new(
        &**dns_resolver,
        domain_resolver,
        processor.rate_limiter.as_ref(),
        processor.circuit_breaker_instance.as_ref(),
    );

    // Stage 1: Resolve mail servers (MX override or DNS lookup)
    let dns_resolution = pipeline
        .resolve_mail_servers(&info.recipient_domain)
        .await?;

    // Store the resolved mail servers
    processor
        .queue
        .set_mail_servers(message_id, dns_resolution.mail_servers.clone());

    // Use the primary (highest priority) mail server
    let mx_address = dns_resolution.primary_server.address();

    internal!(
        "Sending message to {:?} with MX host {} (priority {})",
        message_id,
        mx_address,
        dns_resolution.primary_server.priority
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

    // Stage 2: Check rate limit before attempting delivery
    match pipeline.check_rate_limit(message_id, &info.recipient_domain) {
        crate::policy::RateLimitResult::RateLimited { wait_time } => {
            // Rate limited - schedule retry
            let next_retry_at =
                crate::policy::DeliveryPipeline::calculate_rate_limit_retry(wait_time);

            processor.queue.set_next_retry_at(message_id, next_retry_at);
            processor
                .queue
                .update_status(message_id, DeliveryStatus::Pending);

            // Persist the delayed status to spool
            if let Err(e) = persist_delivery_state(processor, message_id, spool).await {
                warn!(
                    message_id = %message_id,
                    error = %e,
                    "Failed to persist delivery state after rate limit delay"
                );
            }

            return Ok(()); // Not an error, just delayed
        }
        crate::policy::RateLimitResult::Allowed => {
            // Proceed with delivery
        }
    }

    // Audit log: Delivery attempt
    empath_common::audit::log_delivery_attempt(
        &message_id.to_string(),
        &info.recipient_domain,
        &mx_address,
        usize::try_from(info.attempt_count()).unwrap_or(0) + 1, // Next attempt number (1-based)
    );

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
                    message_id = %message_id,
                    error = %e,
                    "Failed to persist delivery state after successful delivery"
                );
            }

            // Delete the message from the spool after successful delivery
            if let Err(e) = spool.delete(message_id).await {
                error!(
                    message_id = %message_id,
                    error = %e,
                    "Failed to delete message from spool after successful delivery - adding to cleanup queue for retry"
                );
                // Add to cleanup queue for retry with exponential backoff
                processor
                    .cleanup_queue
                    .add_failed_deletion(message_id.clone());
                // Don't fail the delivery just because we couldn't delete the spool file
                // The message was delivered successfully
            }

            // Remove from in-memory queue to keep queue synchronized with spool
            // This ensures `queue list` and `queue stats` reflect the actual state
            processor.queue.remove(message_id);

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

            // Audit log: Delivery successful
            let duration_ms = info.queued_at.elapsed().map_or(0, |d| d.as_millis());
            empath_common::audit::log_delivery_success(
                &message_id.to_string(),
                &info.recipient_domain,
                &mx_address,
                usize::try_from(info.attempt_count()).unwrap_or(0) + 1,
                duration_ms,
            );

            // Stage 3: Record successful delivery in circuit breaker
            pipeline.record_success(&info.recipient_domain);

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
        timestamp: std::time::SystemTime::now(),
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
            message_id = %message_id,
            domain = %info.recipient_domain,
            server = %next_server.host,
            priority = next_server.priority,
            delivery_attempt = info.attempt_count(),
            "Trying next MX server"
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
                message_id = %message_id,
                error = %e,
                "Failed to persist delivery state after MX server fallback"
            );
        }

        return error;
    }

    // All MX servers exhausted or permanent failure, use normal retry logic
    // Determine new status based on attempt count
    let new_status = if updated_info.attempt_count() >= processor.retry_policy.max_attempts {
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
        let next_retry_at = processor
            .retry_policy
            .calculate_next_retry(updated_info.attempt_count());

        processor.queue.set_next_retry_at(message_id, next_retry_at);

        // Calculate delay for logging
        let delay_secs = next_retry_at
            .duration_since(std::time::SystemTime::now())
            .unwrap_or_default()
            .as_secs();

        info!(
            message_id = %message_id,
            domain = %updated_info.recipient_domain,
            delivery_attempt = updated_info.attempt_count(),
            retry_delay_secs = delay_secs,
            next_retry_at = ?next_retry_at,
            "Scheduled retry with exponential backoff"
        );
    }

    // Persist the updated status (Retry or Failed) to spool
    if let Some(spool) = &processor.spool
        && let Err(e) = persist_delivery_state(processor, message_id, spool).await
    {
        warn!(
            message_id = %message_id,
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

    // Audit log: Delivery failed
    empath_common::audit::log_delivery_failure(
        &message_id.to_string(),
        &updated_info.recipient_domain,
        &error.to_string(),
        usize::try_from(updated_info.attempt_count()).unwrap_or(0),
        &format!("{new_status:?}"),
    );

    // Create delivery pipeline for circuit breaker tracking
    if let Some(dns_resolver) = &processor.dns_resolver
        && let Some(domain_resolver) = &processor.domain_resolver
    {
        let pipeline = crate::policy::DeliveryPipeline::new(
            &**dns_resolver,
            domain_resolver,
            processor.rate_limiter.as_ref(),
            processor.circuit_breaker_instance.as_ref(),
        );

        // Record circuit breaker failure (only for temporary failures)
        // Permanent failures are recipient/config issues, not server health problems
        pipeline.record_failure(&updated_info.recipient_domain, is_temporary_failure);
    }

    // Generate DSN if appropriate
    if processor.dsn.enabled && crate::dsn::should_generate_dsn(context, &updated_info, &error) {
        match crate::dsn::generate_dsn(context, &updated_info, &error, &processor.dsn) {
            Ok(dsn_context) => {
                // Spool the DSN for delivery
                if let Some(spool) = &processor.spool {
                    let mut dsn_context_mut = dsn_context;
                    match spool.write(&mut dsn_context_mut).await {
                        Ok(dsn_id) => {
                            info!(
                                message_id = %message_id,
                                dsn_id = %dsn_id,
                                original_sender = %context.sender(),
                                "DSN generated and spooled successfully"
                            );
                        }
                        Err(e) => {
                            warn!(
                                message_id = %message_id,
                                error = %e,
                                "Failed to spool DSN message"
                            );
                        }
                    }
                } else {
                    warn!(
                        message_id = %message_id,
                        "Cannot spool DSN: spool not initialized"
                    );
                }
            }
            Err(e) => {
                warn!(
                    message_id = %message_id,
                    error = %e,
                    "Failed to generate DSN"
                );
            }
        }
    }

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
    // Get the domain policy resolver (already checked earlier in prepare_delivery)
    let Some(domain_resolver) = &processor.domain_resolver else {
        return Err(SystemError::NotInitialized(
            "Domain policy resolver not initialized. Call init() first.".to_string(),
        )
        .into());
    };

    // Check if TLS is required for this domain
    let require_tls = domain_resolver.requires_tls(&delivery_info.recipient_domain);

    // Determine if we should accept invalid certificates
    // Priority: per-domain override > global configuration
    let accept_invalid_certs =
        domain_resolver.accepts_invalid_certs(&delivery_info.recipient_domain);

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
