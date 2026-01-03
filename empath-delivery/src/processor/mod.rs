//! Delivery processor orchestration

pub mod cleanup;
pub mod delivery;
pub mod process;
pub mod scan;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use dashmap::DashSet;
use empath_common::audit::{AuditLogger, AuditLoggerBuilder};
use empath_common::{DeliveryStatus, Signal, internal, tracing};
use empath_metrics::DeliveryMetrics;
use empath_spool::{BackingStore, SpoolScanCursor, SpoolScanMode, SpooledMessageId};
use empath_tracing::traced;
use parking_lot::RwLock;
use serde::Deserialize;
use tokio::pin;
use tokio::sync::{Mutex, Notify};

use crate::binding::OutboundBindingRegistry;
use crate::binding_health::{BindingHealthConfig, BindingHealthStatus, BindingHealthTracker};
use crate::circuit_breaker::{CircuitBreaker, CircuitBreakerConfig};
use crate::dns::{DnsConfig, DnsResolver, HickoryDnsResolver};
use crate::domain_config::DomainConfigRegistry;
use crate::dsn::DsnConfig;
use crate::error::DeliveryError;
use crate::policy::{RetryPolicy, TlsPolicy};
use crate::queue::DeliveryQueue;
use crate::rate_limiter::{RateLimitConfig, RateLimiter};
use crate::types::{SmtpPoolConfig, SmtpTimeouts};

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
#[derive(Debug)]
pub struct DeliveryProcessor {
    /// How often to scan the spool for new messages (in seconds)
    pub scan_interval_secs: u64,

    /// How often to process the delivery queue (in seconds)
    pub process_interval_secs: u64,

    /// Retry policy configuration
    ///
    /// Controls retry behavior including maximum attempts, exponential backoff,
    /// and jitter. This is flattened into the config for backward compatibility.
    ///
    /// Default: 25 attempts, 300s base delay, 86400s max delay, 10% jitter
    pub retry_policy: RwLock<RetryPolicy>,

    /// Maximum number of concurrent delivery tasks
    ///
    /// Controls the level of parallelism for processing the delivery queue.
    /// Higher values increase throughput but also increase resource usage.
    ///
    /// Default: Number of CPU cores (`num_cpus::get()`)
    pub max_concurrent_deliveries: usize,

    /// Message expiration time (in seconds)
    ///
    /// Messages older than this will be marked as expired and removed from the queue.
    /// Set to `None` to never expire messages.
    ///
    /// Default: None (never expire)
    pub message_expiration_secs: Option<u64>,

    /// Accept invalid TLS certificates globally (for testing only)
    ///
    /// **SECURITY WARNING**: Setting this to `true` disables certificate validation
    /// for all domains (unless overridden per-domain), making connections vulnerable
    /// to Man-in-the-Middle attacks. Only enable for testing with self-signed certificates.
    ///
    /// Default: `false` (secure)
    pub accept_invalid_certs: bool,

    /// Global TLS policy for outbound delivery.
    pub tls_policy: TlsPolicy,

    /// DNS configuration for resolver
    pub dns: DnsConfig,

    /// MTA-STS configuration
    pub mta_sts: crate::mta_sts::MtaStsConfig,

    /// DKIM signing configuration
    pub dkim_signing: Option<crate::dkim::DkimSigningConfig>,

    /// Per-domain delivery configuration
    pub domains: DomainConfigRegistry,

    /// Outbound binding configuration
    pub outbound_bindings: OutboundBindingRegistry,

    /// SMTP operation timeout configuration
    pub smtp_timeouts: RwLock<SmtpTimeouts>,

    /// SMTP client connection pool configuration
    pub smtp_pool: SmtpPoolConfig,

    /// DSN (Delivery Status Notification) configuration
    ///
    /// Controls generation of bounce messages for failed deliveries.
    /// DSNs are sent back to the original sender when delivery fails permanently
    /// or after max retry attempts are exhausted.
    ///
    /// Default: enabled with localhost as reporting MTA
    pub dsn: DsnConfig,

    /// Rate limiting configuration
    ///
    /// Controls per-domain rate limits to prevent overwhelming recipient servers
    /// and avoid blacklisting. Uses token bucket algorithm for burst tolerance.
    ///
    /// Default: 10 messages/sec with burst of 20
    pub rate_limit: RateLimitConfig,

    /// Circuit breaker configuration
    ///
    /// Controls per-domain circuit breakers to prevent retry storms when destination
    /// servers are experiencing prolonged outages. Automatically opens the circuit
    /// after a threshold of failures, rejecting delivery attempts immediately to
    /// preserve resources.
    ///
    /// Default: 5 failures in 60 seconds triggers 5-minute circuit open
    pub circuit_breaker: RwLock<CircuitBreakerConfig>,

    /// How often to process the cleanup queue for failed deletions (in seconds)
    ///
    /// When spool deletion fails after successful delivery, messages are added
    /// to the cleanup queue and retried with exponential backoff.
    ///
    /// Default: 60 seconds
    pub cleanup_interval_secs: u64,

    /// Maximum number of cleanup attempts before giving up
    ///
    /// After this many failed deletion attempts, a CRITICAL alert is logged
    /// and the message is removed from the cleanup queue (manual intervention required).
    ///
    /// Default: 3 attempts
    pub max_cleanup_attempts: u32,

    /// The spool backing store to read messages from (initialized in `init()`)
    pub(crate) spool: Option<Arc<dyn BackingStore>>,

    /// The delivery queue (initialized in `init()`)
    pub(crate) queue: DeliveryQueue,

    /// DNS resolver for MX record lookups (initialized in `init()`)
    pub(crate) dns_resolver: Option<Arc<dyn DnsResolver>>,

    /// MTA-STS policy cache (initialized in `init()`)
    pub(crate) mta_sts_cache: crate::mta_sts::MtaStsPolicyCache,

    /// Cleanup queue for failed spool deletions (initialized in `init()`)
    pub cleanup_queue: crate::queue::cleanup::CleanupQueue,

    /// Delivery metrics collector (initialized in `init()`)
    pub(crate) metrics: Option<DeliveryMetrics>,

    /// SMTP client pool (initialized in `init()`)
    pub(crate) smtp_pool_state: crate::smtp_pool::SmtpClientPool,

    /// Rate limiter for per-domain throttling (initialized in `init()`)
    pub(crate) rate_limiter: Option<RateLimiter>,

    /// Circuit breaker for per-domain failure protection (initialized in `init()`)
    pub(crate) circuit_breaker_instance: Option<CircuitBreaker>,

    /// Binding health tracker (initialized in `init()`)
    pub(crate) binding_health: Option<BindingHealthTracker>,

    /// DKIM signer for outbound messages (initialized in `init()`)
    pub(crate) dkim_signer: Option<Arc<crate::dkim::DkimSigningKey>>,

    /// Audit logger for delivery lifecycle events.
    pub(crate) audit_logger: Arc<dyn AuditLogger>,

    /// Runtime disabled outbound bindings
    pub(crate) disabled_bindings: DashSet<String>,

    /// Domain policy resolver for configuration lookups (initialized in `init()`)
    pub(crate) domain_resolver: Option<crate::policy::DomainPolicyResolver>,

    /// Notifier to trigger immediate scan/process from control commands
    process_now_notifier: Notify,

    /// Cursor for incremental spool scans
    pub(crate) spool_scan_cursor: Mutex<SpoolScanCursor>,
}

#[derive(Debug, Deserialize)]
struct DeliveryProcessorConfig {
    #[serde(default = "default_scan_interval")]
    scan_interval_secs: u64,
    #[serde(default = "default_process_interval")]
    process_interval_secs: u64,
    #[serde(flatten, default)]
    retry_policy: RetryPolicy,
    #[serde(default = "default_max_concurrent_deliveries")]
    max_concurrent_deliveries: usize,
    #[serde(default)]
    message_expiration_secs: Option<u64>,
    #[serde(default)]
    accept_invalid_certs: bool,
    #[serde(default)]
    tls_policy: TlsPolicy,
    #[serde(default)]
    dns: DnsConfig,
    #[serde(default)]
    mta_sts: crate::mta_sts::MtaStsConfig,
    #[serde(default)]
    dkim_signing: Option<crate::dkim::DkimSigningConfig>,
    #[serde(default)]
    domains: DomainConfigRegistry,
    #[serde(default)]
    outbound_bindings: OutboundBindingRegistry,
    #[serde(default)]
    smtp_timeouts: SmtpTimeouts,
    #[serde(default)]
    smtp_pool: SmtpPoolConfig,
    #[serde(default)]
    dsn: DsnConfig,
    #[serde(default)]
    rate_limit: RateLimitConfig,
    #[serde(default)]
    circuit_breaker: CircuitBreakerConfig,
    #[serde(default = "default_cleanup_interval")]
    cleanup_interval_secs: u64,
    #[serde(default = "default_max_cleanup_attempts")]
    max_cleanup_attempts: u32,
}

impl<'de> Deserialize<'de> for DeliveryProcessor {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let config = DeliveryProcessorConfig::deserialize(deserializer)?;
        Ok(Self::from(config))
    }
}

impl From<DeliveryProcessorConfig> for DeliveryProcessor {
    fn from(config: DeliveryProcessorConfig) -> Self {
        Self {
            scan_interval_secs: config.scan_interval_secs,
            process_interval_secs: config.process_interval_secs,
            retry_policy: RwLock::new(config.retry_policy),
            max_concurrent_deliveries: config.max_concurrent_deliveries,
            message_expiration_secs: config.message_expiration_secs,
            accept_invalid_certs: config.accept_invalid_certs,
            tls_policy: config.tls_policy,
            dns: config.dns,
            mta_sts: config.mta_sts.clone(),
            dkim_signing: config.dkim_signing,
            domains: config.domains,
            outbound_bindings: config.outbound_bindings,
            smtp_timeouts: RwLock::new(config.smtp_timeouts),
            smtp_pool: config.smtp_pool.clone(),
            dsn: config.dsn,
            rate_limit: config.rate_limit,
            circuit_breaker: RwLock::new(config.circuit_breaker),
            cleanup_interval_secs: config.cleanup_interval_secs,
            max_cleanup_attempts: config.max_cleanup_attempts,
            mta_sts_cache: crate::mta_sts::MtaStsPolicyCache::builder()
                .with_enabled(config.mta_sts.enabled)
                .with_min_cache_age_secs(config.mta_sts.min_cache_age_secs)
                .with_max_cache_age_secs(config.mta_sts.max_cache_age_secs)
                .build(),
            smtp_pool_state: crate::smtp_pool::SmtpClientPool::new(config.smtp_pool),
            audit_logger: AuditLoggerBuilder::default().build(),
            ..Default::default()
        }
    }
}

impl Default for DeliveryProcessor {
    fn default() -> Self {
        let mta_sts = crate::mta_sts::MtaStsConfig::default();
        Self {
            scan_interval_secs: default_scan_interval(),
            process_interval_secs: default_process_interval(),
            retry_policy: RwLock::new(RetryPolicy::default()),
            max_concurrent_deliveries: default_max_concurrent_deliveries(),
            message_expiration_secs: None,
            accept_invalid_certs: false,
            tls_policy: TlsPolicy::default(),
            dns: DnsConfig::default(),
            mta_sts: mta_sts.clone(),
            dkim_signing: None,
            domains: DomainConfigRegistry::default(),
            outbound_bindings: OutboundBindingRegistry::default(),
            smtp_timeouts: RwLock::new(SmtpTimeouts::default()),
            smtp_pool: SmtpPoolConfig::default(),
            dsn: DsnConfig::default(),
            rate_limit: RateLimitConfig::default(),
            circuit_breaker: RwLock::new(CircuitBreakerConfig::default()),
            cleanup_interval_secs: default_cleanup_interval(),
            max_cleanup_attempts: default_max_cleanup_attempts(),
            spool: None,
            queue: DeliveryQueue::new(),
            dns_resolver: None,
            mta_sts_cache: crate::mta_sts::MtaStsPolicyCache::builder()
                .with_enabled(mta_sts.enabled)
                .with_min_cache_age_secs(mta_sts.min_cache_age_secs)
                .with_max_cache_age_secs(mta_sts.max_cache_age_secs)
                .build(),
            cleanup_queue: crate::queue::cleanup::CleanupQueue::new(),
            metrics: None,
            smtp_pool_state: crate::smtp_pool::SmtpClientPool::new(SmtpPoolConfig::default()),
            rate_limiter: None,
            circuit_breaker_instance: None,
            binding_health: None,
            audit_logger: AuditLoggerBuilder::default().build(),
            dkim_signer: None,
            disabled_bindings: DashSet::new(),
            domain_resolver: None,
            process_now_notifier: Notify::new(),
            spool_scan_cursor: Mutex::new(SpoolScanCursor::default()),
        }
    }
}

impl DeliveryProcessor {
    pub fn set_audit_logger(&mut self, audit_logger: Arc<dyn AuditLogger>) {
        self.audit_logger = audit_logger;
    }

    #[must_use]
    pub fn binding_enabled(&self, name: &str) -> bool {
        !self.disabled_bindings.contains(name)
    }

    /// Enable or disable an outbound binding at runtime.
    ///
    /// # Errors
    ///
    /// Returns an error if the binding does not exist.
    pub fn set_binding_enabled(&self, name: &str, enabled: bool) -> Result<(), String> {
        if self.outbound_bindings.get(name).is_none() {
            return Err(format!("Unknown outbound binding '{name}'"));
        }

        if enabled {
            self.disabled_bindings.remove(name);
        } else {
            self.disabled_bindings.insert(name.to_string());
        }

        Ok(())
    }

    #[must_use]
    pub fn binding_health_config_for(&self, name: &str) -> BindingHealthConfig {
        self.outbound_bindings.health_config_for(name)
    }

    #[must_use]
    pub fn binding_health_status_for(&self, name: &str) -> Option<BindingHealthStatus> {
        self.binding_health
            .as_ref()
            .map(|health| health.status(name, self.binding_health_config_for(name)))
    }

    /// Initialize the delivery processor
    ///
    /// # Errors
    ///
    /// Returns an error if the processor cannot be initialized
    pub fn init(
        &mut self,
        spool: Arc<dyn BackingStore>,
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

        self.mta_sts_cache = crate::mta_sts::MtaStsPolicyCache::builder()
            .with_enabled(self.mta_sts.enabled)
            .with_min_cache_age_secs(self.mta_sts.min_cache_age_secs)
            .with_max_cache_age_secs(self.mta_sts.max_cache_age_secs)
            .build();

        // Initialize delivery metrics
        let metrics = DeliveryMetrics::new(10, Vec::default());
        metrics.register_bindings(self.outbound_bindings.names());
        self.metrics = Some(metrics);
        internal!("Delivery metrics initialized");

        self.smtp_pool_state = crate::smtp_pool::SmtpClientPool::new(self.smtp_pool.clone());
        internal!(
            "SMTP client pool initialized (enabled: {}, max_per_domain: {}, idle_ttl_secs: {})",
            self.smtp_pool.enabled,
            self.smtp_pool.max_per_domain,
            self.smtp_pool.idle_ttl_secs
        );

        // Initialize rate limiter
        self.rate_limiter = Some(RateLimiter::new(self.rate_limit.clone()));
        internal!(
            "Rate limiter initialized (default: {} msg/sec, burst: {})",
            self.rate_limit.messages_per_second,
            self.rate_limit.burst_size
        );

        // Initialize circuit breaker
        let circuit_config = self.circuit_breaker.read().clone();
        self.circuit_breaker_instance = Some(CircuitBreaker::new(circuit_config.clone()));
        internal!(
            "Circuit breaker initialized (threshold: {} failures in {}s, timeout: {}s)",
            circuit_config.failure_threshold,
            circuit_config.failure_window_secs,
            circuit_config.timeout_secs
        );

        self.binding_health = Some(BindingHealthTracker::new());
        internal!("Binding health tracker initialized");

        if let Some(config) = &self.dkim_signing {
            let signer = config.build_signer()?;
            self.dkim_signer = Some(Arc::new(signer));
            internal!(
                "DKIM signing enabled (domain: {}, selector: {})",
                config.domain,
                config.selector
            );
        } else {
            self.dkim_signer = None;
        }

        // Initialize domain policy resolver
        self.domain_resolver = Some(crate::policy::DomainPolicyResolver::new(
            self.domains.clone(),
            self.accept_invalid_certs,
            self.tls_policy,
        ));
        internal!(
            "Domain policy resolver initialized ({} domain configs, global accept_invalid_certs: {})",
            self.domains.len(),
            self.accept_invalid_certs
        );

        if self.tls_policy == TlsPolicy::Disabled {
            tracing::warn!("Global TLS policy is disabled; outbound deliveries will be plaintext");
        }
        for (name, binding) in &self.outbound_bindings.bindings {
            if !binding.tls.attempt_starttls {
                tracing::warn!(
                    binding = %name,
                    "Outbound binding disables STARTTLS; deliveries via this binding will be plaintext"
                );
            }
        }

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
    pub const fn spool(&self) -> &Option<Arc<dyn BackingStore>> {
        &self.spool
    }

    /// Request an immediate scan/process cycle (non-blocking)
    pub fn request_process_now(&self) {
        self.process_now_notifier.notify_one();
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
    #[traced(instrument(level = tracing::Level::TRACE, skip_all))]
    pub async fn serve(
        self: Arc<Self>,
        mut shutdown: tokio::sync::broadcast::Receiver<Signal>,
    ) -> Result<(), DeliveryError> {
        internal!("Delivery processor starting");

        let spool = self.require_spool()?;
        self.initial_spool_scan(spool).await;

        let (mut scan_timer, mut process_timer, mut cleanup_timer, mut pool_cleanup_timer) =
            self.build_intervals();
        let process_now = self.process_now_notifier.notified();
        pin!(process_now);

        // Skip the first tick to avoid immediate execution (after initial scan)
        scan_timer.tick().await;
        process_timer.tick().await;
        cleanup_timer.tick().await;
        pool_cleanup_timer.tick().await;

        // Track if we're currently processing a delivery
        let processing = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let processing_clone = processing.clone();

        loop {
            tokio::select! {
                () = &mut process_now => {
                    self.handle_manual_process(spool, &processing).await;
                    process_now.set(self.process_now_notifier.notified());
                }
                _ = scan_timer.tick() => {
                    self.handle_scan_tick(spool).await;
                }
                _ = process_timer.tick() => {
                    self.handle_process_tick(spool, &processing).await;
                }
                _ = pool_cleanup_timer.tick() => {
                    self.handle_pool_cleanup();
                }
                _ = cleanup_timer.tick() => {
                    self.handle_cleanup_tick(spool).await;
                }
                sig = shutdown.recv() => {
                    match sig {
                        Ok(Signal::Shutdown | Signal::Finalised) => {
                            internal!("Delivery processor received shutdown signal");
                            self.wait_for_processing(&processing_clone).await;

                            // Queue state is automatically persisted to spool on every status change
                            internal!("Delivery processor shutdown complete");
                            break;
                        }
                        Ok(Signal::Drain(_)) => {}
                        Err(e) => {
                            tracing::error!("Delivery processor shutdown channel error: {e}");
                            break;
                        }
                    }
                }
            }
        }

        Ok(())
    }

    fn require_spool(&self) -> Result<&Arc<dyn BackingStore>, DeliveryError> {
        self.spool.as_ref().ok_or_else(|| {
            crate::error::SystemError::NotInitialized(
                "Delivery processor not initialized. Call init() first.".to_string(),
            )
            .into()
        })
    }

    fn build_intervals(
        &self,
    ) -> (
        tokio::time::Interval,
        tokio::time::Interval,
        tokio::time::Interval,
        tokio::time::Interval,
    ) {
        let scan_interval = Duration::from_secs(self.scan_interval_secs);
        let process_interval = Duration::from_secs(self.process_interval_secs);
        let cleanup_interval = Duration::from_secs(self.cleanup_interval_secs);
        let pool_cleanup_interval = Duration::from_secs(self.smtp_pool.cleanup_interval_secs);

        (
            tokio::time::interval(scan_interval),
            tokio::time::interval(process_interval),
            tokio::time::interval(cleanup_interval),
            tokio::time::interval(pool_cleanup_interval),
        )
    }

    async fn initial_spool_scan(&self, spool: &Arc<dyn BackingStore>) {
        match scan::scan_spool_internal(self, spool, SpoolScanMode::Full).await {
            Ok(count) if count > 0 => {
                internal!("Initial spool scan found {count} messages");
            }
            Ok(_) => {
                internal!("Initial spool scan found no messages");
            }
            Err(e) => {
                tracing::error!("Error during initial spool scan: {e}");
            }
        }
    }

    async fn handle_manual_process(
        self: &Arc<Self>,
        spool: &Arc<dyn BackingStore>,
        processing: &std::sync::atomic::AtomicBool,
    ) {
        processing.store(true, std::sync::atomic::Ordering::SeqCst);

        if let Err(e) = scan::scan_spool_internal(self, spool, SpoolScanMode::Delta).await {
            tracing::error!("Error scanning spool: {e}");
        }

        match process::process_queue_internal(Arc::clone(self), Arc::clone(spool)).await {
            Ok(()) => {
                tracing::info!("Processed delivery queue on manual trigger");
            }
            Err(e) => {
                tracing::error!("Error processing delivery queue on manual trigger: {e}");
            }
        }

        processing.store(false, std::sync::atomic::Ordering::SeqCst);
    }

    async fn handle_scan_tick(&self, spool: &Arc<dyn BackingStore>) {
        match scan::scan_spool_internal(self, spool, SpoolScanMode::Delta).await {
            Ok(count) if count > 0 => {
                tracing::info!("Scanned spool, found {count} new messages");
            }
            Ok(_) => {
                tracing::debug!("Scanned spool, no new messages");
            }
            Err(e) => {
                tracing::error!("Error scanning spool: {e}");
            }
        }
    }

    async fn handle_process_tick(
        self: &Arc<Self>,
        spool: &Arc<dyn BackingStore>,
        processing: &std::sync::atomic::AtomicBool,
    ) {
        processing.store(true, std::sync::atomic::Ordering::SeqCst);

        match process::process_queue_internal(Arc::clone(self), Arc::clone(spool)).await {
            Ok(()) => {
                tracing::debug!("Processed delivery queue");
            }
            Err(e) => {
                tracing::error!("Error processing delivery queue: {e}");
            }
        }

        processing.store(false, std::sync::atomic::Ordering::SeqCst);
    }

    fn handle_pool_cleanup(&self) {
        if self.smtp_pool_state.enabled() {
            let removed = self.smtp_pool_state.cleanup_idle(self.metrics.as_ref());
            if removed > 0 {
                tracing::debug!(
                    removed = removed,
                    "Cleaned up idle SMTP client pool connections"
                );
            }
        }
    }

    async fn handle_cleanup_tick(&self, spool: &Arc<dyn BackingStore>) {
        match cleanup::process_cleanup_queue(self, spool).await {
            Ok(count) if count > 0 => {
                tracing::info!("Cleanup queue processed, {count} messages cleaned up");
            }
            Ok(_) => {
                tracing::debug!("Cleanup queue processed, no messages ready for retry");
            }
            Err(e) => {
                tracing::error!("Error processing cleanup queue: {e}");
            }
        }
    }

    async fn wait_for_processing(&self, processing: &std::sync::atomic::AtomicBool) {
        let shutdown_timeout = Duration::from_secs(30);
        let start = std::time::Instant::now();

        while processing.load(std::sync::atomic::Ordering::SeqCst) {
            if start.elapsed() >= shutdown_timeout {
                tracing::warn!(
                    "Shutdown timeout exceeded, {} remaining in-flight delivery will be retried on restart",
                    if processing.load(std::sync::atomic::Ordering::SeqCst) {
                        "1"
                    } else {
                        "0"
                    }
                );
                break;
            }

            tracing::debug!(
                "Waiting for in-flight delivery to complete ({:.1}s elapsed)...",
                start.elapsed().as_secs_f64()
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        if !processing.load(std::sync::atomic::Ordering::SeqCst) {
            internal!("All in-flight deliveries completed successfully");
        }
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

    fn get_message(&self, id: &SpooledMessageId) -> Option<crate::DeliveryInfo> {
        self.queue.get(id)
    }

    fn list_messages(&self, status: Option<DeliveryStatus>) -> Vec<crate::DeliveryInfo> {
        self.queue
            .queue
            .iter()
            .filter(|entry| {
                status
                    .as_ref()
                    .is_none_or(|filter| entry.value().status == *filter)
            })
            .map(|entry| entry.value().clone())
            .collect()
    }

    fn list_messages_iter(
        &self,
        status: Option<DeliveryStatus>,
    ) -> Box<dyn Iterator<Item = crate::DeliveryInfo> + '_> {
        let iter = self.queue.queue.iter().filter(move |entry| {
            status
                .as_ref()
                .is_none_or(|filter| entry.value().status == *filter)
        });
        Box::new(iter.map(|entry| entry.value().clone()))
    }

    fn list_message_ids(&self, status_filter: Option<&str>) -> Vec<SpooledMessageId> {
        let mut ids: Vec<_> = self
            .queue
            .queue
            .iter()
            .filter(|entry| {
                status_filter.is_none_or(|filter| entry.value().status.matches_filter(filter))
            })
            .map(|entry| entry.key().clone())
            .collect();
        ids.sort();
        ids
    }

    fn queue_stats_snapshot(&self) -> crate::service::QueueSnapshot {
        let mut snapshot = crate::service::QueueSnapshot::default();
        let mut status_counts: HashMap<&'static str, usize> = HashMap::new();
        let now = std::time::SystemTime::now();

        for entry in self.queue.queue.iter() {
            let info = entry.value();
            snapshot.total += 1;

            *status_counts.entry(info.status.label()).or_insert(0) += 1;
            *snapshot
                .by_domain
                .entry(info.recipient_domain.to_string())
                .or_insert(0) += 1;

            let age_secs = now
                .duration_since(
                    std::time::UNIX_EPOCH
                        + std::time::Duration::from_millis(info.message_id.timestamp_ms()),
                )
                .unwrap_or_default()
                .as_secs();
            snapshot.oldest_message_age_secs = Some(
                snapshot
                    .oldest_message_age_secs
                    .map_or(age_secs, |cur| cur.max(age_secs)),
            );
        }

        snapshot.by_status = status_counts;

        snapshot
    }

    fn update_status(&self, message_id: &SpooledMessageId, status: DeliveryStatus) {
        self.queue.update_status(message_id, status);
    }

    fn set_next_retry_at(
        &self,
        message_id: &SpooledMessageId,
        next_retry_at: std::time::SystemTime,
    ) {
        self.queue.set_next_retry_at(message_id, next_retry_at);
    }

    fn reset_server_index(&self, message_id: &SpooledMessageId) {
        self.queue.reset_server_index(message_id);
    }

    fn remove(&self, message_id: &SpooledMessageId) -> Option<crate::DeliveryInfo> {
        self.queue.remove(message_id)
    }

    fn dns_resolver(&self) -> &Option<Arc<dyn DnsResolver>> {
        &self.dns_resolver
    }

    fn spool(&self) -> &Option<std::sync::Arc<dyn BackingStore>> {
        &self.spool
    }

    fn domains(&self) -> &DomainConfigRegistry {
        &self.domains
    }

    fn outbound_bindings(&self) -> crate::OutboundBindingRegistry {
        self.outbound_bindings.clone()
    }

    fn binding_health_status(&self, name: &str) -> Option<crate::BindingHealthStatus> {
        self.binding_health_status_for(name)
    }

    fn binding_enabled(&self, name: &str) -> bool {
        Self::binding_enabled(self, name)
    }

    fn set_binding_enabled(&self, name: &str, enabled: bool) -> Result<(), String> {
        Self::set_binding_enabled(self, name, enabled)
    }

    fn request_process_now(&self) {
        Self::request_process_now(self);
    }

    fn connection_stats(&self) -> crate::types::ConnectionStats {
        crate::types::ConnectionStats {
            queue_size: self.queue.len(),
            rate_limit_domains: self
                .rate_limiter
                .as_ref()
                .map_or(0, crate::rate_limiter::RateLimiter::bucket_count),
        }
    }

    fn update_rate_limit(
        &self,
        domain: Option<String>,
        messages_per_second: f64,
        burst: u32,
    ) -> Result<(), String> {
        let limiter = self
            .rate_limiter
            .as_ref()
            .ok_or_else(|| "Rate limiter not initialized".to_string())?;
        limiter.update_limits(domain.as_deref(), messages_per_second, burst)
    }

    fn replace_rate_limit_config(&self, config: crate::rate_limiter::RateLimitConfig) {
        if let Some(limiter) = &self.rate_limiter {
            limiter.replace_config(config);
        }
    }

    fn replace_retry_policy(&self, policy: crate::policy::RetryPolicy) {
        *self.retry_policy.write() = policy;
    }

    fn replace_smtp_timeouts(&self, timeouts: crate::types::SmtpTimeouts) {
        *self.smtp_timeouts.write() = timeouts;
    }

    fn replace_circuit_breaker_config(&self, config: crate::circuit_breaker::CircuitBreakerConfig) {
        *self.circuit_breaker.write() = config.clone();
        if let Some(breaker) = &self.circuit_breaker_instance {
            breaker.replace_config(config);
        }
    }

    fn replace_domains(&self, registry: crate::DomainConfigRegistry) {
        // Clear current domains and replace with new registry contents
        self.domains.clear();
        for (domain, config) in registry.iter() {
            self.domains.insert(domain, config);
        }
    }

    fn tls_policy(&self) -> crate::policy::TlsPolicy {
        self.tls_policy
    }
}
