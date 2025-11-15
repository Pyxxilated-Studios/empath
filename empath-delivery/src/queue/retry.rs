//! Retry logic with exponential backoff

use std::time::{Duration, SystemTime};

use rand::Rng;

/// Calculate the next retry time using exponential backoff with jitter
///
/// # Formula
/// `delay = min(base * 2^(attempts - 1), max_delay) * (1 ± jitter)`
///
/// # Arguments
/// * `attempt` - The attempt number (1-indexed)
/// * `base_delay_secs` - Base delay in seconds (e.g., 60 for 1 minute)
/// * `max_delay_secs` - Maximum delay in seconds (e.g., 86400 for 24 hours)
/// * `jitter_factor` - Jitter factor (e.g., 0.2 for ±20%)
///
/// # Returns
/// `SystemTime` when the next retry should occur
pub fn calculate_next_retry_time(
    attempt: u32,
    base_delay_secs: u64,
    max_delay_secs: u64,
    jitter_factor: f64,
) -> SystemTime {
    // Calculate exponential backoff: base * 2^(attempts - 1)
    // Use saturating operations to prevent overflow
    let exponent = attempt.saturating_sub(1);
    let delay = if exponent >= 63 {
        // 2^63 would overflow, use max_delay directly
        max_delay_secs
    } else {
        let multiplier = 1u64 << exponent; // 2^exponent
        base_delay_secs
            .saturating_mul(multiplier)
            .min(max_delay_secs)
    };

    // Apply jitter: delay * (1 ± jitter_factor)
    // Intentional precision loss and casting for randomization
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )]
    let jittered_delay = {
        let jitter_range = (delay as f64) * jitter_factor;
        let mut rng = rand::rng();
        let jitter: f64 = rng.random_range(-jitter_range..=jitter_range);
        ((delay as f64) + jitter).max(0.0) as u64
    };

    // Calculate next retry timestamp
    SystemTime::now() + Duration::from_secs(jittered_delay)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg_attr(miri, ignore = "Calls an unsupported method")]
    fn test_exponential_backoff_calculation() {
        // Test exponential backoff with base=60s, max=86400s, jitter=0
        // We'll test with jitter=0 for predictable results
        let base_delay = 60;
        let max_delay = 86400;
        let jitter_factor = 0.0; // No jitter for testing

        // Attempt 1: 60 * 2^0 = 60 seconds
        let now = SystemTime::now();
        let next_retry = calculate_next_retry_time(1, base_delay, max_delay, jitter_factor);
        let delay = next_retry.duration_since(now).unwrap_or_default().as_secs();
        assert_eq!(delay, 60, "First retry should be 60 seconds");

        // Attempt 2: 60 * 2^1 = 120 seconds
        let now = SystemTime::now();
        let next_retry = calculate_next_retry_time(2, base_delay, max_delay, jitter_factor);
        let delay = next_retry.duration_since(now).unwrap_or_default().as_secs();
        assert_eq!(delay, 120, "Second retry should be 120 seconds");

        // Attempt 3: 60 * 2^2 = 240 seconds
        let now = SystemTime::now();
        let next_retry = calculate_next_retry_time(3, base_delay, max_delay, jitter_factor);
        let delay = next_retry.duration_since(now).unwrap_or_default().as_secs();
        assert_eq!(delay, 240, "Third retry should be 240 seconds");

        // Attempt 20: Should be capped at max_delay (86400 seconds = 24 hours)
        let now = SystemTime::now();
        let next_retry = calculate_next_retry_time(20, base_delay, max_delay, jitter_factor);
        let delay = next_retry.duration_since(now).unwrap_or_default().as_secs();
        assert_eq!(
            delay, max_delay,
            "High attempt number should be capped at max_delay"
        );
    }

    #[test]
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_sign_loss,
        clippy::cast_possible_truncation
    )]
    fn test_exponential_backoff_with_jitter() {
        // Test that jitter is applied (result should be different from exact calculation)
        let base_delay = 60;
        let max_delay = 86400;
        let jitter_factor = 0.2; // ±20%

        // Attempt 2: Expected = 120 seconds, with ±20% jitter = 96-144 seconds
        let now = SystemTime::now();
        let next_retry = calculate_next_retry_time(2, base_delay, max_delay, jitter_factor);
        let delay = next_retry.duration_since(now).unwrap_or_default().as_secs();

        // Check that delay is within jitter range
        let expected = 120;
        let min = expected - (expected as f64 * jitter_factor) as u64;
        let max = expected + (expected as f64 * jitter_factor) as u64;
        assert!(
            delay >= min && delay <= max,
            "Delay {delay} should be within jitter range [{min}, {max}]"
        );
    }
}
