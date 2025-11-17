//! Cleanup queue for failed spool deletions
//!
//! When spool deletion fails after successful delivery, messages are added to
//! this cleanup queue for retry with exponential backoff. This prevents disk
//! exhaustion from accumulated .deleted files.

use std::time::SystemTime;

use dashmap::DashMap;
use empath_spool::SpooledMessageId;

/// Cleanup entry tracking a failed deletion
#[derive(Debug, Clone)]
pub struct CleanupEntry {
    /// Message ID that failed to delete
    pub message_id: SpooledMessageId,
    /// Number of deletion attempts
    pub attempt_count: u32,
    /// When to retry next
    pub next_retry_at: SystemTime,
    /// When the first deletion failed
    pub first_failure: SystemTime,
}

/// Queue for managing failed spool deletions with retry logic
#[derive(Debug, Clone, Default)]
pub struct CleanupQueue {
    /// Map of message IDs to cleanup entries (lock-free concurrent access)
    pub(crate) entries: DashMap<SpooledMessageId, CleanupEntry>,
}

impl CleanupQueue {
    /// Create a new empty cleanup queue
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries: DashMap::new(),
        }
    }

    /// Add a failed deletion to the cleanup queue
    ///
    /// This should be called when `spool.delete()` fails after successful delivery.
    pub fn add_failed_deletion(&self, message_id: SpooledMessageId) {
        let now = SystemTime::now();

        self.entries.insert(
            message_id.clone(),
            CleanupEntry {
                message_id,
                attempt_count: 1,   // First failure
                next_retry_at: now, // Retry immediately
                first_failure: now,
            },
        );
    }

    /// Get entries that are ready for retry
    ///
    /// Returns entries where `next_retry_at` is in the past.
    pub fn ready_for_retry(&self, now: SystemTime) -> Vec<CleanupEntry> {
        self.entries
            .iter()
            .filter(|entry| entry.value().next_retry_at <= now)
            .map(|entry| entry.value().clone())
            .collect()
    }

    /// Schedule a retry for a message with exponential backoff
    ///
    /// Increments attempt count and sets next retry time.
    pub fn schedule_retry(&self, message_id: &SpooledMessageId, next_retry_at: SystemTime) {
        if let Some(mut entry) = self.entries.get_mut(message_id) {
            entry.value_mut().attempt_count += 1;
            entry.value_mut().next_retry_at = next_retry_at;
        }
    }

    /// Remove a message from the cleanup queue
    ///
    /// Called after successful deletion or max retries exceeded.
    pub fn remove(&self, message_id: &SpooledMessageId) {
        self.entries.remove(message_id);
    }

    /// Get the number of messages pending cleanup
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    #[cfg_attr(miri, ignore = "Calls an unsupported method")]
    fn test_add_failed_deletion() {
        let queue = CleanupQueue::new();
        let message_id = SpooledMessageId::generate();

        queue.add_failed_deletion(message_id.clone());

        assert_eq!(queue.len(), 1);
        let entries = queue.ready_for_retry(SystemTime::now());
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].message_id, message_id);
        assert_eq!(entries[0].attempt_count, 1);
    }

    #[test]
    #[cfg_attr(miri, ignore = "Calls an unsupported method")]
    fn test_ready_for_retry() {
        let queue = CleanupQueue::new();
        let message_id = SpooledMessageId::generate();

        queue.add_failed_deletion(message_id.clone());

        // Should be ready immediately (first attempt)
        let now = SystemTime::now();
        let ready = queue.ready_for_retry(now);
        assert_eq!(ready.len(), 1);

        // Schedule retry for 10 seconds in the future
        let future = now + Duration::from_secs(10);
        queue.schedule_retry(&message_id, future);

        // Should not be ready yet
        let ready = queue.ready_for_retry(now);
        assert_eq!(ready.len(), 0);

        // Should be ready after the delay
        let ready = queue.ready_for_retry(future + Duration::from_secs(1));
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].attempt_count, 2); // Incremented by schedule_retry
    }

    #[test]
    #[cfg_attr(miri, ignore = "Calls an unsupported method")]
    fn test_remove() {
        let queue = CleanupQueue::new();
        let message_id = SpooledMessageId::generate();

        queue.add_failed_deletion(message_id.clone());
        assert_eq!(queue.len(), 1);

        queue.remove(&message_id);
        assert_eq!(queue.len(), 0);
        assert!(queue.entries.is_empty());
    }

    #[test]
    fn test_exponential_backoff_calculation() {
        // Test the exponential backoff calculation (2^n seconds)
        let delays: Vec<u64> = (0..5).map(|n| 2u64.pow(n)).collect();

        assert_eq!(delays[0], 1); // 2^0 = 1 second
        assert_eq!(delays[1], 2); // 2^1 = 2 seconds
        assert_eq!(delays[2], 4); // 2^2 = 4 seconds
        assert_eq!(delays[3], 8); // 2^3 = 8 seconds
        assert_eq!(delays[4], 16); // 2^4 = 16 seconds
    }
}
