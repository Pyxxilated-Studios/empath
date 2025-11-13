//! DNS resolver metrics
//!
//! Tracks DNS resolution performance including:
//! - Lookup durations by query type
//! - Cache hit/miss rates
//! - DNS errors

use std::sync::atomic::{AtomicU64, Ordering};

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

    /// Total number of cache hits
    cache_hits: Counter<u64>,

    /// Total number of cache misses
    cache_misses: Counter<u64>,

    /// Total number of DNS errors by type
    errors_total: Counter<u64>,

    /// Current cache size
    cache_size: AtomicU64,

    /// Total number of cache evictions
    cache_evictions: Counter<u64>,
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

        let cache_hits = meter
            .u64_counter("empath.dns.cache.hits.total")
            .with_description("Total number of DNS cache hits")
            .build();

        let cache_misses = meter
            .u64_counter("empath.dns.cache.misses.total")
            .with_description("Total number of DNS cache misses")
            .build();

        let errors_total = meter
            .u64_counter("empath.dns.errors.total")
            .with_description("Total number of DNS errors by type")
            .build();

        let cache_evictions = meter
            .u64_counter("empath.dns.cache.evictions.total")
            .with_description("Total number of DNS cache evictions")
            .build();

        Ok(Self {
            lookup_duration,
            lookups_total,
            cache_hits,
            cache_misses,
            errors_total,
            cache_size: AtomicU64::new(0),
            cache_evictions,
        })
    }

    /// Record a DNS lookup
    pub fn record_lookup(&self, query_type: &str, duration_secs: f64) {
        let attributes = [KeyValue::new("query_type", query_type.to_string())];
        self.lookup_duration.record(duration_secs, &attributes);
        self.lookups_total.add(1, &attributes);
    }

    /// Record a cache hit
    pub fn record_cache_hit(&self, query_type: &str) {
        let attributes = [KeyValue::new("query_type", query_type.to_string())];
        self.cache_hits.add(1, &attributes);
    }

    /// Record a cache miss
    pub fn record_cache_miss(&self, query_type: &str) {
        let attributes = [KeyValue::new("query_type", query_type.to_string())];
        self.cache_misses.add(1, &attributes);
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
        self.cache_evictions.add(1, &[]);
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
