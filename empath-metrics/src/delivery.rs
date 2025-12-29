//! Delivery processor metrics
//!
//! Tracks outbound mail delivery including:
//! - Delivery attempts by status (success/failure)
//! - Delivery durations by domain
//! - Queue sizes by status
//! - Active SMTP client connections

use std::{
    collections::HashSet,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use dashmap::DashMap;
use opentelemetry::{
    KeyValue,
    metrics::{Counter, Gauge, Histogram, Meter, UpDownCounter},
};

use crate::util::u64_to_f64;

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

    /// Total number of deliveries delayed by rate limiting
    rate_limited_total: Counter<u64>,

    /// Distribution of rate limit delay durations
    rate_limit_delay_seconds: Histogram<f64>,

    /// Total number of circuit breaker trips (Open state entered)
    circuit_breaker_trips_total: Counter<u64>,

    /// Total number of circuit breaker recoveries (Closed state resumed)
    circuit_breaker_recoveries_total: Counter<u64>,

    /// Total number of outbound binding selections/usages
    binding_selected_total: Counter<u64>,
    /// Distribution of outbound binding connect durations
    binding_connect_duration_seconds: Histogram<f64>,
    /// Distribution of outbound binding EHLO durations
    binding_ehlo_duration_seconds: Histogram<f64>,
    /// Current outbound binding health state (0=Closed, 1=HalfOpen, 2=Open)
    binding_health_state: Gauge<u64>,

    /// Total number of SMTP client pool hits
    smtp_pool_hits_total: Counter<u64>,
    /// Total number of SMTP client pool misses
    smtp_pool_misses_total: Counter<u64>,
    /// Total number of TLS handshake failures
    tls_handshake_failures_total: Counter<u64>,
    /// Current SMTP client pool size per domain
    smtp_pool_sizes: Arc<DashMap<String, Arc<AtomicU64>>>,

    // Fast atomic counters for hot path (read by observable counters via callbacks)
    messages_delivered_count: Arc<AtomicU64>,
    messages_failed_count: Arc<AtomicU64>,
    messages_retrying_count: Arc<AtomicU64>,

    // Per-domain circuit breaker state (0=Closed, 1=Open, 2=HalfOpen)
    circuit_breaker_states: Arc<DashMap<String, Arc<AtomicU64>>>,

    /// Known outbound binding names for label validation
    binding_names: Arc<DashMap<String, ()>>,

    // Local counters for queue size tracking (shared with observable gauge callback)
    queue_pending: Arc<AtomicU64>,
    queue_in_progress: Arc<AtomicU64>,
    queue_completed: Arc<AtomicU64>,
    queue_failed: Arc<AtomicU64>,
    queue_retry: Arc<AtomicU64>,
    queue_expired: Arc<AtomicU64>,
    active_conn_count: AtomicU64,

    // Cardinality limiting for domain labels
    /// Maximum number of unique domains to track
    max_domains: u64,
    /// Domains that always bypass the cardinality limit
    high_priority_domains: HashSet<String>,
    /// Currently tracked domains (up to `max_domains`)
    /// Lock-free concurrent map prevents panic on lock poisoning
    tracked_domains: Arc<DashMap<String, ()>>,
    /// Atomic counter for tracked regular domains (excludes high-priority domains)
    /// Used for lock-free cardinality limiting
    tracked_domains_count: Arc<AtomicU64>,
    /// Counter for domains bucketed into "other"
    bucketed_domains_count: Arc<AtomicU64>,
}

struct DeliveryCounters {
    delivered: Arc<AtomicU64>,
    failed: Arc<AtomicU64>,
    retrying: Arc<AtomicU64>,
}

impl DeliveryCounters {
    fn new() -> Self {
        Self {
            delivered: Arc::new(AtomicU64::new(0)),
            failed: Arc::new(AtomicU64::new(0)),
            retrying: Arc::new(AtomicU64::new(0)),
        }
    }
}

struct DeliveryQueueCounters {
    pending: Arc<AtomicU64>,
    in_progress: Arc<AtomicU64>,
    completed: Arc<AtomicU64>,
    failed: Arc<AtomicU64>,
    retry: Arc<AtomicU64>,
    expired: Arc<AtomicU64>,
}

impl DeliveryQueueCounters {
    fn new() -> Self {
        Self {
            pending: Arc::new(AtomicU64::new(0)),
            in_progress: Arc::new(AtomicU64::new(0)),
            completed: Arc::new(AtomicU64::new(0)),
            failed: Arc::new(AtomicU64::new(0)),
            retry: Arc::new(AtomicU64::new(0)),
            expired: Arc::new(AtomicU64::new(0)),
        }
    }
}

struct DeliveryPoolState {
    smtp_pool_sizes: Arc<DashMap<String, Arc<AtomicU64>>>,
    circuit_breaker_states: Arc<DashMap<String, Arc<AtomicU64>>>,
    oldest_message_seconds: Arc<AtomicU64>,
}

impl DeliveryPoolState {
    fn new() -> Self {
        Self {
            smtp_pool_sizes: Arc::new(DashMap::<String, Arc<AtomicU64>>::new()),
            circuit_breaker_states: Arc::new(DashMap::<String, Arc<AtomicU64>>::new()),
            oldest_message_seconds: Arc::new(AtomicU64::new(0)),
        }
    }
}

struct DeliveryInstruments {
    attempts_total: Counter<u64>,
    duration_seconds: Histogram<f64>,
    retry_count: Histogram<u64>,
    queue_age_seconds: Histogram<f64>,
    rate_limited_total: Counter<u64>,
    rate_limit_delay_seconds: Histogram<f64>,
    binding_selected_total: Counter<u64>,
    binding_connect_duration_seconds: Histogram<f64>,
    binding_ehlo_duration_seconds: Histogram<f64>,
    binding_health_state: Gauge<u64>,
    smtp_pool_hits_total: Counter<u64>,
    smtp_pool_misses_total: Counter<u64>,
    tls_handshake_failures_total: Counter<u64>,
    circuit_breaker_trips_total: Counter<u64>,
    circuit_breaker_recoveries_total: Counter<u64>,
    active_connections: UpDownCounter<i64>,
}

impl DeliveryMetrics {
    /// Create a new delivery metrics collector
    ///
    /// # Arguments
    ///
    /// * `max_domains` - Maximum number of unique domains to track before bucketing
    /// * `high_priority_domains` - Domains that always bypass the cardinality limit
    pub fn new(max_domains: u64, high_priority_domains: Vec<String>) -> Self {
        let meter = meter();
        let counters = DeliveryCounters::new();
        let queue = DeliveryQueueCounters::new();
        let pools = DeliveryPoolState::new();
        let instruments = Self::build_instruments(&meter);

        Self::register_message_observables(&meter, &counters);
        Self::register_error_rate_observables(&meter, &counters);
        Self::register_queue_observables(&meter, &queue);
        Self::register_pool_observables(&meter, &pools);

        let bucketed_domains_ref = Arc::new(AtomicU64::new(0));
        Self::register_cardinality_observable(&meter, &bucketed_domains_ref);

        let high_priority_set: HashSet<String> = high_priority_domains.into_iter().collect();

        Self {
            attempts_total: instruments.attempts_total,
            duration_seconds: instruments.duration_seconds,
            active_connections: instruments.active_connections,
            retry_count: instruments.retry_count,
            queue_age_seconds: instruments.queue_age_seconds,
            oldest_message_seconds: pools.oldest_message_seconds,
            rate_limited_total: instruments.rate_limited_total,
            rate_limit_delay_seconds: instruments.rate_limit_delay_seconds,
            binding_selected_total: instruments.binding_selected_total,
            binding_connect_duration_seconds: instruments.binding_connect_duration_seconds,
            binding_ehlo_duration_seconds: instruments.binding_ehlo_duration_seconds,
            binding_health_state: instruments.binding_health_state,
            smtp_pool_hits_total: instruments.smtp_pool_hits_total,
            smtp_pool_misses_total: instruments.smtp_pool_misses_total,
            tls_handshake_failures_total: instruments.tls_handshake_failures_total,
            smtp_pool_sizes: pools.smtp_pool_sizes,
            circuit_breaker_trips_total: instruments.circuit_breaker_trips_total,
            circuit_breaker_recoveries_total: instruments.circuit_breaker_recoveries_total,
            messages_delivered_count: counters.delivered,
            messages_failed_count: counters.failed,
            messages_retrying_count: counters.retrying,
            circuit_breaker_states: pools.circuit_breaker_states,
            queue_pending: queue.pending,
            queue_in_progress: queue.in_progress,
            queue_completed: queue.completed,
            queue_failed: queue.failed,
            queue_retry: queue.retry,
            queue_expired: queue.expired,
            active_conn_count: AtomicU64::new(0),
            max_domains,
            high_priority_domains: high_priority_set,
            tracked_domains: Arc::new(DashMap::new()),
            tracked_domains_count: Arc::new(AtomicU64::new(0)),
            bucketed_domains_count: bucketed_domains_ref,
            binding_names: Arc::new(DashMap::new()),
        }
    }

    fn build_instruments(meter: &Meter) -> DeliveryInstruments {
        let attempts_total = meter
            .u64_counter("empath.delivery.attempts.total")
            .with_description("Total number of delivery attempts by status")
            .build();

        let duration_seconds = meter
            .f64_histogram("empath.delivery.duration.seconds")
            .with_description("Distribution of delivery durations by domain")
            .build();

        let retry_count = meter
            .u64_histogram("empath.delivery.retry.count")
            .with_description("Distribution of retry counts before success")
            .build();

        let queue_age_seconds = meter
            .f64_histogram("empath.delivery.queue.age.seconds")
            .with_description("Distribution of queue age (time between spool and delivery attempt)")
            .build();

        let rate_limited_total = meter
            .u64_counter("empath.delivery.rate_limited.total")
            .with_description("Total number of deliveries delayed by rate limiting")
            .build();

        let rate_limit_delay_seconds = meter
            .f64_histogram("empath.delivery.rate_limit.delay.seconds")
            .with_description("Distribution of rate limit delay durations")
            .build();

        let binding_selected_total = meter
            .u64_counter("empath.delivery.bindings.selected.total")
            .with_description("Total number of deliveries using each outbound binding")
            .build();
        let binding_connect_duration_seconds = meter
            .f64_histogram("empath.delivery.bindings.connect.duration.seconds")
            .with_description("Distribution of outbound SMTP connect durations by binding")
            .build();
        let binding_ehlo_duration_seconds = meter
            .f64_histogram("empath.delivery.bindings.ehlo.duration.seconds")
            .with_description("Distribution of outbound EHLO durations by binding")
            .build();
        let binding_health_state = meter
            .u64_gauge("empath.delivery.bindings.health.state")
            .with_description("Outbound binding health state (0=Closed, 1=HalfOpen, 2=Open)")
            .build();

        let smtp_pool_hits_total = meter
            .u64_counter("empath.delivery.smtp_pool.hits.total")
            .with_description("Total number of outbound SMTP pool hits by domain")
            .build();
        let smtp_pool_misses_total = meter
            .u64_counter("empath.delivery.smtp_pool.misses.total")
            .with_description("Total number of outbound SMTP pool misses by domain")
            .build();

        let tls_handshake_failures_total = meter
            .u64_counter("empath.delivery.tls.handshake_failures.total")
            .with_description("Total number of TLS handshake failures by domain and policy")
            .build();

        let circuit_breaker_trips_total = meter
            .u64_counter("empath.delivery.circuit_breaker.trips.total")
            .with_description(
                "Total number of circuit breaker trips (Open state entered) by domain",
            )
            .build();

        let circuit_breaker_recoveries_total = meter
            .u64_counter("empath.delivery.circuit_breaker.recoveries.total")
            .with_description(
                "Total number of circuit breaker recoveries (Closed state resumed) by domain",
            )
            .build();

        let active_connections = meter
            .i64_up_down_counter("empath.delivery.connections.active")
            .with_description("Number of currently active outbound SMTP connections")
            .build();

        DeliveryInstruments {
            attempts_total,
            duration_seconds,
            retry_count,
            queue_age_seconds,
            rate_limited_total,
            rate_limit_delay_seconds,
            binding_selected_total,
            binding_connect_duration_seconds,
            binding_ehlo_duration_seconds,
            binding_health_state,
            smtp_pool_hits_total,
            smtp_pool_misses_total,
            tls_handshake_failures_total,
            circuit_breaker_trips_total,
            circuit_breaker_recoveries_total,
            active_connections,
        }
    }

    fn register_message_observables(meter: &Meter, counters: &DeliveryCounters) {
        let delivered_clone = counters.delivered.clone();
        meter
            .u64_observable_counter("empath.delivery.messages.delivered.total")
            .with_description("Total number of messages delivered successfully")
            .with_callback(move |observer| {
                observer.observe(delivered_clone.load(Ordering::Relaxed), &[]);
            })
            .build();

        let failed_clone = counters.failed.clone();
        meter
            .u64_observable_counter("empath.delivery.messages.failed.total")
            .with_description("Total number of messages permanently failed")
            .with_callback(move |observer| {
                observer.observe(failed_clone.load(Ordering::Relaxed), &[]);
            })
            .build();

        let retrying_clone = counters.retrying.clone();
        meter
            .u64_observable_counter("empath.delivery.messages.retrying.total")
            .with_description("Total number of messages retrying")
            .with_callback(move |observer| {
                observer.observe(retrying_clone.load(Ordering::Relaxed), &[]);
            })
            .build();
    }

    fn register_error_rate_observables(meter: &Meter, counters: &DeliveryCounters) {
        let delivered_for_error_rate = counters.delivered.clone();
        let failed_for_error_rate = counters.failed.clone();
        let retrying_for_error_rate = counters.retrying.clone();
        meter
            .f64_observable_gauge("empath.delivery.error_rate")
            .with_description("Delivery error rate (failed / total attempts, 0-1)")
            .with_callback(move |observer| {
                let delivered = delivered_for_error_rate.load(Ordering::Relaxed);
                let failed = failed_for_error_rate.load(Ordering::Relaxed);
                let retrying = retrying_for_error_rate.load(Ordering::Relaxed);
                let total = delivered + failed + retrying;

                let error_rate = if total > 0 {
                    u64_to_f64(failed) / u64_to_f64(total)
                } else {
                    0.0
                };

                observer.observe(error_rate, &[]);
            })
            .build();

        let delivered_for_success_rate = counters.delivered.clone();
        let failed_for_success_rate = counters.failed.clone();
        let retrying_for_success_rate = counters.retrying.clone();
        meter
            .f64_observable_gauge("empath.delivery.success_rate")
            .with_description("Delivery success rate (delivered / total attempts, 0-1)")
            .with_callback(move |observer| {
                let delivered = delivered_for_success_rate.load(Ordering::Relaxed);
                let failed = failed_for_success_rate.load(Ordering::Relaxed);
                let retrying = retrying_for_success_rate.load(Ordering::Relaxed);
                let total = delivered + failed + retrying;

                let success_rate = if total > 0 {
                    u64_to_f64(delivered) / u64_to_f64(total)
                } else {
                    0.0
                };

                observer.observe(success_rate, &[]);
            })
            .build();
    }

    fn register_pool_observables(meter: &Meter, pools: &DeliveryPoolState) {
        let smtp_pool_sizes_clone = pools.smtp_pool_sizes.clone();
        meter
            .u64_observable_gauge("empath.delivery.smtp_pool.size")
            .with_description("Current outbound SMTP pool size by domain")
            .with_callback(move |observer| {
                for entry in smtp_pool_sizes_clone.iter() {
                    let domain = entry.key();
                    let size = entry.value().load(Ordering::Relaxed);
                    observer.observe(size, &[KeyValue::new("domain", domain.clone())]);
                }
            })
            .build();

        let circuit_states_clone = pools.circuit_breaker_states.clone();
        meter
            .u64_observable_gauge("empath.delivery.circuit_breaker.state")
            .with_description(
                "Current circuit breaker state by domain (0=Closed, 1=Open, 2=HalfOpen)",
            )
            .with_callback(move |observer| {
                for entry in circuit_states_clone.iter() {
                    let domain = entry.key();
                    let state = entry.value().load(Ordering::Relaxed);
                    observer.observe(state, &[KeyValue::new("domain", domain.clone())]);
                }
            })
            .build();

        let oldest_clone = pools.oldest_message_seconds.clone();
        meter
            .u64_observable_gauge("empath.delivery.queue.oldest.seconds")
            .with_description("Age of the oldest pending message in the queue")
            .with_callback(move |observer| {
                observer.observe(oldest_clone.load(Ordering::Relaxed), &[]);
            })
            .build();
    }

    fn register_queue_observables(meter: &Meter, queue: &DeliveryQueueCounters) {
        let pending = queue.pending.clone();
        let in_progress = queue.in_progress.clone();
        let completed = queue.completed.clone();
        let failed = queue.failed.clone();
        let retry = queue.retry.clone();
        let expired = queue.expired.clone();

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
    }

    fn register_cardinality_observable(meter: &Meter, bucketed_domains: &Arc<AtomicU64>) {
        let bucketed_clone = bucketed_domains.clone();
        meter
            .u64_observable_gauge("empath.delivery.domain.cardinality")
            .with_description("Number of unique domains currently tracked in metrics")
            .with_callback(move |observer| {
                observer.observe(
                    bucketed_clone.load(Ordering::Relaxed),
                    &[KeyValue::new("type", "bucketed")],
                );
            })
            .build();
    }

    /// Register configured outbound bindings for metrics labeling.
    pub fn register_bindings(&self, bindings: Vec<String>) {
        for name in bindings {
            self.binding_names.insert(name, ());
        }
    }

    /// Record a binding selection or usage.
    pub fn record_binding_selected(&self, binding: Option<&str>) {
        let label = self.binding_label(binding);
        self.binding_selected_total
            .add(1, &[KeyValue::new("binding", label)]);
    }

    pub fn record_binding_connect_duration(&self, binding: Option<&str>, duration: Duration) {
        let label = self.binding_label(binding);
        self.binding_connect_duration_seconds
            .record(duration.as_secs_f64(), &[KeyValue::new("binding", label)]);
    }

    pub fn record_binding_ehlo_duration(&self, binding: Option<&str>, duration: Duration) {
        let label = self.binding_label(binding);
        self.binding_ehlo_duration_seconds
            .record(duration.as_secs_f64(), &[KeyValue::new("binding", label)]);
    }

    pub fn record_binding_health_state(&self, binding: &str, state: u64) {
        let label = self.binding_label(Some(binding));
        self.binding_health_state
            .record(state, &[KeyValue::new("binding", label)]);
    }

    pub fn record_smtp_pool_hit(&self, domain: &str) {
        let label = self.bucket_domain(domain);
        self.smtp_pool_hits_total
            .add(1, &[KeyValue::new("domain", label)]);
    }

    pub fn record_smtp_pool_miss(&self, domain: &str) {
        let label = self.bucket_domain(domain);
        self.smtp_pool_misses_total
            .add(1, &[KeyValue::new("domain", label)]);
    }

    pub fn update_smtp_pool_size(&self, domain: &str, size: u64) {
        let label = self.bucket_domain(domain);
        self.smtp_pool_sizes
            .entry(label)
            .or_insert_with(|| Arc::new(AtomicU64::new(0)))
            .store(size, Ordering::Relaxed);
    }

    fn binding_label(&self, binding: Option<&str>) -> String {
        match binding {
            Some(name)
                if self.binding_names.is_empty() || self.binding_names.contains_key(name) =>
            {
                name.to_string()
            }
            Some(_) => "other".to_string(),
            None => "none".to_string(),
        }
    }

    /// Bucket a domain name to limit cardinality
    ///
    /// High-cardinality domain labels can create thousands of metric series which impacts
    /// Prometheus memory and query performance. This method implements cardinality limiting
    /// by bucketing domains into an "other" category once the limit is reached.
    ///
    /// # Algorithm
    ///
    /// 1. If domain is in `high_priority_domains`, always return it (bypass limit)
    /// 2. If domain is already being tracked, return it
    /// 3. If we haven't reached `max_domains` yet, start tracking this domain
    /// 4. Otherwise, return "other" and increment bucketed counter
    ///
    /// # Arguments
    ///
    /// * `domain` - The domain name to potentially bucket
    ///
    /// # Returns
    ///
    /// Either the original domain name or "other" if the cardinality limit is reached
    fn bucket_domain(&self, domain: &str) -> String {
        // High-priority domains always bypass the limit
        if self.high_priority_domains.contains(domain) {
            return domain.to_string();
        }

        // Lock-free check if we're already tracking this domain
        if self.tracked_domains.contains_key(domain) {
            return domain.to_string();
        }

        // Atomically try to reserve a slot for this domain using compare-and-swap
        // This prevents the race condition where multiple threads check len() < max_domains
        // and all insert, causing the limit to be exceeded
        let mut current = self.tracked_domains_count.load(Ordering::Acquire);
        loop {
            // Check if we've reached the limit
            if current >= self.max_domains {
                // Limit reached - bucket into "other"
                self.bucketed_domains_count.fetch_add(1, Ordering::Relaxed);
                return "other".to_string();
            }

            // Try to atomically increment the counter to reserve a slot
            // If another thread incremented it, we'll retry with the new value
            match self.tracked_domains_count.compare_exchange_weak(
                current,
                current + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    // Successfully reserved a slot - now insert the domain
                    // insert() returns Some(old_value) if the key already existed
                    if self
                        .tracked_domains
                        .insert(domain.to_string(), ())
                        .is_some()
                    {
                        // Another thread inserted this domain first - unreserve our slot
                        self.tracked_domains_count.fetch_sub(1, Ordering::Release);
                    }
                    return domain.to_string();
                }
                Err(actual) => {
                    // Another thread modified the counter - retry with the actual value
                    current = actual;
                }
            }
        }
    }

    /// Record a delivery attempt
    pub fn record_attempt(&self, status: &str, domain: &str) {
        let bucketed_domain = self.bucket_domain(domain);
        let attributes = [
            KeyValue::new("status", status.to_string()),
            KeyValue::new("domain", bucketed_domain),
        ];
        self.attempts_total.add(1, &attributes);
    }

    /// Record a successful delivery
    pub fn record_delivery_success(&self, domain: &str, duration_secs: f64, retry_count: u64) {
        let bucketed_domain = self.bucket_domain(domain);
        let attributes = [KeyValue::new("domain", bucketed_domain)];
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

    /// Record a permanent failure
    pub fn record_permanent_failure(&self, domain: &str) {
        self.record_attempt("permanent_failure", domain);
    }

    /// Record a TLS handshake failure
    pub fn record_tls_handshake_failure(&self, domain: &str, policy: &str) {
        let bucketed_domain = self.bucket_domain(domain);
        let attributes = [
            KeyValue::new("domain", bucketed_domain),
            KeyValue::new("policy", policy.to_string()),
        ];
        self.tls_handshake_failures_total.add(1, &attributes);
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

    /// Get the number of domains that have been bucketed into "other"
    ///
    /// This counter increments each time a delivery attempt is made to a domain
    /// that exceeds the cardinality limit. Use this to monitor cardinality pressure.
    #[must_use]
    pub fn bucketed_domains_count(&self) -> u64 {
        self.bucketed_domains_count.load(Ordering::Relaxed)
    }

    /// Get the number of domains currently being tracked
    ///
    /// This does not include high-priority domains that bypass the limit.
    #[must_use]
    pub fn tracked_domains_count(&self) -> usize {
        self.tracked_domains.len()
    }

    /// Record a rate limiting event
    ///
    /// # Arguments
    ///
    /// * `domain` - The domain that was rate limited
    /// * `delay_secs` - The delay duration in seconds before the next retry
    pub fn record_rate_limit(&self, domain: &str, delay_secs: f64) {
        let bucketed_domain = self.bucket_domain(domain);
        let attributes = [KeyValue::new("domain", bucketed_domain)];
        self.rate_limited_total.add(1, &attributes);
        self.rate_limit_delay_seconds
            .record(delay_secs, &attributes);
    }

    /// Record a circuit breaker trip (Open state entered)
    ///
    /// # Arguments
    ///
    /// * `domain` - The domain whose circuit breaker tripped
    pub fn record_circuit_breaker_trip(&self, domain: &str) {
        let bucketed_domain = self.bucket_domain(domain);
        let attributes = [KeyValue::new("domain", bucketed_domain.clone())];
        self.circuit_breaker_trips_total.add(1, &attributes);

        // Update circuit state to Open (1)
        self.circuit_breaker_states
            .entry(bucketed_domain)
            .or_insert_with(|| Arc::new(AtomicU64::new(0)))
            .store(1, Ordering::Relaxed);
    }

    /// Record a circuit breaker recovery (Closed state resumed)
    ///
    /// # Arguments
    ///
    /// * `domain` - The domain whose circuit breaker recovered
    pub fn record_circuit_breaker_recovery(&self, domain: &str) {
        let bucketed_domain = self.bucket_domain(domain);
        let attributes = [KeyValue::new("domain", bucketed_domain.clone())];
        self.circuit_breaker_recoveries_total.add(1, &attributes);

        // Update circuit state to Closed (0)
        self.circuit_breaker_states
            .entry(bucketed_domain)
            .or_insert_with(|| Arc::new(AtomicU64::new(0)))
            .store(0, Ordering::Relaxed);
    }

    /// Update circuit breaker state
    ///
    /// # Arguments
    ///
    /// * `domain` - The domain whose circuit breaker state changed
    /// * `state` - The new state (0=Closed, 1=Open, 2=HalfOpen)
    pub fn update_circuit_breaker_state(&self, domain: &str, state: u64) {
        let bucketed_domain = self.bucket_domain(domain);
        self.circuit_breaker_states
            .entry(bucketed_domain)
            .or_insert_with(|| Arc::new(AtomicU64::new(0)))
            .store(state, Ordering::Relaxed);
    }
}

/// Get the OpenTelemetry meter for delivery metrics
fn meter() -> Meter {
    opentelemetry::global::meter("empath.delivery")
}
