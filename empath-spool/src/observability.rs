use std::time::Duration;

use empath_metrics::try_metrics;
use tracing::debug;

use crate::SpooledMessageId;

pub fn record_spool_io(
    operation: &'static str,
    bytes: u64,
    duration: Duration,
    backend: &'static str,
    message_id: Option<&SpooledMessageId>,
) {
    if let Some(metrics) = try_metrics() {
        metrics.system.record_spool_io(operation, bytes, duration);
    }

    let duration_ms = u64::try_from(duration.as_millis()).unwrap_or(u64::MAX);
    debug!(
        operation,
        bytes,
        duration_ms,
        backend,
        message_id = message_id.map(ToString::to_string),
        "Spool I/O completed"
    );
}
