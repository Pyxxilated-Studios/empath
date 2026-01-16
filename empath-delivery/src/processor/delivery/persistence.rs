//! Persistence helpers for delivery state

use std::sync::Arc;

use empath_common::DeliveryStatus;
use empath_common::context::DeliveryContext;
use empath_common::tracing::warn;
use empath_spool::{BackingStore, SpooledMessageId};

use super::server_id_from;
use crate::error::{DeliveryError, SystemError};
use crate::processor::DeliveryProcessor;

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
    spool: &Arc<dyn BackingStore>,
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

    let current_server = info
        .current_mail_server()
        .map(|srv| server_id_from(&srv.address()))
        .transpose()?;

    // Update the delivery field with current queue state
    context.delivery = Some(DeliveryContext {
        message_id: message_id.clone(),
        domain: info.recipient_domain.clone(),
        server: current_server,
        error: match &info.status {
            DeliveryStatus::Failed(e) | DeliveryStatus::Retry { last_error: e, .. } => {
                Some(e.clone())
            }
            _ => None,
        },
        attempts: Some(info.attempt_count()),
        binding: info.binding.clone(),
        status: info.status.clone(),
        attempt_history: info.attempts.clone(),
        queued_at: info.queued_at,
        next_retry_at: info.next_retry_at,
        current_server_index: info.current_server_index,
        priority_tier: info.priority_tier,
    });

    // Atomically update spool
    spool
        .update(message_id, &context)
        .await
        .map_err(|e| SystemError::SpoolWrite(e.to_string()))?;

    Ok(())
}

/// Persist delivery state after an error, logging failures
pub(super) async fn persist_after_error(
    processor: &DeliveryProcessor,
    message_id: &SpooledMessageId,
) {
    if let Some(spool) = &processor.spool
        && let Err(e) = persist_delivery_state(processor, message_id, spool).await
    {
        warn!(
            message_id = %message_id,
            error = %e,
            "Failed to persist delivery state after handling delivery error"
        );
    }
}
