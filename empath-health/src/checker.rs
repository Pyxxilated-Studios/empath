//! Health check logic

use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

/// Health checker that tracks system component status
///
/// This struct provides thread-safe health status tracking for various
/// system components (SMTP, spool, delivery, DNS).
#[derive(Debug)]
pub struct HealthChecker {
    /// Whether SMTP listeners are bound and accepting connections
    smtp_ready: Arc<AtomicBool>,

    /// Whether the spool is writable
    spool_ready: Arc<AtomicBool>,

    /// Whether the delivery processor is running
    delivery_ready: Arc<AtomicBool>,

    /// Whether the DNS resolver is operational
    dns_ready: Arc<AtomicBool>,

    /// Current queue size (number of pending messages)
    queue_size: Arc<AtomicU64>,

    /// Maximum queue size threshold for readiness
    max_queue_size: u64,
}

impl HealthChecker {
    /// Create a new health checker with the specified maximum queue size
    #[must_use]
    pub fn new(max_queue_size: u64) -> Self {
        Self {
            smtp_ready: Arc::new(AtomicBool::new(false)),
            spool_ready: Arc::new(AtomicBool::new(false)),
            delivery_ready: Arc::new(AtomicBool::new(false)),
            dns_ready: Arc::new(AtomicBool::new(false)),
            queue_size: Arc::new(AtomicU64::new(0)),
            max_queue_size,
        }
    }

    /// Mark SMTP as ready (listeners bound)
    pub fn set_smtp_ready(&self, ready: bool) {
        self.smtp_ready.store(ready, Ordering::Relaxed);
        tracing::debug!(ready, "SMTP readiness updated");
    }

    /// Mark spool as ready (writable)
    pub fn set_spool_ready(&self, ready: bool) {
        self.spool_ready.store(ready, Ordering::Relaxed);
        tracing::debug!(ready, "Spool readiness updated");
    }

    /// Mark delivery processor as ready
    pub fn set_delivery_ready(&self, ready: bool) {
        self.delivery_ready.store(ready, Ordering::Relaxed);
        tracing::debug!(ready, "Delivery readiness updated");
    }

    /// Mark DNS resolver as ready
    pub fn set_dns_ready(&self, ready: bool) {
        self.dns_ready.store(ready, Ordering::Relaxed);
        tracing::debug!(ready, "DNS readiness updated");
    }

    /// Update the current queue size
    pub fn set_queue_size(&self, size: u64) {
        self.queue_size.store(size, Ordering::Relaxed);
    }

    /// Check if the application is alive
    ///
    /// For liveness, we just need to respond. If we can't respond,
    /// the HTTP server itself is dead, which Kubernetes will detect
    /// via timeout.
    #[must_use]
    pub const fn is_alive(&self) -> bool {
        true
    }

    /// Check if the application is ready to accept traffic
    ///
    /// Returns true if all components are ready and queue size is below threshold.
    #[must_use]
    pub fn is_ready(&self) -> bool {
        let smtp_ok = self.smtp_ready.load(Ordering::Relaxed);
        let spool_ok = self.spool_ready.load(Ordering::Relaxed);
        let delivery_ok = self.delivery_ready.load(Ordering::Relaxed);
        let dns_ok = self.dns_ready.load(Ordering::Relaxed);
        let current_queue = self.queue_size.load(Ordering::Relaxed);
        let queue_ok = current_queue < self.max_queue_size;

        let ready = smtp_ok && spool_ok && delivery_ok && dns_ok && queue_ok;

        if !ready {
            tracing::debug!(
                smtp_ready = smtp_ok,
                spool_ready = spool_ok,
                delivery_ready = delivery_ok,
                dns_ready = dns_ok,
                queue_size = current_queue,
                max_queue_size = self.max_queue_size,
                "Readiness check failed"
            );
        }

        ready
    }

    /// Get detailed readiness status for debugging
    #[must_use]
    pub fn get_status(&self) -> HealthStatus {
        HealthStatus {
            alive: self.is_alive(),
            ready: self.is_ready(),
            smtp_ready: self.smtp_ready.load(Ordering::Relaxed),
            spool_ready: self.spool_ready.load(Ordering::Relaxed),
            delivery_ready: self.delivery_ready.load(Ordering::Relaxed),
            dns_ready: self.dns_ready.load(Ordering::Relaxed),
            queue_size: self.queue_size.load(Ordering::Relaxed),
            max_queue_size: self.max_queue_size,
        }
    }
}

/// Detailed health status information
#[derive(Debug, Clone, serde::Serialize)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "Status struct intentionally has multiple boolean fields for clarity"
)]
pub struct HealthStatus {
    /// Whether the application is alive
    pub alive: bool,

    /// Whether the application is ready
    pub ready: bool,

    /// Whether SMTP is ready
    pub smtp_ready: bool,

    /// Whether spool is ready
    pub spool_ready: bool,

    /// Whether delivery is ready
    pub delivery_ready: bool,

    /// Whether DNS is ready
    pub dns_ready: bool,

    /// Current queue size
    pub queue_size: u64,

    /// Maximum queue size threshold
    pub max_queue_size: u64,
}
