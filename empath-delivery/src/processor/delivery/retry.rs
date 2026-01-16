//! Retry logic for delivery attempts

use empath_common::DeliveryStatus;
use empath_common::tracing::{info, warn};
use empath_spool::SpooledMessageId;

use super::persistence::persist_delivery_state;
use crate::error::DeliveryError;
use crate::processor::DeliveryProcessor;
use crate::types::DeliveryInfo;

/// Try to fall back to the next MX server on temporary failure
///
/// Returns `true` if fallback was successful (caller should return early),
/// `false` if no more servers are available.
pub(super) async fn try_mx_fallback(
    processor: &DeliveryProcessor,
    message_id: &SpooledMessageId,
    is_temporary_failure: bool,
) -> bool {
    if !is_temporary_failure {
        return false;
    }

    if processor.queue.try_next_server(message_id)
        && let Some(updated) = processor.queue.get(message_id)
        && let Some(next_server) = updated.current_mail_server()
    {
        info!(
            message_id = %message_id,
            domain = %updated.recipient_domain,
            server = %next_server.host,
            priority = next_server.priority,
            delivery_attempt = updated.attempt_count(),
            "Trying next MX server"
        );
        processor
            .queue
            .update_status(message_id, DeliveryStatus::Pending);

        if let Some(spool) = &processor.spool
            && let Err(e) = persist_delivery_state(processor, message_id, spool).await
        {
            warn!(
                message_id = %message_id,
                error = %e,
                "Failed to persist delivery state after MX server fallback"
            );
        }
        return true;
    }

    false
}

/// Update the retry state based on attempt count and retry policy
///
/// Handles binding failover when the binding's retry policy is exhausted.
/// Returns the new status and the active retry policy.
pub(super) fn update_retry_state(
    processor: &DeliveryProcessor,
    message_id: &SpooledMessageId,
    info: &DeliveryInfo,
    error: &DeliveryError,
) -> (DeliveryStatus, crate::policy::RetryPolicy) {
    let binding_config = info
        .binding
        .as_deref()
        .and_then(|name| processor.outbound_bindings.get(name));
    let binding_retry_policy = binding_config.and_then(|binding| binding.retry_policy.clone());
    let binding_allows_failover = binding_config.is_some_and(|binding| binding.allow_failover);
    let global_retry_policy = processor.retry_policy.read().clone();
    let mut retry_policy = binding_retry_policy.unwrap_or_else(|| global_retry_policy.clone());

    if binding_config.is_some()
        && info.attempt_count() >= retry_policy.max_attempts
        && binding_allows_failover
        && global_retry_policy.should_retry(info.attempt_count())
    {
        processor.queue.set_binding(message_id, None);
        retry_policy = global_retry_policy;
        info!(
            message_id = %message_id,
            domain = %info.recipient_domain,
            "Binding retry policy exhausted; failing over to default binding"
        );
    }

    let new_status = if info.attempt_count() >= retry_policy.max_attempts {
        DeliveryStatus::Failed(error.to_string())
    } else {
        DeliveryStatus::Retry {
            attempts: info.attempt_count(),
            last_error: error.to_string(),
        }
    };

    processor
        .queue
        .update_status(message_id, new_status.clone());

    (new_status, retry_policy)
}

/// Apply retry delay based on exponential backoff
pub(super) fn apply_retry_delay(
    processor: &DeliveryProcessor,
    message_id: &SpooledMessageId,
    info: &DeliveryInfo,
    new_status: &DeliveryStatus,
    retry_policy: &crate::policy::RetryPolicy,
) {
    if matches!(new_status, DeliveryStatus::Retry { .. }) {
        let next_retry_at = retry_policy.calculate_next_retry(info.attempt_count());
        processor.queue.set_next_retry_at(message_id, next_retry_at);
        let delay_secs = next_retry_at
            .duration_since(std::time::SystemTime::now())
            .unwrap_or_default()
            .as_secs();
        info!(
            message_id = %message_id,
            domain = %info.recipient_domain,
            delivery_attempt = info.attempt_count(),
            retry_delay_secs = delay_secs,
            next_retry_at = ?next_retry_at,
            "Scheduled retry with exponential backoff"
        );
    }
}
