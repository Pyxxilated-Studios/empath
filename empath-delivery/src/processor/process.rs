//! Queue processing logic for delivery attempts

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use empath_common::DeliveryStatus;
use empath_common::email::ServerId;
use empath_common::tracing::{debug, error, info, warn};
use empath_spool::{BackingStore, SpooledMessageId};
use tokio::task::JoinSet;

use super::delivery;
use crate::dns::MailServer;
use crate::error::DeliveryError;
use crate::processor::DeliveryProcessor;
use crate::processor::delivery::{prepare_message, server_id_from};
use crate::types::DeliveryInfo;

fn handle_circuit_breaker_gate(processor: &DeliveryProcessor, info: &DeliveryInfo) -> bool {
    if let Some(circuit_breaker) = &processor.circuit_breaker_instance
        && !circuit_breaker.should_allow_delivery(&info.recipient_domain)
    {
        // Read timeout_secs directly without cloning entire config
        let timeout_secs = processor.circuit_breaker.read().timeout_secs;
        warn!(
            message_id = ?info.message_id,
            domain = %info.recipient_domain,
            "Circuit breaker is OPEN - rejecting delivery attempt to prevent retry storm"
        );
        // Update next retry time to check again after circuit timeout
        let next_retry = SystemTime::now() + Duration::from_secs(timeout_secs);
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
        return false;
    }

    true
}

async fn handle_binding_disabled_gate(
    processor: &DeliveryProcessor,
    spool: &Arc<dyn BackingStore>,
    info: &DeliveryInfo,
) -> bool {
    let Some(binding_name) = info.binding.as_deref() else {
        return true;
    };

    if processor.binding_enabled(binding_name) {
        return true;
    }

    warn!(
        message_id = ?info.message_id,
        binding = binding_name,
        "Outbound binding is disabled - delaying delivery"
    );

    let binding = processor.outbound_bindings.get(binding_name);
    if binding.is_some_and(|binding| binding.allow_failover) {
        processor.queue.set_binding(&info.message_id, None);
    }

    processor.queue.update_status(
        &info.message_id,
        DeliveryStatus::Retry {
            attempts: info.attempt_count(),
            last_error: "Outbound binding disabled".to_string(),
        },
    );

    // Call method directly on read guard without cloning entire config
    let next_retry_at = processor
        .retry_policy
        .read()
        .calculate_next_retry(info.attempt_count());
    processor
        .queue
        .set_next_retry_at(&info.message_id, next_retry_at);

    if let Err(e) = delivery::persist_delivery_state(processor, &info.message_id, spool).await {
        warn!(
            message_id = ?info.message_id,
            error = %e,
            "Failed to persist delivery state after binding disable gate"
        );
    }

    false
}

async fn handle_binding_health_gate(
    processor: &DeliveryProcessor,
    spool: &Arc<dyn BackingStore>,
    info: &DeliveryInfo,
) -> bool {
    let Some(binding_name) = info.binding.as_deref() else {
        return true;
    };

    let Some(binding_health) = &processor.binding_health else {
        return true;
    };

    let health_state = binding_health.should_allow(
        binding_name,
        processor.binding_health_config_for(binding_name),
    );
    if !matches!(health_state, crate::BindingHealthState::Open) {
        return true;
    }

    warn!(
        message_id = ?info.message_id,
        binding = binding_name,
        "Outbound binding is unhealthy - delaying delivery"
    );

    let binding = processor.outbound_bindings.get(binding_name);
    if binding.is_some_and(|binding| binding.allow_failover) {
        processor.queue.set_binding(&info.message_id, None);
    }

    processor.queue.update_status(
        &info.message_id,
        DeliveryStatus::Retry {
            attempts: info.attempt_count(),
            last_error: "Outbound binding unhealthy".to_string(),
        },
    );

    // Call method directly on read guard without cloning entire config
    let next_retry_at = processor
        .retry_policy
        .read()
        .calculate_next_retry(info.attempt_count());
    processor
        .queue
        .set_next_retry_at(&info.message_id, next_retry_at);

    if let Err(e) = delivery::persist_delivery_state(processor, &info.message_id, spool).await {
        warn!(
            message_id = ?info.message_id,
            error = %e,
            "Failed to persist delivery state after binding health gate"
        );
    }

    if let Some(metrics) = &processor.metrics {
        metrics.record_binding_health_state(binding_name, health_state.as_metric_value());
    }

    false
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

    if !handle_binding_disabled_gate(&processor, &spool, &info).await {
        return;
    }

    if !handle_binding_health_gate(&processor, &spool, &info).await {
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
            let _error = delivery::handle_delivery_error(
                &processor,
                &info.message_id,
                &mut context,
                e,
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

    let (pending_message_ids, to_persist, oldest_pending_age_secs) = scan_queue(&processor, now);

    if let Some(metrics) = &processor.metrics {
        metrics.update_oldest_message_age(oldest_pending_age_secs);
    }

    persist_updates(&processor, &spool, to_persist).await;
    process_pending_messages(processor, spool, pending_message_ids).await;

    Ok(())
}

fn scan_queue(
    processor: &DeliveryProcessor,
    now: SystemTime,
) -> (Vec<SpooledMessageId>, Vec<SpooledMessageId>, u64) {
    let mut pending_messages = Vec::new();
    let mut to_persist = Vec::new();
    let mut oldest_pending_age_secs = 0u64;

    // Use active_message_ids for O(1) lookup instead of O(n) scan of entire queue
    // This avoids iterating over Completed/Failed/Expired/InProgress messages
    let active_ids: Vec<_> = processor.queue.active_message_ids().collect();

    for message_id in active_ids {
        let Some(mut entry) = processor.queue.queue.get_mut(&message_id) else {
            // Message was removed between collecting IDs and processing
            processor.queue.mark_inactive(&message_id);
            continue;
        };
        let info = entry.value_mut();

        // Double-check status in case it changed (defensive)
        if matches!(
            info.status,
            DeliveryStatus::Completed
                | DeliveryStatus::Failed(_)
                | DeliveryStatus::Expired
                | DeliveryStatus::InProgress
        ) {
            processor.queue.mark_inactive(&message_id);
            continue;
        }

        if let Ok(age) = now.duration_since(info.queued_at) {
            oldest_pending_age_secs = oldest_pending_age_secs.max(age.as_secs());
        }

        if is_expired(processor, info, now) {
            processor.queue.mark_inactive(&message_id);
            to_persist.push(info.message_id.clone());
            continue;
        }

        if !retry_ready(info, now, &mut to_persist) {
            continue;
        }

        if matches!(info.status, DeliveryStatus::Pending) {
            let queued_at_secs = info
                .queued_at
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            pending_messages.push((
                info.priority_tier.rank(),
                queued_at_secs,
                info.message_id.clone(),
            ));
        }
    }

    pending_messages.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    let pending_message_ids = pending_messages
        .into_iter()
        .map(|(_, _, message_id)| message_id)
        .collect();

    (pending_message_ids, to_persist, oldest_pending_age_secs)
}

fn is_expired(processor: &DeliveryProcessor, info: &mut DeliveryInfo, now: SystemTime) -> bool {
    let Some(expiration_secs) = processor.message_expiration_secs else {
        return false;
    };
    let age_secs = now
        .duration_since(info.queued_at)
        .unwrap_or_default()
        .as_secs();
    if age_secs <= expiration_secs {
        return false;
    }

    warn!(
        message_id = ?info.message_id,
        age_secs = age_secs,
        expiration_secs = expiration_secs,
        "Message expired, marking as Expired"
    );
    info.status = DeliveryStatus::Expired;
    true
}

fn retry_ready(
    info: &mut DeliveryInfo,
    now: SystemTime,
    to_persist: &mut Vec<SpooledMessageId>,
) -> bool {
    if !matches!(info.status, DeliveryStatus::Retry { .. }) {
        return true;
    }

    if let Some(next_retry_at) = info.next_retry_at
        && now < next_retry_at
    {
        let wait_secs = next_retry_at
            .duration_since(now)
            .unwrap_or_default()
            .as_secs();
        debug!(
            message_id = ?info.message_id,
            wait_secs = wait_secs,
            "Skipping message, not yet time to retry"
        );
        return false;
    }

    debug!(
        message_id = ?info.message_id,
        attempt = info.attempt_count(),
        "Time to retry delivery"
    );
    info.status = DeliveryStatus::Pending;
    info.reset_server_index();
    to_persist.push(info.message_id.clone());
    true
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

    let mut join_set: JoinSet<()> = JoinSet::new();
    let mut pending_iter = pending_message_ids.into_iter();

    for _ in 0..processor.max_concurrent_deliveries.min(pending_iter.len()) {
        if let Some(message_id) = pending_iter.next() {
            spawn_delivery_task(&mut join_set, &processor, &spool, message_id);
        }
    }

    while join_set.join_next().await.is_some() {
        if let Some(message_id) = pending_iter.next() {
            spawn_delivery_task(&mut join_set, &processor, &spool, message_id);
        }
    }
}

fn spawn_delivery_task(
    join_set: &mut JoinSet<()>,
    processor: &Arc<DeliveryProcessor>,
    spool: &Arc<dyn BackingStore>,
    message_id: SpooledMessageId,
) {
    let processor_clone = Arc::clone(processor);
    let spool_clone = Arc::clone(spool);

    join_set.spawn(async move {
        if let Some(info) = processor_clone.queue.get(&message_id) {
            Box::pin(process_single_message(processor_clone, spool_clone, info)).await;
        }
    });
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

        let (pending, _persist, _age) = scan_queue(&processor, now);
        assert_eq!(pending.len(), 3);
        assert_eq!(pending[0], high_old_id);
        assert_eq!(pending[1], high_newer_id);
        assert_eq!(pending[2], normal_new_id);
    }
}
