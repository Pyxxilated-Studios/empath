//! SMTP session metrics
//!
//! Tracks SMTP server performance including:
//! - Total connections (active and completed)
//! - SMTP errors by response code
//! - Session durations
//! - Command processing

use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use opentelemetry::{
    KeyValue,
    metrics::{Counter, Histogram, Meter, UpDownCounter},
};

use crate::MetricsError;

/// SMTP metrics collector
#[derive(Debug)]
pub struct SmtpMetrics {
    /// Number of currently active SMTP connections
    connections_active: UpDownCounter<i64>,

    /// Total number of SMTP errors by response code
    errors_total: Counter<u64>,

    /// Distribution of SMTP session durations in seconds
    session_duration: Histogram<f64>,

    /// Distribution of command processing durations in seconds
    command_duration: Histogram<f64>,

    /// Distribution of message sizes in bytes
    message_size_bytes: Histogram<u64>,

    // Fast atomic counters for hot path (read by observable counters via callbacks)
    connections_total_count: Arc<AtomicU64>,
    messages_received_count: Arc<AtomicU64>,
    connections_failed_count: Arc<AtomicU64>,
    active_count: AtomicU64,
}

impl SmtpMetrics {
    /// Create a new SMTP metrics collector
    ///
    /// # Errors
    ///
    /// Returns an error if metric instruments cannot be created.
    pub fn new() -> Result<Self, MetricsError> {
        let meter = meter();

        // Create atomic counters for high-frequency metrics
        let connections_total_ref = Arc::new(AtomicU64::new(0));
        let messages_received_ref = Arc::new(AtomicU64::new(0));
        let connections_failed_ref = Arc::new(AtomicU64::new(0));

        // Observable counter for total connections (reads from atomic)
        // Meter keeps this alive internally via callback
        let connections_clone = connections_total_ref.clone();
        meter
            .u64_observable_counter("empath.smtp.connections.total")
            .with_description("Total number of SMTP connections established")
            .with_callback(move |observer| {
                observer.observe(connections_clone.load(Ordering::Relaxed), &[]);
            })
            .build();

        let connections_active = meter
            .i64_up_down_counter("empath.smtp.connections.active")
            .with_description("Number of currently active SMTP connections")
            .build();

        let errors_total = meter
            .u64_counter("empath.smtp.errors.total")
            .with_description("Total number of SMTP errors by response code")
            .build();

        let session_duration = meter
            .f64_histogram("empath.smtp.session.duration.seconds")
            .with_description("Distribution of SMTP session durations")
            .build();

        let command_duration = meter
            .f64_histogram("empath.smtp.command.duration.seconds")
            .with_description("Distribution of command processing durations")
            .build();

        // Observable counter for messages received (reads from atomic)
        // Meter keeps this alive internally via callback
        let messages_clone = messages_received_ref.clone();
        meter
            .u64_observable_counter("empath.smtp.messages.received.total")
            .with_description("Total number of messages received via SMTP")
            .with_callback(move |observer| {
                observer.observe(messages_clone.load(Ordering::Relaxed), &[]);
            })
            .build();

        let message_size_bytes = meter
            .u64_histogram("empath.smtp.message.size.bytes")
            .with_description("Distribution of received message sizes")
            .build();

        // Observable gauge for SMTP error rate (failed / total connections)
        // Pre-calculated for easier alerting
        let total_for_error_rate = connections_total_ref.clone();
        let failed_for_error_rate = connections_failed_ref.clone();
        meter
            .f64_observable_gauge("empath.smtp.connection.error_rate")
            .with_description("SMTP connection error rate (failed / total connections, 0-1)")
            .with_callback(move |observer| {
                let total = total_for_error_rate.load(Ordering::Relaxed);
                let failed = failed_for_error_rate.load(Ordering::Relaxed);

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

        Ok(Self {
            connections_active,
            errors_total,
            session_duration,
            command_duration,
            message_size_bytes,
            connections_total_count: connections_total_ref,
            messages_received_count: messages_received_ref,
            connections_failed_count: connections_failed_ref,
            active_count: AtomicU64::new(0),
        })
    }

    /// Record a new SMTP connection
    pub fn record_connection(&self) {
        // Fast atomic increment instead of Counter::add() (80-120ns → <10ns)
        self.connections_total_count.fetch_add(1, Ordering::Relaxed);
        self.connections_active.add(1, &[]);
        self.active_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a closed SMTP connection with its duration
    pub fn record_connection_closed(&self, duration_secs: f64) {
        self.connections_active.add(-1, &[]);
        self.active_count.fetch_sub(1, Ordering::Relaxed);
        self.session_duration.record(duration_secs, &[]);
    }

    /// Record an SMTP error
    pub fn record_error(&self, code: u32) {
        let attributes = [KeyValue::new("code", code.to_string())];
        self.errors_total.add(1, &attributes);
    }

    /// Record a command processing duration
    pub fn record_command(&self, command: &str, duration_secs: f64) {
        let attributes = [KeyValue::new("command", command.to_string())];
        self.command_duration.record(duration_secs, &attributes);
    }

    /// Record a received message
    pub fn record_message_received(&self, size_bytes: u64) {
        // Fast atomic increment instead of Counter::add() (80-120ns → <10ns)
        self.messages_received_count.fetch_add(1, Ordering::Relaxed);
        self.message_size_bytes.record(size_bytes, &[]);
    }

    /// Get the current number of active connections
    #[must_use]
    pub fn active_connections(&self) -> u64 {
        self.active_count.load(Ordering::Relaxed)
    }

    /// Record a failed SMTP connection
    ///
    /// This should be called when a connection fails (e.g., protocol error,
    /// authentication failure, etc.) to track the connection error rate.
    pub fn record_connection_failed(&self) {
        // Fast atomic increment for failed connection tracking
        self.connections_failed_count.fetch_add(1, Ordering::Relaxed);
    }
}

/// Get the OpenTelemetry meter for SMTP metrics
fn meter() -> Meter {
    opentelemetry::global::meter("empath.smtp")
}
