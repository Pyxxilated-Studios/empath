//! Spool scanning logic for discovering new messages

use std::sync::Arc;

use empath_common::tracing::warn;

use crate::{
    error::{DeliveryError, SystemError},
    processor::DeliveryProcessor,
    types::DeliveryInfo,
};

/// Scan the spool for new messages and add them to the queue
///
/// # Errors
/// Returns an error if the spool cannot be read
pub async fn scan_spool_internal(
    processor: &DeliveryProcessor,
    spool: &Arc<dyn empath_spool::BackingStore>,
) -> Result<usize, DeliveryError> {
    let message_ids = spool
        .list()
        .await
        .map_err(|e| SystemError::SpoolRead(e.to_string()))?;
    let mut added = 0;

    for msg_id in message_ids {
        // Check if already in queue
        if processor.queue.get(&msg_id).is_some() {
            continue;
        }

        // Read the message to get context (potentially with delivery state)
        let context = spool
            .read(&msg_id)
            .await
            .map_err(|e| SystemError::SpoolRead(e.to_string()))?;

        // Check if this message already has delivery state persisted
        if let Some(delivery_ctx) = &context.delivery {
            // Restore from persisted state
            let info = DeliveryInfo {
                message_id: msg_id.clone(),
                status: delivery_ctx.status.clone(),
                attempts: delivery_ctx.attempt_history.clone(),
                recipient_domain: delivery_ctx.domain.clone(),
                mail_servers: Arc::new(Vec::new()), // Will be resolved again if needed
                current_server_index: delivery_ctx.current_server_index,
                queued_at: delivery_ctx.queued_at,
                next_retry_at: delivery_ctx.next_retry_at,
            };

            // Add to queue with existing state
            processor.queue.insert(msg_id.clone(), info);
            added += 1;
            continue;
        }

        // New message without delivery state - create fresh DeliveryInfo
        // Group recipients by domain (handle multi-recipient messages)
        let Some(recipients) = context.envelope.recipients() else {
            warn!("Message {:?} has no recipients, skipping", msg_id);
            continue;
        };

        // Collect unique domains from all recipients
        let mut domains = std::collections::HashMap::new();
        for recipient in recipients.iter() {
            // Extract the domain from the Mailbox
            let domain = recipient.domain.clone();
            let recipient_str = recipient.to_string();

            domains
                .entry(domain)
                .or_insert_with(Vec::new)
                .push(recipient_str);
        }

        // Enqueue for each unique domain
        for (domain, _recipients) in domains {
            processor.queue.enqueue(msg_id.clone(), domain);
            added += 1;
        }
    }

    Ok(added)
}
