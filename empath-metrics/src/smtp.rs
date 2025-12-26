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
    metrics::{Counter, Gauge, Histogram, Meter, UpDownCounter},
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

    /// TLS handshake duration in seconds
    tls_handshake_duration: Histogram<f64>,

    /// TLS handshake failures
    tls_handshake_failures: Counter<u64>,

    /// DKIM verification results
    dkim_verifications_total: Counter<u64>,
    /// DKIM enforcement rejections
    dkim_rejections_total: Counter<u64>,

    /// Total TLS contexts registered
    tls_cert_contexts_total: Gauge<u64>,
    /// Minimum seconds until any TLS cert expires (0 if expired or none)
    tls_cert_expiry_min_seconds: Gauge<u64>,
    /// Number of expired TLS certificates
    tls_cert_expired_count: Gauge<u64>,
    /// Number of TLS certificate parse errors
    tls_cert_parse_errors: Gauge<u64>,

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
    #[allow(clippy::too_many_lines)]
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

        let tls_handshake_duration = meter
            .f64_histogram("empath.smtp.tls.handshake.duration.seconds")
            .with_description("TLS handshake duration for SMTP connections")
            .build();

        let tls_handshake_failures = meter
            .u64_counter("empath.smtp.tls.handshake.failures")
            .with_description("Count of TLS handshake failures for SMTP connections")
            .build();

        let dkim_verifications_total = meter
            .u64_counter("empath.smtp.dkim.verifications.total")
            .with_description("Total DKIM verification results for SMTP messages")
            .build();

        let dkim_rejections_total = meter
            .u64_counter("empath.smtp.dkim.rejections.total")
            .with_description("Total DKIM enforcement rejections for SMTP messages")
            .build();

        let tls_cert_contexts_total = meter
            .u64_gauge("empath.smtp.tls.cert.contexts.total")
            .with_description("Number of TLS contexts registered for SMTP listeners")
            .build();

        let tls_cert_expiry_min_seconds = meter
            .u64_gauge("empath.smtp.tls.cert.expiry.min.seconds")
            .with_description("Minimum seconds until any TLS cert expires (0 if expired or none)")
            .with_unit("{seconds}")
            .build();

        let tls_cert_expired_count = meter
            .u64_gauge("empath.smtp.tls.cert.expired.count")
            .with_description("Number of TLS certificates already expired")
            .build();

        let tls_cert_parse_errors = meter
            .u64_gauge("empath.smtp.tls.cert.parse_errors.count")
            .with_description("Number of TLS certificate parse/inspection errors")
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
            tls_handshake_duration,
            tls_handshake_failures,
            dkim_verifications_total,
            dkim_rejections_total,
            tls_cert_contexts_total,
            tls_cert_expiry_min_seconds,
            tls_cert_expired_count,
            tls_cert_parse_errors,
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

    /// Record a TLS handshake success
    pub fn record_tls_handshake(&self, duration_secs: f64, proto: &str, cipher: &str) {
        let attributes = [
            KeyValue::new("protocol", proto.to_string()),
            KeyValue::new("cipher", cipher.to_string()),
        ];
        self.tls_handshake_duration
            .record(duration_secs, &attributes);
    }

    /// Record a TLS handshake failure
    pub fn record_tls_handshake_failure(&self, reason: &str) {
        let attributes = [KeyValue::new("reason", reason.to_string())];
        self.tls_handshake_failures.add(1, &attributes);
    }

    /// Record DKIM verification result for a message.
    pub fn record_dkim_verification(&self, result: &str) {
        let attributes = [KeyValue::new("result", result.to_string())];
        self.dkim_verifications_total.add(1, &attributes);
    }

    /// Record DKIM enforcement rejection for a message.
    pub fn record_dkim_rejection(&self, result: &str) {
        let attributes = [KeyValue::new("result", result.to_string())];
        self.dkim_rejections_total.add(1, &attributes);
    }

    /// Record TLS certificate watchdog metrics.
    pub fn record_tls_cert_status(
        &self,
        contexts_total: u64,
        min_seconds_until_expiry: Option<u64>,
        expired_count: u64,
        parse_error_count: u64,
    ) {
        self.tls_cert_contexts_total.record(contexts_total, &[]);
        self.tls_cert_expiry_min_seconds
            .record(min_seconds_until_expiry.unwrap_or(0), &[]);
        self.tls_cert_expired_count.record(expired_count, &[]);
        self.tls_cert_parse_errors.record(parse_error_count, &[]);
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
        self.connections_failed_count
            .fetch_add(1, Ordering::Relaxed);
    }
}

/// Get the OpenTelemetry meter for SMTP metrics
fn meter() -> Meter {
    opentelemetry::global::meter("empath.smtp")
}
