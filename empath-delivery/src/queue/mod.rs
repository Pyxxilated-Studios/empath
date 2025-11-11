//! Delivery queue management

pub mod retry;

use std::{collections::HashMap, sync::Arc};

use empath_common::DeliveryStatus;
use empath_spool::SpooledMessageId;
use tokio::sync::RwLock;

use crate::{dns::MailServer, types::DeliveryInfo};

/// Manages the delivery queue for outbound messages
#[derive(Debug, Clone)]
pub struct DeliveryQueue {
    /// Map of message IDs to delivery information
    pub(crate) queue: Arc<RwLock<HashMap<SpooledMessageId, DeliveryInfo>>>,
}

impl Default for DeliveryQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl DeliveryQueue {
    /// Create a new empty delivery queue
    #[must_use]
    pub fn new() -> Self {
        Self {
            queue: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Add a message to the delivery queue
    pub async fn enqueue(&self, message_id: SpooledMessageId, recipient_domain: String) {
        let mut queue = self.queue.write().await;
        queue.insert(
            message_id.clone(),
            DeliveryInfo::new(message_id, recipient_domain),
        );
    }

    /// Get delivery info for a message
    pub async fn get(&self, message_id: &SpooledMessageId) -> Option<DeliveryInfo> {
        let queue = self.queue.read().await;
        queue.get(message_id).cloned()
    }

    /// Update the status of a message
    pub async fn update_status(&self, message_id: &SpooledMessageId, status: DeliveryStatus) {
        let mut queue = self.queue.write().await;
        if let Some(info) = queue.get_mut(message_id) {
            info.status = status;
        }
    }

    /// Record a delivery attempt
    pub async fn record_attempt(
        &self,
        message_id: &SpooledMessageId,
        attempt: empath_common::DeliveryAttempt,
    ) {
        let mut queue = self.queue.write().await;
        if let Some(info) = queue.get_mut(message_id) {
            info.record_attempt(attempt);
        }
    }

    /// Set the resolved mail servers for a message
    pub async fn set_mail_servers(
        &self,
        message_id: &SpooledMessageId,
        servers: Arc<Vec<MailServer>>,
    ) {
        let mut queue = self.queue.write().await;
        if let Some(info) = queue.get_mut(message_id) {
            info.mail_servers = servers;
            info.current_server_index = 0;
        }
    }

    /// Try the next MX server for a message.
    ///
    /// Returns `true` if there is another server to try, `false` if all exhausted.
    pub async fn try_next_server(&self, message_id: &SpooledMessageId) -> bool {
        let mut queue = self.queue.write().await;
        queue
            .get_mut(message_id)
            .is_some_and(DeliveryInfo::try_next_server)
    }

    /// Remove a message from the queue
    pub async fn remove(&self, message_id: &SpooledMessageId) -> Option<DeliveryInfo> {
        let mut queue = self.queue.write().await;
        queue.remove(message_id)
    }

    /// Set the next retry timestamp for a message
    pub async fn set_next_retry_at(&self, message_id: &SpooledMessageId, next_retry_at: u64) {
        let mut queue = self.queue.write().await;
        if let Some(info) = queue.get_mut(message_id) {
            info.next_retry_at = Some(next_retry_at);
        }
    }

    /// Reset the server index to 0 for a message (for new retry cycle)
    pub async fn reset_server_index(&self, message_id: &SpooledMessageId) {
        let mut queue = self.queue.write().await;
        if let Some(info) = queue.get_mut(message_id) {
            info.reset_server_index();
        }
    }

    /// Get all pending messages
    pub async fn pending_messages(&self) -> Vec<DeliveryInfo> {
        let queue = self.queue.read().await;
        queue
            .values()
            .filter(|info| info.status == DeliveryStatus::Pending)
            .cloned()
            .collect()
    }

    /// Get all messages with their current status
    pub async fn all_messages(&self) -> Vec<DeliveryInfo> {
        let queue = self.queue.read().await;
        queue.values().cloned().collect()
    }
}
