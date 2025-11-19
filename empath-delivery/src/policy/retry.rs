//! Retry policy for delivery operations.
//!
//! This module provides a clean abstraction over retry configuration and logic,
//! making it easy to test and reason about retry behavior independently of the
//! delivery processor.

use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::queue::retry::calculate_next_retry_time;

/// Retry policy configuration for delivery operations.
///
/// Encapsulates all retry-related configuration and provides methods for
/// determining retry behavior without exposing implementation details.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryPolicy {
    /// Maximum number of delivery attempts before giving up.
    ///
    /// Default: 25 attempts
    #[serde(default = "defaults::max_attempts")]
    pub max_attempts: u32,

    /// Base delay for exponential backoff (in seconds).
    ///
    /// The actual delay is calculated as: `base * 2^(attempts - 1)`
    ///
    /// Default: 300 seconds (5 minutes)
    #[serde(default = "defaults::base_retry_delay_secs")]
    pub base_retry_delay_secs: u64,

    /// Maximum retry delay (in seconds).
    ///
    /// Caps the exponential backoff to prevent excessively long delays.
    ///
    /// Default: 86400 seconds (24 hours)
    #[serde(default = "defaults::max_retry_delay_secs")]
    pub max_retry_delay_secs: u64,

    /// Jitter factor for randomizing retry delays.
    ///
    /// Jitter prevents thundering herd problems when many messages
    /// retry simultaneously. The delay is randomized within ±`jitter_factor`.
    ///
    /// Default: 0.1 (±10%)
    #[serde(default = "defaults::retry_jitter_factor")]
    pub retry_jitter_factor: f64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: defaults::max_attempts(),
            base_retry_delay_secs: defaults::base_retry_delay_secs(),
            max_retry_delay_secs: defaults::max_retry_delay_secs(),
            retry_jitter_factor: defaults::retry_jitter_factor(),
        }
    }
}

impl RetryPolicy {
    /// Create a new retry policy with default values.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Check if another retry should be attempted based on attempt count.
    ///
    /// Returns `true` if the number of attempts is less than `max_attempts`.
    #[must_use]
    pub const fn should_retry(&self, attempt_count: u32) -> bool {
        attempt_count < self.max_attempts
    }

    /// Calculate when the next retry should occur.
    ///
    /// Uses exponential backoff with jitter to determine the retry time.
    ///
    /// # Arguments
    /// * `attempt_count` - Number of attempts made so far (0-indexed in practice,
    ///   but calculation treats as 1-indexed internally)
    ///
    /// # Returns
    /// `SystemTime` when the next retry should be attempted
    #[must_use]
    pub fn calculate_next_retry(&self, attempt_count: u32) -> SystemTime {
        // Add 1 because the retry calculation expects 1-indexed attempts
        let attempt = attempt_count + 1;
        calculate_next_retry_time(
            attempt,
            self.base_retry_delay_secs,
            self.max_retry_delay_secs,
            self.retry_jitter_factor,
        )
    }

    /// Get the number of remaining retry attempts.
    ///
    /// Returns `0` if max attempts has been reached.
    #[must_use]
    pub const fn remaining_attempts(&self, attempt_count: u32) -> u32 {
        self.max_attempts.saturating_sub(attempt_count)
    }

    /// Check if this is the final retry attempt.
    #[must_use]
    pub const fn is_final_attempt(&self, attempt_count: u32) -> bool {
        attempt_count + 1 >= self.max_attempts
    }
}

mod defaults {
    pub const fn max_attempts() -> u32 {
        25
    }

    pub const fn base_retry_delay_secs() -> u64 {
        300 // 5 minutes
    }

    pub const fn max_retry_delay_secs() -> u64 {
        86400 // 24 hours
    }

    pub const fn retry_jitter_factor() -> f64 {
        0.1 // ±10%
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn test_retry_policy_defaults() {
        let policy = RetryPolicy::default();
        assert_eq!(policy.max_attempts, 25);
        assert_eq!(policy.base_retry_delay_secs, 300);
        assert_eq!(policy.max_retry_delay_secs, 86400);
        assert!((policy.retry_jitter_factor - 0.1).abs() < f64::EPSILON);
    }

    #[test]
    fn test_should_retry() {
        let policy = RetryPolicy::default();

        // Should retry on first few attempts
        assert!(policy.should_retry(0));
        assert!(policy.should_retry(1));
        assert!(policy.should_retry(10));
        assert!(policy.should_retry(24));

        // Should not retry after max attempts
        assert!(!policy.should_retry(25));
        assert!(!policy.should_retry(26));
        assert!(!policy.should_retry(100));
    }

    #[test]
    fn test_remaining_attempts() {
        let policy = RetryPolicy::default();

        assert_eq!(policy.remaining_attempts(0), 25);
        assert_eq!(policy.remaining_attempts(1), 24);
        assert_eq!(policy.remaining_attempts(10), 15);
        assert_eq!(policy.remaining_attempts(24), 1);
        assert_eq!(policy.remaining_attempts(25), 0);
        assert_eq!(policy.remaining_attempts(30), 0); // Saturating
    }

    #[test]
    fn test_is_final_attempt() {
        let policy = RetryPolicy::default();

        assert!(!policy.is_final_attempt(0));
        assert!(!policy.is_final_attempt(10));
        assert!(!policy.is_final_attempt(23));
        assert!(policy.is_final_attempt(24)); // Last attempt
        assert!(policy.is_final_attempt(25)); // Already past max
    }

    #[test]
    #[cfg_attr(miri, ignore = "Calls an unsupported method")]
    fn test_calculate_next_retry() {
        let policy = RetryPolicy {
            max_attempts: 25,
            base_retry_delay_secs: 60,
            max_retry_delay_secs: 86400,
            retry_jitter_factor: 0.0, // No jitter for predictable testing
        };

        let now = SystemTime::now();

        // First retry (attempt 0 -> calculation uses 1)
        let next = policy.calculate_next_retry(0);
        let delay = next
            .duration_since(now)
            .expect("next retry should be in future")
            .as_secs();
        assert_eq!(delay, 60);

        // Second retry (attempt 1 -> calculation uses 2)
        let now = SystemTime::now();
        let next = policy.calculate_next_retry(1);
        let delay = next
            .duration_since(now)
            .expect("next retry should be in future")
            .as_secs();
        assert_eq!(delay, 120);

        // Third retry (attempt 2 -> calculation uses 3)
        let now = SystemTime::now();
        let next = policy.calculate_next_retry(2);
        let delay = next
            .duration_since(now)
            .expect("next retry should be in future")
            .as_secs();
        assert_eq!(delay, 240);
    }

    #[test]
    fn test_custom_retry_policy() {
        let policy = RetryPolicy {
            max_attempts: 5,
            base_retry_delay_secs: 10,
            max_retry_delay_secs: 100,
            retry_jitter_factor: 0.0,
        };

        assert!(policy.should_retry(0));
        assert!(policy.should_retry(4));
        assert!(!policy.should_retry(5));

        assert_eq!(policy.remaining_attempts(2), 3);
    }
}
