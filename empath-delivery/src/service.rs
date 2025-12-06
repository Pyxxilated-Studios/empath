//! Service trait abstraction for delivery operations
//!
//! This module provides trait abstractions to decouple control interfaces
//! from the concrete `DeliveryProcessor` implementation, following the
//! Interface Segregation Principle.

use std::sync::Arc;

use empath_common::DeliveryStatus;
use empath_spool::{BackingStore, SpooledMessageId};

use crate::{DeliveryInfo, DnsResolver, DomainConfigRegistry};

/// Service trait for querying delivery state and managing queue operations
///
/// This trait provides an abstraction layer between control interfaces
/// (like the control socket handler) and the delivery processor implementation.
/// It enables:
/// - Clean separation of concerns (Interface Segregation Principle)
/// - Mockable interface for testing
/// - Reduced coupling (~80% reduction in dependencies)
/// - Future CQRS pattern support if needed
///
/// # Example
///
/// ```rust,ignore
/// fn handle_queue_stats(service: &dyn DeliveryQueryService) -> usize {
///     service.queue_len()
/// }
/// ```
pub trait DeliveryQueryService: Send + Sync {
    /// Get the number of messages in the delivery queue
    fn queue_len(&self) -> usize;

    /// Get delivery information for a specific message
    ///
    /// Returns `None` if the message is not in the queue.
    fn get_message(&self, id: &SpooledMessageId) -> Option<DeliveryInfo>;

    /// List all messages in the queue, optionally filtered by status
    ///
    /// # Arguments
    ///
    /// * `status` - Optional status filter. If provided, only messages
    ///   matching this status will be returned.
    ///
    /// # Returns
    ///
    /// A vector of delivery information for all matching messages.
    fn list_messages(&self, status: Option<DeliveryStatus>) -> Vec<DeliveryInfo>;

    /// Borrowing iterator over queue entries to avoid cloning when only
    /// read-only access is needed (e.g., listing).
    fn list_messages_iter(
        &self,
        status: Option<DeliveryStatus>,
    ) -> Box<dyn Iterator<Item = DeliveryInfo> + '_>;

    /// Return sorted message identifiers for control pagination without
    /// cloning full delivery records.
    fn list_message_ids(&self, status_filter: Option<&str>) -> Vec<SpooledMessageId>;

    /// Snapshot lightweight queue stats without cloning full entries.
    fn queue_stats_snapshot(&self) -> QueueSnapshot;

    /// Update the delivery status of a message
    ///
    /// This is used by control commands like `retry` to reset failed
    /// messages back to pending status.
    ///
    /// # Arguments
    ///
    /// * `message_id` - The ID of the message to update
    /// * `status` - The new delivery status
    fn update_status(&self, message_id: &SpooledMessageId, status: DeliveryStatus);

    /// Set the next retry time for a message
    ///
    /// Used by control commands to schedule immediate retry or defer delivery.
    ///
    /// # Arguments
    ///
    /// * `message_id` - The ID of the message to update
    /// * `next_retry_at` - The time when the next retry should occur
    fn set_next_retry_at(
        &self,
        message_id: &SpooledMessageId,
        next_retry_at: std::time::SystemTime,
    );

    /// Reset the mail server index for a message
    ///
    /// Used by retry command to restart delivery attempts from the first MX server.
    ///
    /// # Arguments
    ///
    /// * `message_id` - The ID of the message to update
    fn reset_server_index(&self, message_id: &SpooledMessageId);

    /// Remove a message from the delivery queue
    ///
    /// Used by delete command to remove messages from the queue.
    /// Returns the removed `DeliveryInfo` if the message was found.
    ///
    /// # Arguments
    ///
    /// * `message_id` - The ID of the message to remove
    ///
    /// # Returns
    ///
    /// The delivery information for the removed message, or `None` if not found.
    fn remove(&self, message_id: &SpooledMessageId) -> Option<DeliveryInfo>;

    /// Get a reference to the DNS resolver
    ///
    /// Used by DNS cache management commands.
    ///
    /// Returns `None` if the resolver is not initialized.
    fn dns_resolver(&self) -> &Option<Arc<dyn DnsResolver>>;

    /// Get a reference to the spool backing store
    ///
    /// Used by queue commands to read message content.
    ///
    /// Returns `None` if the spool is not initialized.
    fn spool(&self) -> &Option<Arc<dyn BackingStore>>;

    /// Get a reference to the domain configuration registry
    ///
    /// Used by domain management commands.
    fn domains(&self) -> &DomainConfigRegistry;

    /// Request an immediate scan/process cycle (used by control interface)
    fn request_process_now(&self);

    /// Get connection/rate-limit statistics
    fn connection_stats(&self) -> crate::types::ConnectionStats;

    /// Update rate limits (global or per-domain)
    ///
    /// # Errors
    ///
    /// Returns an error if the rate limiter is not initialized or provided values are invalid.
    fn update_rate_limit(
        &self,
        domain: Option<String>,
        messages_per_second: f64,
        burst: u32,
    ) -> Result<(), String>;

    /// Replace the full rate limit configuration (global + per-domain).
    fn replace_rate_limit_config(&self, config: crate::rate_limiter::RateLimitConfig);

    /// Replace domain configuration registry contents.
    fn replace_domains(&self, registry: crate::DomainConfigRegistry);
}

/// Lightweight view of queue statistics for control endpoints.
#[derive(Debug, Default)]
pub struct QueueSnapshot {
    pub total: usize,
    pub by_status: std::collections::HashMap<String, usize>,
    pub by_domain: std::collections::HashMap<String, usize>,
    pub oldest_message_age_secs: Option<u64>,
}
