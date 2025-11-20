//! Delivery processor orchestration

pub mod cleanup;
pub mod delivery;
pub mod process;
pub mod scan;

use std::{sync::Arc, time::Duration};

use empath_common::{Signal, internal};
use empath_tracing::traced;
use serde::Deserialize;

use crate::{
    circuit_breaker::{CircuitBreaker, CircuitBreakerConfig},
    dns::{DnsConfig, DnsResolver, HickoryDnsResolver},
    domain_config::DomainConfigRegistry,
    dsn::DsnConfig,
    error::DeliveryError,
    policy::RetryPolicy,
    queue::DeliveryQueue,
    rate_limiter::{RateLimitConfig, RateLimiter},
    types::SmtpTimeouts,
};

const fn default_scan_interval() -> u64 {
    30
}

const fn default_process_interval() -> u64 {
    10
}

fn default_max_concurrent_deliveries() -> usize {
    // Default to number of CPU cores for optimal parallelism
    num_cpus::get()
}

const fn default_cleanup_interval() -> u64 {
    60 // 1 minute
}

const fn default_max_cleanup_attempts() -> u32 {
    3 // 3 attempts before giving up
}

/// Processor for handling delivery of messages from the spool
///
/// This processor runs continuously, scanning the spool for new messages
/// and processing the delivery queue at configurable intervals.
#[allow(
    clippy::unsafe_derive_deserialize,
    reason = "The unsafe aspects have nothing to do with the struct"
)]
#[derive(Debug, Deserialize)]
pub struct DeliveryProcessor {
    /// How often to scan the spool for new messages (in seconds)
    #[serde(default = "default_scan_interval")]
    pub scan_interval_secs: u64,

    /// How often to process the delivery queue (in seconds)
    #[serde(default = "default_process_interval")]
    pub process_interval_secs: u64,

    /// Retry policy configuration
    ///
    /// Controls retry behavior including maximum attempts, exponential backoff,
    /// and jitter. This is flattened into the config for backward compatibility.
    ///
    /// Default: 25 attempts, 300s base delay, 86400s max delay, 10% jitter
    #[serde(flatten, default)]
    pub retry_policy: RetryPolicy,

    /// Maximum number of concurrent delivery tasks
    ///
    /// Controls the level of parallelism for processing the delivery queue.
    /// Higher values increase throughput but also increase resource usage.
    ///
    /// Default: Number of CPU cores (`num_cpus::get()`)
    #[serde(default = "default_max_concurrent_deliveries")]
    pub max_concurrent_deliveries: usize,

    /// Message expiration time (in seconds)
    ///
    /// Messages older than this will be marked as expired and removed from the queue.
    /// Set to `None` to never expire messages.
    ///
    /// Default: None (never expire)
    #[serde(default)]
    pub message_expiration_secs: Option<u64>,

    /// Accept invalid TLS certificates globally (for testing only)
    ///
    /// **SECURITY WARNING**: Setting this to `true` disables certificate validation
    /// for all domains (unless overridden per-domain), making connections vulnerable
    /// to Man-in-the-Middle attacks. Only enable for testing with self-signed certificates.
    ///
    /// Default: `false` (secure)
    #[serde(default)]
    pub accept_invalid_certs: bool,

    /// DNS configuration for resolver
    #[serde(default)]
    pub dns: DnsConfig,

    /// Per-domain delivery configuration
    #[serde(default)]
    pub domains: DomainConfigRegistry,

    /// SMTP operation timeout configuration
    #[serde(default)]
    pub smtp_timeouts: SmtpTimeouts,

    /// DSN (Delivery Status Notification) configuration
    ///
    /// Controls generation of bounce messages for failed deliveries.
    /// DSNs are sent back to the original sender when delivery fails permanently
    /// or after max retry attempts are exhausted.
    ///
    /// Default: enabled with localhost as reporting MTA
    #[serde(default)]
    pub dsn: DsnConfig,

    /// Rate limiting configuration
    ///
    /// Controls per-domain rate limits to prevent overwhelming recipient servers
    /// and avoid blacklisting. Uses token bucket algorithm for burst tolerance.
    ///
    /// Default: 10 messages/sec with burst of 20
    #[serde(default)]
    pub rate_limit: RateLimitConfig,

    /// Circuit breaker configuration
    ///
    /// Controls per-domain circuit breakers to prevent retry storms when destination
    /// servers are experiencing prolonged outages. Automatically opens the circuit
    /// after a threshold of failures, rejecting delivery attempts immediately to
    /// preserve resources.
    ///
    /// Default: 5 failures in 60 seconds triggers 5-minute circuit open
    #[serde(default)]
    pub circuit_breaker: CircuitBreakerConfig,

    /// How often to process the cleanup queue for failed deletions (in seconds)
    ///
    /// When spool deletion fails after successful delivery, messages are added
    /// to the cleanup queue and retried with exponential backoff.
    ///
    /// Default: 60 seconds
    #[serde(default = "default_cleanup_interval")]
    pub cleanup_interval_secs: u64,

    /// Maximum number of cleanup attempts before giving up
    ///
    /// After this many failed deletion attempts, a CRITICAL alert is logged
    /// and the message is removed from the cleanup queue (manual intervention required).
    ///
    /// Default: 3 attempts
    #[serde(default = "default_max_cleanup_attempts")]
    pub max_cleanup_attempts: u32,

    /// The spool backing store to read messages from (initialized in `init()`)
    #[serde(skip)]
    pub(crate) spool: Option<Arc<dyn empath_spool::BackingStore>>,

    /// The delivery queue (initialized in `init()`)
    #[serde(skip)]
    pub(crate) queue: DeliveryQueue,

    /// DNS resolver for MX record lookups (initialized in `init()`)
    #[serde(skip)]
    pub(crate) dns_resolver: Option<Arc<dyn DnsResolver>>,

    /// Cleanup queue for failed spool deletions (initialized in `init()`)
    #[serde(skip)]
    pub cleanup_queue: crate::queue::cleanup::CleanupQueue,

    /// Delivery metrics collector (initialized in `init()`)
    #[serde(skip)]
    pub(crate) metrics: Option<empath_metrics::DeliveryMetrics>,

    /// Rate limiter for per-domain throttling (initialized in `init()`)
    #[serde(skip)]
    pub(crate) rate_limiter: Option<RateLimiter>,

    /// Circuit breaker for per-domain failure protection (initialized in `init()`)
    #[serde(skip)]
    pub(crate) circuit_breaker_instance: Option<CircuitBreaker>,

    /// Domain policy resolver for configuration lookups (initialized in `init()`)
    #[serde(skip)]
    pub(crate) domain_resolver: Option<crate::policy::DomainPolicyResolver>,
}

impl Default for DeliveryProcessor {
    fn default() -> Self {
        Self {
            scan_interval_secs: default_scan_interval(),
            process_interval_secs: default_process_interval(),
            retry_policy: RetryPolicy::default(),
            max_concurrent_deliveries: default_max_concurrent_deliveries(),
            message_expiration_secs: None,
            accept_invalid_certs: false,
            dns: DnsConfig::default(),
            domains: DomainConfigRegistry::default(),
            smtp_timeouts: SmtpTimeouts::default(),
            dsn: DsnConfig::default(),
            rate_limit: RateLimitConfig::default(),
            circuit_breaker: CircuitBreakerConfig::default(),
            cleanup_interval_secs: default_cleanup_interval(),
            max_cleanup_attempts: default_max_cleanup_attempts(),
            spool: None,
            queue: DeliveryQueue::new(),
            dns_resolver: None,
            cleanup_queue: crate::queue::cleanup::CleanupQueue::new(),
            metrics: None,
            rate_limiter: None,
            circuit_breaker_instance: None,
            domain_resolver: None,
        }
    }
}

impl DeliveryProcessor {
    /// Initialize the delivery processor
    ///
    /// # Errors
    ///
    /// Returns an error if the processor cannot be initialized
    pub fn init(
        &mut self,
        spool: Arc<dyn empath_spool::BackingStore>,
        dns_resolver: Option<Arc<dyn crate::DnsResolver>>,
    ) -> Result<(), DeliveryError> {
        internal!("Initialising Delivery Processor ...");
        self.spool = Some(spool);

        // Use injected DNS resolver or create default HickoryDnsResolver
        self.dns_resolver = if let Some(resolver) = dns_resolver {
            internal!("Using injected DNS resolver (likely MockDnsResolver for testing)");
            Some(resolver)
        } else {
            let resolver = Arc::new(HickoryDnsResolver::with_dns_config(self.dns.clone())?);
            internal!(
                "DNS resolver initialized with timeout={}s, cache_ttl={}, min_ttl={}s, max_ttl={}s, cache_size={}",
                self.dns.timeout_secs,
                self.dns.cache_ttl_secs.map_or_else(
                    || "DNS record TTL".to_string(),
                    |ttl| format!("{ttl}s (override)")
                ),
                self.dns.min_cache_ttl_secs,
                self.dns.max_cache_ttl_secs,
                self.dns.cache_size
            );
            Some(resolver)
        };

        // Initialize delivery metrics
        match empath_metrics::DeliveryMetrics::new(10, Vec::default()) {
            Ok(metrics) => {
                self.metrics = Some(metrics);
                internal!("Delivery metrics initialized");
            }
            Err(e) => {
                empath_common::tracing::warn!(error = %e, "Failed to initialize delivery metrics");
            }
        }

        // Initialize rate limiter
        self.rate_limiter = Some(RateLimiter::new(self.rate_limit.clone()));
        internal!(
            "Rate limiter initialized (default: {} msg/sec, burst: {})",
            self.rate_limit.messages_per_second,
            self.rate_limit.burst_size
        );

        // Initialize circuit breaker
        self.circuit_breaker_instance = Some(CircuitBreaker::new(self.circuit_breaker.clone()));
        internal!(
            "Circuit breaker initialized (threshold: {} failures in {}s, timeout: {}s)",
            self.circuit_breaker.failure_threshold,
            self.circuit_breaker.failure_window_secs,
            self.circuit_breaker.timeout_secs
        );

        // Initialize domain policy resolver
        self.domain_resolver = Some(crate::policy::DomainPolicyResolver::new(
            self.domains.clone(),
            self.accept_invalid_certs,
        ));
        internal!(
            "Domain policy resolver initialized ({} domain configs, global accept_invalid_certs: {})",
            self.domains.len(),
            self.accept_invalid_certs
        );

        Ok(())
    }

    /// Get a reference to the DNS resolver (for control interface)
    #[must_use]
    pub const fn dns_resolver(&self) -> &Option<Arc<dyn DnsResolver>> {
        &self.dns_resolver
    }

    /// Get a reference to the domain configuration registry (for control interface)
    #[must_use]
    pub const fn domains(&self) -> &DomainConfigRegistry {
        &self.domains
    }

    /// Get a reference to the spool backing store (for control interface)
    #[must_use]
    pub const fn spool(&self) -> &Option<Arc<dyn empath_spool::BackingStore>> {
        &self.spool
    }

    /// Run the delivery processor
    ///
    /// This method runs continuously until a shutdown signal is received.
    /// It periodically scans the spool for new messages and processes the
    /// delivery queue.
    ///
    /// ## Graceful Shutdown
    ///
    /// When a shutdown signal is received:
    /// 1. Stop accepting new work (scan/process ticks)
    /// 2. Wait for any in-flight delivery to complete (with 30s timeout)
    /// 3. Exit cleanly
    ///
    /// In-flight deliveries that don't complete within the shutdown timeout
    /// will be marked as pending and retried on the next restart.
    ///
    /// # Errors
    ///
    /// Returns an error if the delivery processor encounters a fatal error
    #[allow(clippy::too_many_lines)]
    #[traced(instrument(level = empath_common::tracing::Level::TRACE, skip_all))]
    pub async fn serve(
        self: Arc<Self>,
        mut shutdown: tokio::sync::broadcast::Receiver<Signal>,
    ) -> Result<(), DeliveryError> {
        internal!("Delivery processor starting");

        let Some(ref spool) = self.spool else {
            return Err(crate::error::SystemError::NotInitialized(
                "Delivery processor not initialized. Call init() first.".to_string(),
            )
            .into());
        };

        let scan_interval = Duration::from_secs(self.scan_interval_secs);
        let process_interval = Duration::from_secs(self.process_interval_secs);
        let cleanup_interval = Duration::from_secs(self.cleanup_interval_secs);

        // Perform initial spool scan at startup to load existing messages
        // This ensures `empathctl queue list` works immediately
        match scan::scan_spool_internal(&self, spool).await {
            Ok(count) if count > 0 => {
                internal!("Initial spool scan found {count} messages");
            }
            Ok(_) => {
                internal!("Initial spool scan found no messages");
            }
            Err(e) => {
                empath_common::tracing::error!("Error during initial spool scan: {e}");
            }
        }

        let mut scan_timer = tokio::time::interval(scan_interval);
        let mut process_timer = tokio::time::interval(process_interval);
        let mut cleanup_timer = tokio::time::interval(cleanup_interval);

        // Skip the first tick to avoid immediate execution (after initial scan)
        scan_timer.tick().await;
        process_timer.tick().await;
        cleanup_timer.tick().await;

        // Track if we're currently processing a delivery
        let processing = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let processing_clone = processing.clone();

        loop {
            tokio::select! {
                _ = scan_timer.tick() => {
                    match scan::scan_spool_internal(&self, spool).await {
                        Ok(count) if count > 0 => {
                            empath_common::tracing::info!("Scanned spool, found {count} new messages");
                        }
                        Ok(_) => {
                            empath_common::tracing::debug!("Scanned spool, no new messages");
                        }
                        Err(e) => {
                            empath_common::tracing::error!("Error scanning spool: {e}");
                        }
                    }
                }
                _ = process_timer.tick() => {

                    // Mark that we're processing
                    processing.store(true, std::sync::atomic::Ordering::SeqCst);

                    match process::process_queue_internal(Arc::clone(&self), Arc::clone(spool)).await {
                        Ok(()) => {
                            empath_common::tracing::debug!("Processed delivery queue");
                        }
                        Err(e) => {
                            empath_common::tracing::error!("Error processing delivery queue: {e}");
                        }
                    }

                    // Mark that we're done processing
                    processing.store(false, std::sync::atomic::Ordering::SeqCst);
                }
                _ = cleanup_timer.tick() => {
                    match cleanup::process_cleanup_queue(&self, spool).await {
                        Ok(count) if count > 0 => {
                            empath_common::tracing::info!("Cleanup queue processed, {count} messages cleaned up");
                        }
                        Ok(_) => {
                            empath_common::tracing::debug!("Cleanup queue processed, no messages ready for retry");
                        }
                        Err(e) => {
                            empath_common::tracing::error!("Error processing cleanup queue: {e}");
                        }
                    }
                }
                sig = shutdown.recv() => {
                    match sig {
                        Ok(Signal::Shutdown | Signal::Finalised) => {
                            internal!("Delivery processor received shutdown signal");

                            // Wait for any in-flight delivery to complete (with 30s timeout)
                            let shutdown_timeout = Duration::from_secs(30);
                            let start = std::time::Instant::now();

                            while processing_clone.load(std::sync::atomic::Ordering::SeqCst) {
                                if start.elapsed() >= shutdown_timeout {
                                    empath_common::tracing::warn!(
                                        "Shutdown timeout exceeded, {} remaining in-flight delivery will be retried on restart",
                                        if processing_clone.load(std::sync::atomic::Ordering::SeqCst) { "1" } else { "0" }
                                    );
                                    break;
                                }

                                empath_common::tracing::debug!(
                                    "Waiting for in-flight delivery to complete ({:.1}s elapsed)...",
                                    start.elapsed().as_secs_f64()
                                );
                                tokio::time::sleep(Duration::from_millis(100)).await;
                            }

                            if !processing_clone.load(std::sync::atomic::Ordering::SeqCst) {
                                internal!("All in-flight deliveries completed successfully");
                            }

                            // Queue state is automatically persisted to spool on every status change
                            internal!("Delivery processor shutdown complete");
                            break;
                        }
                        Err(e) => {
                            empath_common::tracing::error!("Delivery processor shutdown channel error: {e}");
                            break;
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Get a reference to the delivery queue
    pub const fn queue(&self) -> &DeliveryQueue {
        &self.queue
    }
}

// Implement DeliveryQueryService trait for DeliveryProcessor
impl crate::service::DeliveryQueryService for DeliveryProcessor {
    fn queue_len(&self) -> usize {
        self.queue.len()
    }

    fn get_message(&self, id: &empath_spool::SpooledMessageId) -> Option<crate::DeliveryInfo> {
        self.queue.get(id)
    }

    fn list_messages(
        &self,
        status: Option<empath_common::DeliveryStatus>,
    ) -> Vec<crate::DeliveryInfo> {
        let all_messages = self.queue.all_messages();

        if let Some(filter_status) = status {
            all_messages
                .into_iter()
                .filter(|info| info.status == filter_status)
                .collect()
        } else {
            all_messages
        }
    }

    fn update_status(
        &self,
        message_id: &empath_spool::SpooledMessageId,
        status: empath_common::DeliveryStatus,
    ) {
        self.queue.update_status(message_id, status);
    }

    fn set_next_retry_at(
        &self,
        message_id: &empath_spool::SpooledMessageId,
        next_retry_at: std::time::SystemTime,
    ) {
        self.queue.set_next_retry_at(message_id, next_retry_at);
    }

    fn reset_server_index(&self, message_id: &empath_spool::SpooledMessageId) {
        self.queue.reset_server_index(message_id);
    }

    fn remove(&self, message_id: &empath_spool::SpooledMessageId) -> Option<crate::DeliveryInfo> {
        self.queue.remove(message_id)
    }

    fn dns_resolver(&self) -> &Option<Arc<dyn DnsResolver>> {
        &self.dns_resolver
    }

    fn spool(&self) -> &Option<std::sync::Arc<dyn empath_spool::BackingStore>> {
        &self.spool
    }

    fn domains(&self) -> &DomainConfigRegistry {
        &self.domains
    }
}
