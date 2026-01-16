//! Queue processing logic for delivery attempts

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::SystemTime;

use empath_common::DeliveryStatus;
use empath_common::email::ServerId;
use empath_common::tracing::{error, info, warn};
use empath_spool::{BackingStore, SpooledMessageId};
use tokio::task::JoinSet;

use super::{delivery, gates};
use crate::dns::MailServer;
use crate::error::DeliveryError;
use crate::processor::DeliveryProcessor;
use crate::processor::delivery::{prepare_message, server_id_from};
use crate::types::DeliveryInfo;

/// A prepared delivery task that can be spawned on a `JoinSet`
///
/// This type encapsulates all the state needed to process a single message
/// delivery, allowing the caller to control when and how it's executed.
pub struct DeliveryTask {
    /// The future to execute
    future: Pin<Box<dyn Future<Output = ()> + Send + 'static>>,
}

impl DeliveryTask {
    /// Create a new delivery task for the given message
    pub fn new(
        processor: Arc<DeliveryProcessor>,
        spool: Arc<dyn BackingStore>,
        message_id: &SpooledMessageId,
    ) -> Option<Self> {
        let info = processor.queue.get(message_id)?;
        let future = Box::pin(process_single_message(processor, spool, info));
        Some(Self { future })
    }

    /// Spawn this task on a `JoinSet`
    pub fn spawn_on(self, join_set: &mut JoinSet<()>) {
        join_set.spawn(self.future);
    }
}

fn handle_circuit_breaker_gate(processor: &DeliveryProcessor, info: &DeliveryInfo) -> bool {
    let Some(circuit_breaker) = processor.traffic_control.circuit_breaker() else {
        return true; // No circuit breaker configured, allow
    };

    let decision = circuit_breaker.decide(&info.recipient_domain, info.attempt_count());

    if decision.is_blocked() {
        warn!(
            message_id = ?info.message_id,
            domain = %info.recipient_domain,
            "Circuit breaker is OPEN - rejecting delivery attempt to prevent retry storm"
        );
    }

    // Returns true if blocked (state was updated), we negate for "should proceed"
    !processor.queue.apply(&info.message_id, &decision)
}

/// Process a single message for delivery (spawned as a task)
async fn process_single_message(
    processor: Arc<DeliveryProcessor>,
    spool: Arc<dyn BackingStore>,
    info: DeliveryInfo,
) {
    // Record queue age metric before attempting delivery
    if let Some(metrics) = &processor.metrics {
        metrics.record_queue_age(info.queued_at);
    }

    // Check circuit breaker before attempting delivery
    if !handle_circuit_breaker_gate(&processor, &info) {
        return;
    }

    if !gates::check_binding_disabled(&processor, &spool, &info).await {
        return;
    }

    if !gates::check_binding_health(&processor, &spool, &info).await {
        return;
    }

    if let Err(e) = Box::pin(prepare_message(&processor, &info.message_id, &spool)).await {
        error!(
            message_id = ?info.message_id,
            error = %e,
            "Failed to prepare message for delivery"
        );

        let should_handle = processor
            .queue
            .get(&info.message_id)
            .is_some_and(|entry| matches!(entry.status, DeliveryStatus::InProgress));
        if should_handle && let Ok(mut context) = spool.read(&info.message_id).await {
            let server = info
                .mail_servers
                .get(info.current_server_index)
                .map_or_else(|| info.recipient_domain.to_string(), MailServer::address);
            let server_id = server_id_from(&server).unwrap_or_else(|err| {
                warn!(
                    message_id = ?info.message_id,
                    server = server,
                    error = %err,
                    "Failed to validate server id, using placeholder"
                );
                // Static value avoids panic while preserving attempt record
                ServerId::new_unchecked("unknown-server")
            });
            delivery::handle_delivery_error(
                &processor,
                &info.message_id,
                &mut context,
                &e,
                server_id,
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
pub async fn process_queue_internal(
    processor: Arc<DeliveryProcessor>,
    spool: Arc<dyn BackingStore>,
) -> Result<(), DeliveryError> {
    let now = SystemTime::now();

    let scan_result = scan_queue(&processor, now);

    if let Some(metrics) = &processor.metrics {
        metrics.update_oldest_message_age(scan_result.oldest_pending_age_secs);
    }

    persist_updates(&processor, &spool, scan_result.to_persist_ids).await;
    process_pending_messages(processor, spool, scan_result.pending_ids).await;

    Ok(())
}

fn scan_queue(processor: &DeliveryProcessor, now: SystemTime) -> crate::queue::ScanResult {
    let config = crate::queue::ScanConfig {
        message_expiration_secs: processor.message_expiration_secs,
    };
    processor.queue.scan(now, &config)
}

async fn persist_updates(
    processor: &DeliveryProcessor,
    spool: &Arc<dyn BackingStore>,
    to_persist: Vec<SpooledMessageId>,
) {
    for message_id in to_persist {
        if let Err(e) = delivery::persist_delivery_state(processor, &message_id, spool).await {
            warn!(
                message_id = ?message_id,
                error = %e,
                "Failed to persist delivery state after status change"
            );
        }
    }
}

async fn process_pending_messages(
    processor: Arc<DeliveryProcessor>,
    spool: Arc<dyn BackingStore>,
    pending_message_ids: Vec<SpooledMessageId>,
) {
    if pending_message_ids.is_empty() {
        return;
    }

    info!(
        pending_count = pending_message_ids.len(),
        max_concurrent = processor.max_concurrent_deliveries,
        "Processing delivery queue with parallel workers"
    );

    let mut join_set = JoinSet::new();
    let mut pending_iter = pending_message_ids.into_iter();

    // Spawn initial batch of workers using idiomatic .take() adaptor
    for message_id in pending_iter
        .by_ref()
        .take(processor.max_concurrent_deliveries)
    {
        if let Some(task) =
            DeliveryTask::new(Arc::clone(&processor), Arc::clone(&spool), &message_id)
        {
            task.spawn_on(&mut join_set);
        }
    }

    // As each task completes, spawn the next pending message
    while join_set.join_next().await.is_some() {
        if let Some(message_id) = pending_iter.next()
            && let Some(task) =
                DeliveryTask::new(Arc::clone(&processor), Arc::clone(&spool), &message_id)
        {
            task.spawn_on(&mut join_set);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use empath_common::context::DeliveryPriorityTier;
    use empath_spool::SpooledMessageId;

    use super::scan_queue;
    use crate::DeliveryInfo;
    use crate::processor::DeliveryProcessor;

    #[test]
    #[cfg_attr(miri, ignore = "Miri does not support SystemTime::now with isolation")]
    fn scan_queue_orders_by_priority_and_age() {
        let processor = DeliveryProcessor::default();
        let now = SystemTime::now();

        let mut high_old = DeliveryInfo::new_with_priority(
            SpooledMessageId::generate(),
            "example.com".to_string(),
            DeliveryPriorityTier::High,
        );
        high_old.queued_at = SystemTime::UNIX_EPOCH + Duration::from_secs(1);

        let mut normal_new = DeliveryInfo::new_with_priority(
            SpooledMessageId::generate(),
            "example.com".to_string(),
            DeliveryPriorityTier::Normal,
        );
        normal_new.queued_at = SystemTime::UNIX_EPOCH + Duration::from_secs(3);

        let mut high_newer = DeliveryInfo::new_with_priority(
            SpooledMessageId::generate(),
            "example.com".to_string(),
            DeliveryPriorityTier::High,
        );
        high_newer.queued_at = SystemTime::UNIX_EPOCH + Duration::from_secs(2);

        let high_old_id = high_old.message_id.clone();
        let normal_new_id = normal_new.message_id.clone();
        let high_newer_id = high_newer.message_id.clone();

        processor.queue.insert(high_old_id.clone(), high_old);
        processor.queue.insert(normal_new_id.clone(), normal_new);
        processor.queue.insert(high_newer_id.clone(), high_newer);

        let result = scan_queue(&processor, now);
        assert_eq!(result.pending_ids.len(), 3);
        assert_eq!(result.pending_ids[0], high_old_id);
        assert_eq!(result.pending_ids[1], high_newer_id);
        assert_eq!(result.pending_ids[2], normal_new_id);
    }
}
