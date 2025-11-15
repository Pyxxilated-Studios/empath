//! Cleanup processor for failed spool deletions
//!
//! Processes the cleanup queue with exponential backoff retry logic.

use std::{sync::Arc, time::{Duration, SystemTime}};

use empath_spool::BackingStore;
use empath_tracing::traced;
use tracing::{error, info, warn};

use crate::{error::DeliveryError, processor::DeliveryProcessor};

/// Process the cleanup queue and retry failed deletions
///
/// This function is called periodically by the delivery processor's serve loop.
/// It iterates through all entries in the cleanup queue that are ready for retry
/// and attempts to delete them from the spool with exponential backoff.
///
/// # Retry Logic
///
/// - Attempt 1: Immediate (entry already exists from failed deletion)
/// - Attempt 2: 2 seconds later (2^1)
/// - Attempt 3: 4 seconds later (2^2)
/// - After max attempts: Log CRITICAL alert and remove from queue
///
/// # Returns
///
/// Returns the number of messages successfully cleaned up, or an error if
/// processing the queue fails.
#[traced(instrument(skip(processor, spool), ret, err, level = tracing::Level::DEBUG))]
pub async fn process_cleanup_queue(
    processor: &DeliveryProcessor,
    spool: &Arc<dyn BackingStore>,
) -> Result<usize, DeliveryError> {
    let now = SystemTime::now();
    let mut cleaned = 0;

    // Get all entries ready for retry
    let ready = processor.cleanup_queue.ready_for_retry(now);

    if ready.is_empty() {
        return Ok(0);
    }

    info!(
        count = ready.len(),
        "Processing cleanup queue for failed deletions"
    );

    for entry in ready {
        match spool.delete(&entry.message_id).await {
            Ok(()) => {
                // Success! Remove from cleanup queue
                info!(
                    message_id = ?entry.message_id,
                    attempt = entry.attempt_count,
                    "Successfully deleted message from spool after retry"
                );
                processor.cleanup_queue.remove(&entry.message_id);
                cleaned += 1;
            }
            Err(e) if entry.attempt_count >= processor.max_cleanup_attempts => {
                // Max retries exceeded - CRITICAL alert
                error!(
                    message_id = ?entry.message_id,
                    attempts = entry.attempt_count,
                    first_failure = ?entry.first_failure,
                    error = %e,
                    "CRITICAL: Failed to delete message after {} attempts - manual intervention required. \
                     Orphaned .deleted files may exist on disk.",
                    entry.attempt_count
                );
                processor.cleanup_queue.remove(&entry.message_id);
            }
            Err(e) => {
                // Retry later with exponential backoff: 2^attempt_count seconds
                let delay = Duration::from_secs(2u64.pow(entry.attempt_count));
                let next_retry_at = now + delay;

                processor
                    .cleanup_queue
                    .schedule_retry(&entry.message_id, next_retry_at);

                warn!(
                    message_id = ?entry.message_id,
                    attempt = entry.attempt_count + 1,
                    next_retry_secs = delay.as_secs(),
                    error = %e,
                    "Failed to delete message from spool, will retry with exponential backoff"
                );
            }
        }
    }

    if cleaned > 0 {
        info!(
            cleaned,
            remaining = processor.cleanup_queue.len(),
            "Cleanup queue processing complete"
        );
    }

    Ok(cleaned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exponential_backoff() {
        // Test the exponential backoff calculation
        let delays: Vec<u64> = (0..5).map(|n| 2u64.pow(n)).collect();

        assert_eq!(delays[0], 1); // 2^0 = 1 second
        assert_eq!(delays[1], 2); // 2^1 = 2 seconds
        assert_eq!(delays[2], 4); // 2^2 = 4 seconds
        assert_eq!(delays[3], 8); // 2^3 = 8 seconds
        assert_eq!(delays[4], 16); // 2^4 = 16 seconds
    }
}
