//! Queue processing logic for delivery attempts

use std::{sync::Arc, time::SystemTime};

use empath_common::{
    DeliveryStatus,
    tracing::{debug, error, info, warn},
};
use tokio::task::JoinSet;

use crate::{
    dns::MailServer,
    error::DeliveryError,
    processor::{DeliveryProcessor, delivery::prepare_message},
    types::DeliveryInfo,
};

/// Process a single message for delivery (spawned as a task)
async fn process_single_message(
    processor: Arc<DeliveryProcessor>,
    spool: Arc<dyn empath_spool::BackingStore>,
    info: DeliveryInfo,
) {
    // Record queue age metric before attempting delivery
    if let Some(metrics) = &processor.metrics {
        metrics.record_queue_age(info.queued_at);
    }

    // Check circuit breaker before attempting delivery
    if let Some(circuit_breaker) = &processor.circuit_breaker_instance
        && !circuit_breaker.should_allow_delivery(&info.recipient_domain)
    {
        warn!(
            message_id = ?info.message_id,
            domain = %info.recipient_domain,
            "Circuit breaker is OPEN - rejecting delivery attempt to prevent retry storm"
        );
        // Update next retry time to check again after circuit timeout
        let next_retry = std::time::SystemTime::now()
            + std::time::Duration::from_secs(processor.circuit_breaker.timeout_secs);
        processor.queue.update_status(
            &info.message_id,
            DeliveryStatus::Retry {
                attempts: info.attempt_count(),
                last_error: "Circuit breaker open".to_string(),
            },
        );
        processor
            .queue
            .set_next_retry_at(&info.message_id, next_retry);
        return; // Skip delivery
    }

    if let Err(e) = prepare_message(&processor, &info.message_id, &spool).await {
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
                &processor,
                &info.message_id,
                &mut context,
                e,
                server,
            )
            .await;
        }
    }
}

/// Process all pending messages in the queue with parallel delivery
///
/// This method:
/// 1. Checks for expired messages and marks them as `Expired`
/// 2. For messages with `Retry` status, checks if it's time to retry
/// 3. Processes messages that are ready for delivery in parallel (up to `max_concurrent_deliveries`)
///
/// # Errors
/// Returns an error if processing fails
#[allow(
    clippy::too_many_lines,
    reason = "Queue processing logic naturally requires many branches"
)]
pub async fn process_queue_internal(
    processor: Arc<DeliveryProcessor>,
    spool: Arc<dyn empath_spool::BackingStore>,
) -> Result<(), DeliveryError> {
    let now = SystemTime::now();

    // Vector to collect messages ready for parallel delivery
    let mut pending_messages = Vec::new();

    // Get all messages to check for expiration and retry timing
    let all_messages = processor.queue.all_messages();

    // Calculate and update the oldest pending message age
    if let Some(metrics) = &processor.metrics {
        let oldest_age_secs = all_messages
            .iter()
            .filter(|msg| {
                matches!(
                    msg.status,
                    DeliveryStatus::Pending | DeliveryStatus::Retry { .. }
                )
            })
            .filter_map(|msg| now.duration_since(msg.queued_at).ok())
            .map(|duration| duration.as_secs())
            .max()
            .unwrap_or(0);

        metrics.update_oldest_message_age(oldest_age_secs);
    }

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
            let age_secs = now
                .duration_since(info.queued_at)
                .unwrap_or_default()
                .as_secs();
            if age_secs > expiration_secs {
                warn!(
                    message_id = ?info.message_id,
                    age_secs = age_secs,
                    expiration_secs = expiration_secs,
                    "Message expired, marking as Expired"
                );
                processor
                    .queue
                    .update_status(&info.message_id, DeliveryStatus::Expired);

                // Persist the Expired status to spool
                if let Err(e) =
                    super::delivery::persist_delivery_state(&processor, &info.message_id, &spool)
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
                && now < next_retry_at
            {
                // Not yet time to retry, skip this message
                let wait_secs = next_retry_at
                    .duration_since(now)
                    .unwrap_or_default()
                    .as_secs();
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
                .update_status(&info.message_id, DeliveryStatus::Pending);

            // Reset to first MX server for new retry cycle
            processor.queue.reset_server_index(&info.message_id);

            // Persist the Pending status for retry
            if let Err(e) =
                super::delivery::persist_delivery_state(&processor, &info.message_id, &spool).await
            {
                warn!(
                    message_id = ?info.message_id,
                    error = %e,
                    "Failed to persist delivery state after marking message for retry"
                );
            }
        }

        // Collect Pending messages for parallel processing
        if matches!(info.status, DeliveryStatus::Pending) {
            pending_messages.push(info);
        }
    }

    // Process pending messages in parallel using JoinSet
    if !pending_messages.is_empty() {
        info!(
            pending_count = pending_messages.len(),
            max_concurrent = processor.max_concurrent_deliveries,
            "Processing delivery queue with parallel workers"
        );

        let mut join_set: JoinSet<()> = JoinSet::new();
        let mut pending_iter = pending_messages.into_iter();

        // Spawn initial batch of tasks (up to max_concurrent_deliveries)
        for _ in 0..processor.max_concurrent_deliveries.min(pending_iter.len()) {
            if let Some(msg_info) = pending_iter.next() {
                // Clone Arc for this task
                let processor_clone = Arc::clone(&processor);
                let spool_clone = Arc::clone(&spool);

                join_set.spawn(async move {
                    process_single_message(processor_clone, spool_clone, msg_info).await;
                });
            }
        }

        // As tasks complete, spawn new ones for remaining messages
        while join_set.join_next().await.is_some() {
            if let Some(msg_info) = pending_iter.next() {
                let processor_clone = Arc::clone(&processor);
                let spool_clone = Arc::clone(&spool);

                join_set.spawn(async move {
                    process_single_message(processor_clone, spool_clone, msg_info).await;
                });
            }
        }
    }

    Ok(())
}
