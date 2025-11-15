//! DNS resolver metrics
//!
//! Tracks DNS resolution performance including:
//! - Lookup durations by query type
//! - Cache hit/miss rates
//! - DNS errors

use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use opentelemetry::{
    KeyValue,
    metrics::{Counter, Histogram, Meter},
};

use crate::MetricsError;

/// DNS metrics collector
#[derive(Debug)]
pub struct DnsMetrics {
    /// Distribution of DNS lookup durations in seconds
    lookup_duration: Histogram<f64>,

    /// Total number of DNS lookups by query type
    lookups_total: Counter<u64>,

    /// Total number of DNS errors by type
    errors_total: Counter<u64>,

    /// Current cache size
    cache_size: AtomicU64,

    // Fast atomic counters for hot path (read by observable counters via callbacks)
    cache_hits_count: Arc<AtomicU64>,
    cache_misses_count: Arc<AtomicU64>,
    cache_evictions_count: Arc<AtomicU64>,
}

impl DnsMetrics {
    /// Create a new DNS metrics collector
    ///
    /// # Errors
    ///
    /// Returns an error if metric instruments cannot be created.
    pub fn new() -> Result<Self, MetricsError> {
        let meter = meter();

        let lookup_duration = meter
            .f64_histogram("empath.dns.lookup.duration.seconds")
            .with_description("Distribution of DNS lookup durations")
            .build();

        let lookups_total = meter
            .u64_counter("empath.dns.lookups.total")
            .with_description("Total number of DNS lookups by query type")
            .build();

        // Create atomic counters for high-frequency metrics
        let cache_hits_ref = Arc::new(AtomicU64::new(0));
        let cache_misses_ref = Arc::new(AtomicU64::new(0));
        let cache_evictions_ref = Arc::new(AtomicU64::new(0));

        // Observable counter for cache hits (reads from atomic)
        // Meter keeps this alive internally via callback
        let hits_clone = cache_hits_ref.clone();
        meter
            .u64_observable_counter("empath.dns.cache.hits.total")
            .with_description("Total number of DNS cache hits")
            .with_callback(move |observer| {
                observer.observe(hits_clone.load(Ordering::Relaxed), &[]);
            })
            .build();

        // Observable counter for cache misses (reads from atomic)
        // Meter keeps this alive internally via callback
        let misses_clone = cache_misses_ref.clone();
        meter
            .u64_observable_counter("empath.dns.cache.misses.total")
            .with_description("Total number of DNS cache misses")
            .with_callback(move |observer| {
                observer.observe(misses_clone.load(Ordering::Relaxed), &[]);
            })
            .build();

        let errors_total = meter
            .u64_counter("empath.dns.errors.total")
            .with_description("Total number of DNS errors by type")
            .build();

        // Observable counter for cache evictions (reads from atomic)
        // Meter keeps this alive internally via callback
        let evictions_clone = cache_evictions_ref.clone();
        meter
            .u64_observable_counter("empath.dns.cache.evictions.total")
            .with_description("Total number of DNS cache evictions")
            .with_callback(move |observer| {
                observer.observe(evictions_clone.load(Ordering::Relaxed), &[]);
            })
            .build();

        Ok(Self {
            lookup_duration,
            lookups_total,
            errors_total,
            cache_size: AtomicU64::new(0),
            cache_hits_count: cache_hits_ref,
            cache_misses_count: cache_misses_ref,
            cache_evictions_count: cache_evictions_ref,
        })
    }

    /// Record a DNS lookup
    pub fn record_lookup(&self, query_type: &str, duration_secs: f64) {
        let attributes = [KeyValue::new("query_type", query_type.to_string())];
        self.lookup_duration.record(duration_secs, &attributes);
        self.lookups_total.add(1, &attributes);
    }

    /// Record a cache hit
    pub fn record_cache_hit(&self, _query_type: &str) {
        // Fast atomic increment instead of Counter::add() (80-120ns → <10ns)
        // Note: query_type attribute removed for performance - total hits tracked only
        self.cache_hits_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a cache miss
    pub fn record_cache_miss(&self, _query_type: &str) {
        // Fast atomic increment instead of Counter::add() (80-120ns → <10ns)
        // Note: query_type attribute removed for performance - total misses tracked only
        self.cache_misses_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a DNS error
    pub fn record_error(&self, error_type: &str) {
        let attributes = [KeyValue::new("error_type", error_type.to_string())];
        self.errors_total.add(1, &attributes);
    }

    /// Update the cache size
    pub fn set_cache_size(&self, size: u64) {
        self.cache_size.store(size, Ordering::Relaxed);
    }

    /// Record a cache eviction
    pub fn record_cache_eviction(&self) {
        // Fast atomic increment instead of Counter::add() (80-120ns → <10ns)
        self.cache_evictions_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Get the current cache size
    #[must_use]
    pub fn get_cache_size(&self) -> u64 {
        self.cache_size.load(Ordering::Relaxed)
    }
}

/// Get the OpenTelemetry meter for DNS metrics
fn meter() -> Meter {
    opentelemetry::global::meter("empath.dns")
}
