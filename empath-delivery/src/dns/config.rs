//! DNS resolver configuration

use serde::Deserialize;

/// Configuration for DNS resolver.
#[derive(Debug, Clone, Deserialize)]
pub struct DnsConfig {
    /// DNS query timeout in seconds (default: 5)
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,

    /// Cache TTL override in seconds (optional)
    /// If set, overrides the DNS record's TTL for all cached entries
    /// If not set, uses the actual DNS record TTL (recommended)
    #[serde(default)]
    pub cache_ttl_secs: Option<u64>,

    /// Minimum cache TTL in seconds (default: 60 = 1 minute)
    /// Prevents excessive DNS queries for records with very short TTLs
    #[serde(default = "default_min_cache_ttl_secs")]
    pub min_cache_ttl_secs: u64,

    /// Maximum cache TTL in seconds (default: 3600 = 1 hour)
    /// Ensures eventual refresh even for records with very long TTLs
    #[serde(default = "default_max_cache_ttl_secs")]
    pub max_cache_ttl_secs: u64,

    /// Maximum cache size hint (default: 1000)
    /// Note: With `DashMap`, this is not strictly enforced for performance.
    /// The cache uses lock-free concurrent access for better throughput.
    #[serde(default = "default_cache_size")]
    pub cache_size: usize,

    /// Enable DNSSEC validation in the resolver (default: false).
    #[serde(default)]
    pub dnssec_enabled: bool,

    /// Fail lookups on DNSSEC bogus/indeterminate results (default: false).
    #[serde(default)]
    pub dnssec_strict: bool,
}

const fn default_timeout_secs() -> u64 {
    5
}

const fn default_min_cache_ttl_secs() -> u64 {
    60 // 1 minute
}

const fn default_max_cache_ttl_secs() -> u64 {
    3600 // 1 hour
}

const fn default_cache_size() -> usize {
    1000
}

impl Default for DnsConfig {
    fn default() -> Self {
        Self {
            timeout_secs: default_timeout_secs(),
            cache_ttl_secs: None,
            min_cache_ttl_secs: default_min_cache_ttl_secs(),
            max_cache_ttl_secs: default_max_cache_ttl_secs(),
            cache_size: default_cache_size(),
            dnssec_enabled: false,
            dnssec_strict: false,
        }
    }
}
