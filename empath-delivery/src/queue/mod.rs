//! Delivery queue management

pub mod cleanup;
pub mod retry;

use std::sync::Arc;
use std::time::SystemTime;

use dashmap::{DashMap, DashSet};
use empath_common::context::DeliveryPriorityTier;
use empath_common::{DeliveryAttempt, DeliveryStatus};
use empath_spool::SpooledMessageId;

use crate::dns::MailServer;
use crate::types::DeliveryInfo;

/// Configuration for queue scanning
#[derive(Debug, Clone, Default)]
pub struct ScanConfig {
    /// Message expiration time in seconds (None = never expire)
    pub message_expiration_secs: Option<u64>,
}

/// Result of scanning the delivery queue for pending messages
#[derive(Debug, Default)]
pub struct ScanResult {
    /// Message IDs that are ready for delivery (sorted by priority, then age)
    pub pending_ids: Vec<SpooledMessageId>,
    /// Message IDs that need their state persisted to spool
    pub to_persist_ids: Vec<SpooledMessageId>,
    /// Age of oldest pending message in seconds (for metrics)
    pub oldest_pending_age_secs: u64,
}

impl ScanResult {
    /// Create an empty scan result
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            pending_ids: Vec::new(),
            to_persist_ids: Vec::new(),
            oldest_pending_age_secs: 0,
        }
    }

    /// Returns `true` if there are no pending messages
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.pending_ids.is_empty()
    }

    /// Returns the count of pending messages
    #[must_use]
    pub const fn pending_count(&self) -> usize {
        self.pending_ids.len()
    }
}

/// Manages the delivery queue for outbound messages
#[derive(Debug, Clone)]
pub struct DeliveryQueue {
    /// Map of message IDs to delivery information (lock-free concurrent access)
    pub(crate) queue: Arc<DashMap<SpooledMessageId, DeliveryInfo>>,
    /// Set of active message IDs (Pending or Retry status) for O(1) scan optimization
    active_set: Arc<DashSet<SpooledMessageId>>,
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
            active_set: Arc::new(DashSet::new()),
        }
    }

    /// Add a message to the delivery queue
    pub fn enqueue(&self, message_id: SpooledMessageId, recipient_domain: String) {
        self.enqueue_with_priority(
            message_id,
            recipient_domain,
            DeliveryPriorityTier::default(),
        );
    }

    /// Add a message to the delivery queue with a priority tier
    pub fn enqueue_with_priority(
        &self,
        message_id: SpooledMessageId,
        recipient_domain: String,
        priority_tier: DeliveryPriorityTier,
    ) {
        // New messages start with Pending status, so add to active set
        self.active_set.insert(message_id.clone());
        self.queue.insert(
            message_id.clone(),
            DeliveryInfo::new_with_priority(message_id, recipient_domain, priority_tier),
        );
    }

    /// Insert a delivery info directly (for restoring from persisted state)
    pub fn insert(&self, message_id: SpooledMessageId, info: DeliveryInfo) {
        // Update active set based on status
        if matches!(
            info.status,
            DeliveryStatus::Pending | DeliveryStatus::Retry { .. }
        ) {
            self.active_set.insert(message_id.clone());
        } else {
            self.active_set.remove(&message_id);
        }
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
        // Update active set based on new status
        if matches!(
            status,
            DeliveryStatus::Pending | DeliveryStatus::Retry { .. }
        ) {
            self.active_set.insert(message_id.clone());
        } else {
            self.active_set.remove(message_id);
        }

        if let Some(mut entry) = self.queue.get_mut(message_id) {
            entry.value_mut().status = status;
        }
    }

    /// Record a delivery attempt
    pub fn record_attempt(&self, message_id: &SpooledMessageId, attempt: DeliveryAttempt) {
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

    /// Set the outbound binding for a message
    pub fn set_binding(&self, message_id: &SpooledMessageId, binding: Option<String>) {
        if let Some(mut entry) = self.queue.get_mut(message_id) {
            entry.value_mut().binding = binding;
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
        self.active_set.remove(message_id);
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

    /// Get an iterator over pending messages without cloning the map
    pub fn pending_messages_iter(&self) -> impl Iterator<Item = DeliveryInfo> + '_ {
        self.queue
            .iter()
            .filter(|entry| entry.value().status == DeliveryStatus::Pending)
            .map(|entry| entry.value().clone())
    }

    /// Get the active message IDs (Pending or Retry status) for O(1) scan optimization
    ///
    /// Returns an iterator over message IDs that are potentially ready for processing,
    /// avoiding iteration over completed/failed/expired/in-progress messages.
    pub fn active_message_ids(&self) -> impl Iterator<Item = SpooledMessageId> + '_ {
        self.active_set.iter().map(|entry| entry.clone())
    }

    /// Mark a message as no longer active (used by `scan_queue` when status changes internally)
    pub fn mark_inactive(&self, message_id: &SpooledMessageId) {
        self.active_set.remove(message_id);
    }

    /// Scan the queue for messages ready for delivery
    ///
    /// This method:
    /// 1. Iterates over active messages (Pending or Retry status)
    /// 2. Checks for expired messages based on config
    /// 3. Transitions Retry messages to Pending when retry time has elapsed
    /// 4. Sorts pending messages by priority tier and age
    ///
    /// # Arguments
    /// * `now` - Current time for age calculations
    /// * `config` - Scan configuration (expiration settings)
    ///
    /// # Returns
    /// A `ScanResult` containing pending message IDs, IDs needing persistence,
    /// and the oldest message age in seconds.
    pub fn scan(&self, now: SystemTime, config: &ScanConfig) -> ScanResult {
        let mut pending_messages = Vec::new();
        let mut to_persist = Vec::new();
        let mut oldest_pending_age_secs = 0u64;

        // Use active_message_ids for O(1) lookup instead of O(n) scan
        let active_ids: Vec<_> = self.active_message_ids().collect();

        for message_id in active_ids {
            let Some(mut entry) = self.queue.get_mut(&message_id) else {
                // Message was removed between collecting IDs and processing
                self.mark_inactive(&message_id);
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
                self.mark_inactive(&message_id);
                continue;
            }

            // Track oldest message age
            if let Ok(age) = now.duration_since(info.queued_at) {
                oldest_pending_age_secs = oldest_pending_age_secs.max(age.as_secs());
            }

            // Check expiration
            if Self::is_expired(info, now, config) {
                self.mark_inactive(&message_id);
                to_persist.push(info.message_id.clone());
                continue;
            }

            // Check if retry is ready
            if !Self::ready_to_retry(info, now, &mut to_persist) {
                continue;
            }

            // Collect pending messages for sorting
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

        // Sort by priority (lower rank = higher priority), then by age (older first)
        pending_messages.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));

        let pending_ids = pending_messages
            .into_iter()
            .map(|(_, _, message_id)| message_id)
            .collect();

        ScanResult {
            pending_ids,
            to_persist_ids: to_persist,
            oldest_pending_age_secs,
        }
    }

    /// Check if a message has expired based on configuration
    ///
    /// If expired, updates the message status to `Expired` and returns `true`.
    fn is_expired(info: &mut DeliveryInfo, now: SystemTime, config: &ScanConfig) -> bool {
        let Some(expiration_secs) = config.message_expiration_secs else {
            return false;
        };

        let age_secs = now
            .duration_since(info.queued_at)
            .unwrap_or_default()
            .as_secs();

        if age_secs <= expiration_secs {
            return false;
        }

        empath_common::tracing::warn!(
            message_id = ?info.message_id,
            age_secs = age_secs,
            expiration_secs = expiration_secs,
            "Message expired, marking as Expired"
        );

        info.status = DeliveryStatus::Expired;
        true
    }

    /// Check if a Retry message is ready for another attempt
    ///
    /// If ready, transitions status to Pending and returns `true`.
    /// Adds `message_id` to `to_persist` when status changes.
    fn ready_to_retry(
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
            empath_common::tracing::debug!(
                message_id = ?info.message_id,
                wait_secs = wait_secs,
                "Skipping message, not yet time to retry"
            );
            return false;
        }

        empath_common::tracing::debug!(
            message_id = ?info.message_id,
            attempt = info.attempt_count(),
            "Time to retry delivery"
        );
        info.status = DeliveryStatus::Pending;
        info.reset_server_index();
        to_persist.push(info.message_id.clone());
        true
    }

    /// Apply a circuit breaker gate decision to a message
    ///
    /// Updates the message status and next retry time based on the decision.
    /// Returns `true` if the message was blocked (and state was updated).
    pub fn apply(
        &self,
        message_id: &SpooledMessageId,
        decision: &crate::circuit_breaker::GateDecision,
    ) -> bool {
        match decision {
            crate::circuit_breaker::GateDecision::Allow => false,
            crate::circuit_breaker::GateDecision::Block {
                next_retry_at,
                attempt_count,
            } => {
                self.update_status(
                    message_id,
                    DeliveryStatus::Retry {
                        attempts: *attempt_count,
                        last_error: "Circuit breaker open".to_string(),
                    },
                );
                self.set_next_retry_at(message_id, *next_retry_at);
                true
            }
        }
    }
}
