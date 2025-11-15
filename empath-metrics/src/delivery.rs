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

    /// Distribution of retry counts before success
    retry_count: Histogram<u64>,

    /// Distribution of queue age (time between spool and delivery attempt)
    queue_age_seconds: Histogram<f64>,

    /// Age of the oldest pending message in the queue
    oldest_message_seconds: Arc<AtomicU64>,

    // Fast atomic counters for hot path (read by observable counters via callbacks)
    messages_delivered_count: Arc<AtomicU64>,
    messages_failed_count: Arc<AtomicU64>,
    messages_retrying_count: Arc<AtomicU64>,

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
    #[allow(clippy::too_many_lines)]
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

        // Create atomic counters for high-frequency metrics
        let messages_delivered_ref = Arc::new(AtomicU64::new(0));
        let messages_failed_ref = Arc::new(AtomicU64::new(0));
        let messages_retrying_ref = Arc::new(AtomicU64::new(0));

        // Observable counter for delivered messages (reads from atomic)
        // Meter keeps this alive internally via callback
        let delivered_clone = messages_delivered_ref.clone();
        meter
            .u64_observable_counter("empath.delivery.messages.delivered.total")
            .with_description("Total number of messages delivered successfully")
            .with_callback(move |observer| {
                observer.observe(delivered_clone.load(Ordering::Relaxed), &[]);
            })
            .build();

        // Observable counter for failed messages (reads from atomic)
        // Meter keeps this alive internally via callback
        let failed_clone = messages_failed_ref.clone();
        meter
            .u64_observable_counter("empath.delivery.messages.failed.total")
            .with_description("Total number of messages permanently failed")
            .with_callback(move |observer| {
                observer.observe(failed_clone.load(Ordering::Relaxed), &[]);
            })
            .build();

        // Observable counter for retrying messages (reads from atomic)
        // Meter keeps this alive internally via callback
        let retrying_clone = messages_retrying_ref.clone();
        meter
            .u64_observable_counter("empath.delivery.messages.retrying.total")
            .with_description("Total number of messages retrying")
            .with_callback(move |observer| {
                observer.observe(retrying_clone.load(Ordering::Relaxed), &[]);
            })
            .build();

        // Observable gauge for delivery error rate (failed / total attempts)
        // Pre-calculated for easier alerting
        let delivered_for_error_rate = messages_delivered_ref.clone();
        let failed_for_error_rate = messages_failed_ref.clone();
        let retrying_for_error_rate = messages_retrying_ref.clone();
        meter
            .f64_observable_gauge("empath.delivery.error_rate")
            .with_description("Delivery error rate (failed / total attempts, 0-1)")
            .with_callback(move |observer| {
                let delivered = delivered_for_error_rate.load(Ordering::Relaxed);
                let failed = failed_for_error_rate.load(Ordering::Relaxed);
                let retrying = retrying_for_error_rate.load(Ordering::Relaxed);
                let total = delivered + failed + retrying;

                let error_rate = if total > 0 {
                    #[allow(clippy::cast_precision_loss)]
                    {
                        failed as f64 / total as f64
                    }
                } else {
                    0.0
                };

                observer.observe(error_rate, &[]);
            })
            .build();

        // Observable gauge for delivery success rate (1 - error_rate)
        // Pre-calculated for easier alerting
        let delivered_for_success_rate = messages_delivered_ref.clone();
        let failed_for_success_rate = messages_failed_ref.clone();
        let retrying_for_success_rate = messages_retrying_ref.clone();
        meter
            .f64_observable_gauge("empath.delivery.success_rate")
            .with_description("Delivery success rate (delivered / total attempts, 0-1)")
            .with_callback(move |observer| {
                let delivered = delivered_for_success_rate.load(Ordering::Relaxed);
                let failed = failed_for_success_rate.load(Ordering::Relaxed);
                let retrying = retrying_for_success_rate.load(Ordering::Relaxed);
                let total = delivered + failed + retrying;

                let success_rate = if total > 0 {
                    #[allow(clippy::cast_precision_loss)]
                    {
                        delivered as f64 / total as f64
                    }
                } else {
                    0.0
                };

                observer.observe(success_rate, &[]);
            })
            .build();

        let retry_count = meter
            .u64_histogram("empath.delivery.retry.count")
            .with_description("Distribution of retry counts before success")
            .build();

        let queue_age_seconds = meter
            .f64_histogram("empath.delivery.queue.age.seconds")
            .with_description("Distribution of queue age (time between spool and delivery attempt)")
            .build();

        // Create atomic counter for oldest message tracking
        let oldest_message_ref = Arc::new(AtomicU64::new(0));

        // Observable gauge for oldest message age (reads from atomic)
        let oldest_clone = oldest_message_ref.clone();
        meter
            .u64_observable_gauge("empath.delivery.queue.oldest.seconds")
            .with_description("Age of the oldest pending message in the queue")
            .with_callback(move |observer| {
                observer.observe(oldest_clone.load(Ordering::Relaxed), &[]);
            })
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
            retry_count,
            queue_age_seconds,
            oldest_message_seconds: oldest_message_ref,
            messages_delivered_count: messages_delivered_ref,
            messages_failed_count: messages_failed_ref,
            messages_retrying_count: messages_retrying_ref,
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
        // Fast atomic increment instead of Counter::add() (80-120ns → <10ns)
        self.messages_delivered_count
            .fetch_add(1, Ordering::Relaxed);
        self.retry_count.record(retry_count, &[]);
        self.record_attempt("success", domain);
    }

    /// Record a failed delivery
    pub fn record_delivery_failure(&self, domain: &str, _reason: &str) {
        // Fast atomic increment instead of Counter::add() (80-120ns → <10ns)
        // Note: reason attribute removed for performance - total failures tracked only
        self.messages_failed_count.fetch_add(1, Ordering::Relaxed);
        self.record_attempt("failed", domain);
    }

    /// Record a delivery retry
    pub fn record_delivery_retry(&self, domain: &str) {
        // Fast atomic increment instead of Counter::add() (80-120ns → <10ns)
        self.messages_retrying_count.fetch_add(1, Ordering::Relaxed);
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

    /// Record the queue age for a delivery attempt
    ///
    /// Calculates the time between when the message was spooled and when
    /// the delivery attempt is being made.
    ///
    /// # Arguments
    ///
    /// * `queued_at` - When the message was first queued for delivery
    pub fn record_queue_age(&self, queued_at: std::time::SystemTime) {
        let now = std::time::SystemTime::now();
        if let Ok(age) = now.duration_since(queued_at) {
            self.queue_age_seconds.record(age.as_secs_f64(), &[]);
        }
    }

    /// Update the age of the oldest pending message in the queue
    ///
    /// This should be called periodically to track the maximum queue age.
    ///
    /// # Arguments
    ///
    /// * `oldest_age_secs` - Age of the oldest pending message in seconds
    pub fn update_oldest_message_age(&self, oldest_age_secs: u64) {
        self.oldest_message_seconds
            .store(oldest_age_secs, Ordering::Relaxed);
    }
}

/// Get the OpenTelemetry meter for delivery metrics
fn meter() -> Meter {
    opentelemetry::global::meter("empath.delivery")
}
