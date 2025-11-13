//! Delivery processor metrics
//!
//! Tracks outbound mail delivery including:
//! - Delivery attempts by status (success/failure)
//! - Delivery durations by domain
//! - Queue sizes by status
//! - Active SMTP client connections

use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use opentelemetry::{
    KeyValue,
    metrics::{Counter, Histogram, Meter, UpDownCounter},
};

use crate::MetricsError;

/// Delivery metrics collector
#[derive(Debug)]
pub struct DeliveryMetrics {
    /// Total number of delivery attempts by status
    attempts_total: Counter<u64>,

    /// Distribution of delivery durations by domain
    duration_seconds: Histogram<f64>,

    /// Number of currently active outbound SMTP connections
    active_connections: UpDownCounter<i64>,

    /// Total number of messages delivered successfully
    messages_delivered: Counter<u64>,

    /// Total number of messages permanently failed
    messages_failed: Counter<u64>,

    /// Total number of messages retrying
    messages_retrying: Counter<u64>,

    /// Distribution of retry counts before success
    retry_count: Histogram<u64>,

    // Local counters for queue size tracking (shared with observable gauge callback)
    queue_pending: Arc<AtomicU64>,
    queue_in_progress: Arc<AtomicU64>,
    queue_completed: Arc<AtomicU64>,
    queue_failed: Arc<AtomicU64>,
    queue_retry: Arc<AtomicU64>,
    queue_expired: Arc<AtomicU64>,
    active_conn_count: AtomicU64,
}

impl DeliveryMetrics {
    /// Create a new delivery metrics collector
    ///
    /// # Errors
    ///
    /// Returns an error if metric instruments cannot be created.
    pub fn new() -> Result<Self, MetricsError> {
        let meter = meter();

        let attempts_total = meter
            .u64_counter("empath.delivery.attempts.total")
            .with_description("Total number of delivery attempts by status")
            .build();

        let duration_seconds = meter
            .f64_histogram("empath.delivery.duration.seconds")
            .with_description("Distribution of delivery durations by domain")
            .build();

        let messages_delivered = meter
            .u64_counter("empath.delivery.messages.delivered.total")
            .with_description("Total number of messages delivered successfully")
            .build();

        let messages_failed = meter
            .u64_counter("empath.delivery.messages.failed.total")
            .with_description("Total number of messages permanently failed")
            .build();

        let messages_retrying = meter
            .u64_counter("empath.delivery.messages.retrying.total")
            .with_description("Total number of messages retrying")
            .build();

        let retry_count = meter
            .u64_histogram("empath.delivery.retry.count")
            .with_description("Distribution of retry counts before success")
            .build();

        let active_connections = meter
            .i64_up_down_counter("empath.delivery.connections.active")
            .with_description("Number of currently active outbound SMTP connections")
            .build();

        // Create atomic counters for queue size tracking (wrapped in Arc for sharing)
        let queue_pending_ref = Arc::new(AtomicU64::new(0));
        let queue_in_progress_ref = Arc::new(AtomicU64::new(0));
        let queue_completed_ref = Arc::new(AtomicU64::new(0));
        let queue_failed_ref = Arc::new(AtomicU64::new(0));
        let queue_retry_ref = Arc::new(AtomicU64::new(0));
        let queue_expired_ref = Arc::new(AtomicU64::new(0));

        // Observable gauge that reads from atomic counters
        let pending = queue_pending_ref.clone();
        let in_progress = queue_in_progress_ref.clone();
        let completed = queue_completed_ref.clone();
        let failed = queue_failed_ref.clone();
        let retry = queue_retry_ref.clone();
        let expired = queue_expired_ref.clone();

        // Register observable gauge for queue size metrics
        // The meter keeps this alive internally via the callback
        meter
            .u64_observable_gauge("empath.delivery.queue.size")
            .with_description("Current queue size by status")
            .with_callback(move |observer| {
                observer.observe(
                    pending.load(Ordering::Relaxed),
                    &[KeyValue::new("status", "pending")],
                );
                observer.observe(
                    in_progress.load(Ordering::Relaxed),
                    &[KeyValue::new("status", "in_progress")],
                );
                observer.observe(
                    completed.load(Ordering::Relaxed),
                    &[KeyValue::new("status", "completed")],
                );
                observer.observe(
                    failed.load(Ordering::Relaxed),
                    &[KeyValue::new("status", "failed")],
                );
                observer.observe(
                    retry.load(Ordering::Relaxed),
                    &[KeyValue::new("status", "retry")],
                );
                observer.observe(
                    expired.load(Ordering::Relaxed),
                    &[KeyValue::new("status", "expired")],
                );
            })
            .build();

        Ok(Self {
            attempts_total,
            duration_seconds,
            active_connections,
            messages_delivered,
            messages_failed,
            messages_retrying,
            retry_count,
            queue_pending: queue_pending_ref,
            queue_in_progress: queue_in_progress_ref,
            queue_completed: queue_completed_ref,
            queue_failed: queue_failed_ref,
            queue_retry: queue_retry_ref,
            queue_expired: queue_expired_ref,
            active_conn_count: AtomicU64::new(0),
        })
    }

    /// Record a delivery attempt
    pub fn record_attempt(&self, status: &str, domain: &str) {
        let attributes = [
            KeyValue::new("status", status.to_string()),
            KeyValue::new("domain", domain.to_string()),
        ];
        self.attempts_total.add(1, &attributes);
    }

    /// Record a successful delivery
    pub fn record_delivery_success(&self, domain: &str, duration_secs: f64, retry_count: u64) {
        let attributes = [KeyValue::new("domain", domain.to_string())];
        self.duration_seconds.record(duration_secs, &attributes);
        self.messages_delivered.add(1, &[]);
        self.retry_count.record(retry_count, &[]);
        self.record_attempt("success", domain);
    }

    /// Record a failed delivery
    pub fn record_delivery_failure(&self, domain: &str, reason: &str) {
        let attributes = [KeyValue::new("reason", reason.to_string())];
        self.messages_failed.add(1, &attributes);
        self.record_attempt("failed", domain);
    }

    /// Record a delivery retry
    pub fn record_delivery_retry(&self, domain: &str) {
        self.messages_retrying.add(1, &[]);
        self.record_attempt("retry", domain);
    }

    /// Record a temporary failure
    pub fn record_temporary_failure(&self, domain: &str) {
        self.record_attempt("temporary_failure", domain);
    }

    /// Update queue size for a specific status
    pub fn update_queue_size(&self, status: &str, delta: i64) {
        let counter = match status {
            "pending" => &self.queue_pending,
            "in_progress" => &self.queue_in_progress,
            "completed" => &self.queue_completed,
            "failed" => &self.queue_failed,
            "retry" => &self.queue_retry,
            "expired" => &self.queue_expired,
            _ => return,
        };

        if delta > 0 {
            counter.fetch_add(delta.cast_unsigned(), Ordering::Relaxed);
        } else {
            counter.fetch_sub((-delta).cast_unsigned(), Ordering::Relaxed);
        }
    }

    /// Set absolute queue size for a specific status
    pub fn set_queue_size(&self, status: &str, size: u64) {
        let counter = match status {
            "pending" => &self.queue_pending,
            "in_progress" => &self.queue_in_progress,
            "completed" => &self.queue_completed,
            "failed" => &self.queue_failed,
            "retry" => &self.queue_retry,
            "expired" => &self.queue_expired,
            _ => return,
        };

        counter.store(size, Ordering::Relaxed);
    }

    /// Get current queue size for a status
    #[must_use]
    pub fn get_queue_size(&self, status: &str) -> u64 {
        let counter = match status {
            "pending" => &self.queue_pending,
            "in_progress" => &self.queue_in_progress,
            "completed" => &self.queue_completed,
            "failed" => &self.queue_failed,
            "retry" => &self.queue_retry,
            "expired" => &self.queue_expired,
            _ => return 0,
        };

        counter.load(Ordering::Relaxed)
    }

    /// Record a new active connection
    pub fn record_connection_opened(&self) {
        self.active_connections.add(1, &[]);
        self.active_conn_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a closed connection
    pub fn record_connection_closed(&self) {
        self.active_connections.add(-1, &[]);
        self.active_conn_count.fetch_sub(1, Ordering::Relaxed);
    }

    /// Get the current number of active connections
    #[must_use]
    pub fn active_connections_count(&self) -> u64 {
        self.active_conn_count.load(Ordering::Relaxed)
    }
}

/// Get the OpenTelemetry meter for delivery metrics
fn meter() -> Meter {
    opentelemetry::global::meter("empath.delivery")
}
