//! Delivery gates for pre-flight checks before message delivery
//!
//! Gates are pre-delivery checks that can block a message from being sent.
//! When a gate blocks delivery, it schedules a retry rather than failing the message.

use std::sync::Arc;

use empath_common::DeliveryStatus;
use empath_common::tracing::warn;
use empath_spool::{BackingStore, SpooledMessageId};

use super::delivery;
use crate::processor::DeliveryProcessor;
use crate::types::DeliveryInfo;

/// Reason why a gate blocked delivery
#[derive(Debug, Clone)]
pub enum BlockReason {
    /// The outbound binding is disabled
    BindingDisabled,
    /// The outbound binding is unhealthy
    BindingUnhealthy,
}

impl BlockReason {
    /// Get the error message for retry status
    #[must_use]
    pub const fn error_message(&self) -> &'static str {
        match self {
            Self::BindingDisabled => "Outbound binding disabled",
            Self::BindingUnhealthy => "Outbound binding unhealthy",
        }
    }

    /// Get the log message for the warning
    #[must_use]
    pub const fn log_message(&self) -> &'static str {
        match self {
            Self::BindingDisabled => "Outbound binding is disabled - delaying delivery",
            Self::BindingUnhealthy => "Outbound binding is unhealthy - delaying delivery",
        }
    }
}

/// Handle a blocked binding gate by scheduling a retry
///
/// This function contains the common logic for both binding gates:
/// 1. Log the block reason
/// 2. Handle failover if allowed
/// 3. Update status to Retry
/// 4. Calculate and set next retry time
/// 5. Persist delivery state
async fn handle_binding_blocked(
    processor: &DeliveryProcessor,
    spool: &Arc<dyn BackingStore>,
    message_id: &SpooledMessageId,
    binding_name: &str,
    attempt_count: u32,
    reason: BlockReason,
) {
    warn!(
        message_id = ?message_id,
        binding = binding_name,
        "{}",
        reason.log_message()
    );

    // Handle failover if the binding allows it
    let binding = processor.outbound_bindings.get(binding_name);
    if binding.is_some_and(|binding| binding.allow_failover) {
        processor.queue.set_binding(message_id, None);
    }

    // Update status to Retry
    processor.queue.update_status(
        message_id,
        DeliveryStatus::Retry {
            attempts: attempt_count,
            last_error: reason.error_message().to_string(),
        },
    );

    // Calculate and set next retry time
    let next_retry_at = processor
        .retry_policy
        .read()
        .calculate_next_retry(attempt_count);
    processor.queue.set_next_retry_at(message_id, next_retry_at);

    // Persist delivery state
    if let Err(e) = delivery::persist_delivery_state(processor, message_id, spool).await {
        warn!(
            message_id = ?message_id,
            error = %e,
            "Failed to persist delivery state after {} gate",
            match reason {
                BlockReason::BindingDisabled => "binding disable",
                BlockReason::BindingUnhealthy => "binding health",
            }
        );
    }
}

/// Check the binding disabled gate
///
/// Returns `true` if delivery should proceed, `false` if blocked.
pub async fn check_binding_disabled(
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

    handle_binding_blocked(
        processor,
        spool,
        &info.message_id,
        binding_name,
        info.attempt_count(),
        BlockReason::BindingDisabled,
    )
    .await;

    false
}

/// Check the binding health gate
///
/// Returns `true` if delivery should proceed, `false` if blocked.
pub async fn check_binding_health(
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

    handle_binding_blocked(
        processor,
        spool,
        &info.message_id,
        binding_name,
        info.attempt_count(),
        BlockReason::BindingUnhealthy,
    )
    .await;

    // Record metrics for health state
    if let Some(metrics) = &processor.metrics {
        metrics.record_binding_health_state(binding_name, health_state.as_metric_value());
    }

    false
}
