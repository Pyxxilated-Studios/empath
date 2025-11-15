//! Delivery queue management

pub mod retry;

use std::{sync::Arc, time::SystemTime};

use dashmap::DashMap;
use empath_common::DeliveryStatus;
use empath_spool::SpooledMessageId;
use empath_tracing::traced;

use crate::{dns::MailServer, types::DeliveryInfo};

/// Manages the delivery queue for outbound messages
#[derive(Debug, Clone)]
pub struct DeliveryQueue {
    /// Map of message IDs to delivery information (lock-free concurrent access)
    pub(crate) queue: Arc<DashMap<SpooledMessageId, DeliveryInfo>>,
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
            queue: Arc::new(DashMap::new()),
        }
    }

    /// Add a message to the delivery queue
    pub fn enqueue(&self, message_id: SpooledMessageId, recipient_domain: String) {
        self.queue.insert(
            message_id.clone(),
            DeliveryInfo::new(message_id, recipient_domain),
        );
    }

    /// Insert a delivery info directly (for restoring from persisted state)
    pub fn insert(&self, message_id: SpooledMessageId, info: DeliveryInfo) {
        self.queue.insert(message_id, info);
    }

    /// Get delivery info for a message
    pub fn get(&self, message_id: &SpooledMessageId) -> Option<DeliveryInfo> {
        self.queue
            .get(message_id)
            .map(|entry| entry.value().clone())
    }

    /// Update the status of a message
    pub fn update_status(&self, message_id: &SpooledMessageId, status: DeliveryStatus) {
        if let Some(mut entry) = self.queue.get_mut(message_id) {
            entry.value_mut().status = status;
        }
    }

    /// Record a delivery attempt
    pub fn record_attempt(
        &self,
        message_id: &SpooledMessageId,
        attempt: empath_common::DeliveryAttempt,
    ) {
        if let Some(mut entry) = self.queue.get_mut(message_id) {
            entry.value_mut().record_attempt(attempt);
        }
    }

    /// Set the resolved mail servers for a message
    pub fn set_mail_servers(&self, message_id: &SpooledMessageId, servers: Arc<Vec<MailServer>>) {
        if let Some(mut entry) = self.queue.get_mut(message_id) {
            let info = entry.value_mut();
            info.mail_servers = servers;
            info.current_server_index = 0;
        }
    }

    /// Try the next MX server for a message.
    ///
    /// Returns `true` if there is another server to try, `false` if all exhausted.
    pub fn try_next_server(&self, message_id: &SpooledMessageId) -> bool {
        self.queue
            .get_mut(message_id)
            .is_some_and(|mut entry| entry.value_mut().try_next_server())
    }

    /// Remove a message from the queue
    pub fn remove(&self, message_id: &SpooledMessageId) -> Option<DeliveryInfo> {
        self.queue.remove(message_id).map(|(_, info)| info)
    }

    /// Get the number of messages in the queue (for control interface)
    pub fn len(&self) -> usize {
        self.queue.len()
    }

    /// Check if the queue is empty (for control interface)
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// Set the next retry timestamp for a message
    pub fn set_next_retry_at(&self, message_id: &SpooledMessageId, next_retry_at: SystemTime) {
        if let Some(mut entry) = self.queue.get_mut(message_id) {
            entry.value_mut().next_retry_at = Some(next_retry_at);
        }
    }

    /// Reset the server index to 0 for a message (for new retry cycle)
    pub fn reset_server_index(&self, message_id: &SpooledMessageId) {
        if let Some(mut entry) = self.queue.get_mut(message_id) {
            entry.value_mut().reset_server_index();
        }
    }

    /// Get all pending messages
    pub fn pending_messages(&self) -> Vec<DeliveryInfo> {
        self.queue
            .iter()
            .filter(|entry| entry.value().status == DeliveryStatus::Pending)
            .map(|entry| entry.value().clone())
            .collect()
    }

    /// Get all messages with their current status
    #[traced(instrument(ret, level = tracing::Level::TRACE))]
    pub fn all_messages(&self) -> Vec<DeliveryInfo> {
        self.queue
            .iter()
            .map(|entry| entry.value().clone())
            .collect()
    }
}
