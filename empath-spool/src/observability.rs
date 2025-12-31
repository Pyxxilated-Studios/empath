use std::sync::Arc;
use std::time::Duration;

use empath_metrics::metrics;
use tracing::debug;

use crate::SpooledMessageId;

pub trait SpoolObserver: Send + Sync + std::fmt::Debug {
    fn record_io(
        &self,
        operation: &'static str,
        bytes: u64,
        duration: Duration,
        backend: &'static str,
        message_id: Option<&SpooledMessageId>,
    );
}

#[derive(Debug, Default)]
pub struct DefaultSpoolObserver;

impl SpoolObserver for DefaultSpoolObserver {
    fn record_io(
        &self,
        operation: &'static str,
        bytes: u64,
        duration: Duration,
        backend: &'static str,
        message_id: Option<&SpooledMessageId>,
    ) {
        if let Some(metrics) = metrics() {
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
}

#[must_use]
pub fn default_spool_observer() -> Arc<dyn SpoolObserver> {
    Arc::new(DefaultSpoolObserver)
}
