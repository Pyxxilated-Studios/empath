//! Queue processing logic for delivery attempts

use std::sync::Arc;

use empath_common::{DeliveryStatus, tracing::{debug, error, warn}};

use crate::{
    dns::MailServer,
    error::DeliveryError,
    processor::{DeliveryProcessor, delivery::prepare_message},
};

/// Process all pending messages in the queue
///
/// This method:
/// 1. Checks for expired messages and marks them as `Expired`
/// 2. For messages with `Retry` status, checks if it's time to retry
/// 3. Processes messages that are ready for delivery
///
/// # Errors
/// Returns an error if processing fails
#[allow(
    clippy::too_many_lines,
    reason = "Queue processing logic naturally requires many branches"
)]
pub async fn process_queue_internal(
    processor: &DeliveryProcessor,
    spool: &Arc<dyn empath_spool::BackingStore>,
) -> Result<(), DeliveryError> {
    let current_time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    // Get all messages to check for expiration and retry timing
    let all_messages = processor.queue.all_messages().await;

    for info in all_messages {
        // Skip messages that are already completed, failed, expired, or in progress
        if matches!(
            info.status,
            DeliveryStatus::Completed
                | DeliveryStatus::Failed(_)
                | DeliveryStatus::Expired
                | DeliveryStatus::InProgress
        ) {
            continue;
        }

        // Check if message has expired
        if let Some(expiration_secs) = processor.message_expiration_secs {
            let age_secs = current_time.saturating_sub(info.queued_at);
            if age_secs > expiration_secs {
                warn!(
                    message_id = ?info.message_id,
                    age_secs = age_secs,
                    expiration_secs = expiration_secs,
                    "Message expired, marking as Expired"
                );
                processor
                    .queue
                    .update_status(&info.message_id, DeliveryStatus::Expired)
                    .await;

                // Persist the Expired status to spool
                if let Err(e) =
                    super::delivery::persist_delivery_state(processor, &info.message_id, spool)
                        .await
                {
                    warn!(
                        message_id = ?info.message_id,
                        error = %e,
                        "Failed to persist delivery state after marking message as Expired"
                    );
                }

                continue;
            }
        }

        // For Retry status, check if it's time to retry
        if matches!(info.status, DeliveryStatus::Retry { .. }) {
            if let Some(next_retry_at) = info.next_retry_at
                && current_time < next_retry_at
            {
                // Not yet time to retry, skip this message
                let wait_secs = next_retry_at.saturating_sub(current_time);
                debug!(
                    message_id = ?info.message_id,
                    wait_secs = wait_secs,
                    "Skipping message, not yet time to retry"
                );
                continue;
            }

            // Time to retry! Reset status to Pending and reset server index
            debug!(
                message_id = ?info.message_id,
                attempt = info.attempt_count(),
                "Time to retry delivery"
            );
            processor
                .queue
                .update_status(&info.message_id, DeliveryStatus::Pending)
                .await;

            // Reset to first MX server for new retry cycle
            processor
                .queue
                .reset_server_index(&info.message_id)
                .await;

            // Persist the Pending status for retry
            if let Err(e) =
                super::delivery::persist_delivery_state(processor, &info.message_id, spool).await
            {
                warn!(
                    message_id = ?info.message_id,
                    error = %e,
                    "Failed to persist delivery state after marking message for retry"
                );
            }
        }

        // Process the message (Pending status)
        if matches!(info.status, DeliveryStatus::Pending)
            && let Err(e) = prepare_message(processor, &info.message_id, spool).await
        {
            error!(
                message_id = ?info.message_id,
                error = %e,
                "Failed to prepare message for delivery"
            );

            if let Ok(mut context) = spool.read(&info.message_id).await {
                let server = info
                    .mail_servers
                    .get(info.current_server_index)
                    .map_or_else(|| info.recipient_domain.to_string(), MailServer::address);
                let _error = super::delivery::handle_delivery_error(
                    processor,
                    &info.message_id,
                    &mut context,
                    e,
                    server,
                )
                .await;
            }
        }
    }

    Ok(())
}
