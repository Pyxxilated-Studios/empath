//! SMTP session metrics
//!
//! Tracks SMTP server performance including:
//! - Total connections (active and completed)
//! - SMTP errors by response code
//! - Session durations
//! - Command processing

use std::sync::atomic::{AtomicU64, Ordering};

use opentelemetry::{
    KeyValue,
    metrics::{Counter, Histogram, Meter, UpDownCounter},
};

use crate::MetricsError;

/// SMTP metrics collector
#[derive(Debug)]
pub struct SmtpMetrics {
    /// Total number of SMTP connections established
    connections_total: Counter<u64>,

    /// Number of currently active SMTP connections
    connections_active: UpDownCounter<i64>,

    /// Total number of SMTP errors by response code
    errors_total: Counter<u64>,

    /// Distribution of SMTP session durations in seconds
    session_duration: Histogram<f64>,

    /// Distribution of command processing durations in seconds
    command_duration: Histogram<f64>,

    /// Total number of messages received via SMTP
    messages_received: Counter<u64>,

    /// Distribution of message sizes in bytes
    message_size_bytes: Histogram<u64>,

    // Local counters for tracking active connections
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

        let connections_total = meter
            .u64_counter("empath.smtp.connections.total")
            .with_description("Total number of SMTP connections established")
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

        let messages_received = meter
            .u64_counter("empath.smtp.messages.received.total")
            .with_description("Total number of messages received via SMTP")
            .build();

        let message_size_bytes = meter
            .u64_histogram("empath.smtp.message.size.bytes")
            .with_description("Distribution of received message sizes")
            .build();

        Ok(Self {
            connections_total,
            connections_active,
            errors_total,
            session_duration,
            command_duration,
            messages_received,
            message_size_bytes,
            active_count: AtomicU64::new(0),
        })
    }

    /// Record a new SMTP connection
    pub fn record_connection(&self) {
        self.connections_total.add(1, &[]);
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
        self.messages_received.add(1, &[]);
        self.message_size_bytes.record(size_bytes, &[]);
    }

    /// Get the current number of active connections
    #[must_use]
    pub fn active_connections(&self) -> u64 {
        self.active_count.load(Ordering::Relaxed)
    }
}

/// Get the OpenTelemetry meter for SMTP metrics
fn meter() -> Meter {
    opentelemetry::global::meter("empath.smtp")
}
