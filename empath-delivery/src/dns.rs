//! DNS resolution for mail delivery.
//!
//! Implements MX record lookups with A/AAAA fallback per RFC 5321 section 5.1.
//! Includes lock-free concurrent caching using DNS record TTLs with configurable bounds.
//!
//! # Caching Strategy
//!
//! - **DNS TTL by default**: Uses the actual TTL from DNS records (respects authoritative server guidance)
//! - **Bounded TTLs**: Applies min (60s) and max (3600s) bounds to prevent extremes
//! - **Optional override**: `cache_ttl_secs` config can override DNS TTL for all entries (useful for testing)
//! - **Lock-free**: `DashMap` provides concurrent access without mutex contention

use std::{
    borrow::Cow,
    sync::Arc,
    time::{Duration, Instant},
};

use dashmap::DashMap;
use empath_common::context::Context;
use empath_ffi::modules;
use hickory_resolver::{
    TokioResolver,
    config::{ResolverConfig, ResolverOpts},
    name_server::TokioConnectionProvider,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, warn};

/// Errors that can occur during DNS resolution.
#[derive(Debug, Error)]
pub enum DnsError {
    /// No MX, A, or AAAA records found for the domain.
    #[error("No mail servers found for domain: {0}")]
    NoMailServers(String),

    /// DNS query failed due to network or resolver issues.
    #[error("DNS lookup failed: {0}")]
    LookupFailed(#[from] hickory_resolver::ResolveError),

    /// Domain does not exist (NXDOMAIN).
    #[error("Domain does not exist: {0}")]
    DomainNotFound(String),

    /// DNS query timed out.
    #[error("DNS query timed out for domain: {0}")]
    Timeout(String),
}

impl DnsError {
    /// Returns `true` if this error is temporary and should be retried.
    #[must_use]
    pub const fn is_temporary(&self) -> bool {
        matches!(self, Self::Timeout(_) | Self::LookupFailed(_))
    }
}

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
        }
    }
}

/// Represents a mail server target with its priority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MailServer {
    /// The hostname or IP address of the mail server.
    pub host: String,
    /// MX priority (lower value = higher priority). 0 for A/AAAA fallback.
    pub priority: u16,
    /// Port number (default: 25).
    pub port: u16,
}

/// Cached DNS result with expiration time.
#[derive(Debug, Clone)]
struct CachedResult {
    /// The resolved mail servers (Arc for cheap cloning on cache hits)
    servers: Arc<Vec<MailServer>>,
    /// When this cache entry expires
    expires_at: Instant,
}

impl MailServer {
    /// Creates a new mail server entry.
    #[must_use]
    pub const fn new(host: String, priority: u16, port: u16) -> Self {
        Self {
            host,
            priority,
            port,
        }
    }

    /// Returns the full address as `host:port`.
    #[must_use]
    pub fn address(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}

/// DNS resolver for mail delivery with concurrent caching.
///
/// Uses `DashMap` for lock-free concurrent cache access, providing better
/// throughput under high load compared to mutex-based caching.
#[derive(Debug)]
pub struct DnsResolver {
    resolver: TokioResolver,
    cache: Arc<DashMap<String, CachedResult>>,
    config: DnsConfig,
}

impl DnsResolver {
    /// Creates a new DNS resolver with default configuration.
    ///
    /// # Errors
    ///
    /// Returns an error if the system DNS configuration cannot be loaded.
    pub fn new() -> Result<Self, DnsError> {
        Self::with_dns_config(DnsConfig::default())
    }

    /// Creates a new DNS resolver with custom DNS configuration.
    ///
    /// # Errors
    ///
    /// Returns an error if the resolver cannot be initialized.
    pub fn with_dns_config(dns_config: DnsConfig) -> Result<Self, DnsError> {
        let mut opts = ResolverOpts::default();
        opts.timeout = Duration::from_secs(dns_config.timeout_secs);

        // Try system DNS configuration first
        let resolver_result = TokioResolver::builder(TokioConnectionProvider::default())
            .map(|builder| builder.with_options(opts.clone()).build());

        let resolver = match resolver_result {
            Ok(r) => r,
            Err(e) => {
                // System DNS failed, use Cloudflare fallback (1.1.1.1, 1.0.0.1)
                warn!(
                    error = %e,
                    "System DNS configuration failed, using Cloudflare fallback (1.1.1.1)"
                );

                TokioResolver::builder_with_config(
                    ResolverConfig::cloudflare(),
                    TokioConnectionProvider::default(),
                )
                .with_options(opts)
                .build()
            }
        };

        let cache = Arc::new(DashMap::new());

        Ok(Self {
            resolver,
            cache,
            config: dns_config,
        })
    }

    /// Creates a new DNS resolver with custom resolver configuration.
    ///
    /// # Errors
    ///
    /// Returns an error if the resolver cannot be initialized.
    pub fn with_resolver_config(
        resolver_config: ResolverConfig,
        opts: ResolverOpts,
        dns_config: DnsConfig,
    ) -> Result<Self, DnsError> {
        let resolver =
            TokioResolver::builder_with_config(resolver_config, TokioConnectionProvider::default())
                .with_options(opts)
                .build();

        let cache = Arc::new(DashMap::new());

        Ok(Self {
            resolver,
            cache,
            config: dns_config,
        })
    }

    /// Resolves mail servers for a domain following RFC 5321 section 5.1.
    ///
    /// 1. Check cache for unexpired entry
    /// 2. Look up MX records and return them sorted by priority (lower = higher priority)
    /// 3. If no MX records exist, fall back to A/AAAA records (implicit MX with priority 0)
    /// 4. Cache the result using DNS record TTL (bounded by min/max config)
    ///
    /// # Errors
    ///
    /// Returns `DnsError` if:
    /// - The domain does not exist
    /// - No mail servers (MX, A, or AAAA) are found
    /// - The DNS query fails or times out
    pub async fn resolve_mail_servers(
        &self,
        domain: &str,
    ) -> Result<Arc<Vec<MailServer>>, DnsError> {
        debug!("Resolving mail servers for domain: {domain}");

        // Check cache first (lock-free read)
        if let Some(cached) = self.cache.get(domain) {
            if cached.expires_at > Instant::now() {
                debug!("Cache hit for {domain}, {} server(s)", cached.servers.len());

                // Dispatch DNS cache hit event
                let mut ctx = Context::default();
                ctx.metadata
                    .insert(Cow::Borrowed("dns_cache_status"), "hit".to_string());
                ctx.metadata
                    .insert(Cow::Borrowed("dns_domain"), domain.to_string());
                ctx.metadata.insert(
                    Cow::Borrowed("dns_cache_size"),
                    self.cache.len().to_string(),
                );
                modules::dispatch(modules::Event::Event(modules::Ev::DnsLookup), &mut ctx);

                return Ok(Arc::clone(&cached.servers));
            }
            debug!("Cache entry expired for {domain}");
        }

        // Cache miss or expired, perform DNS lookup
        let lookup_start = Instant::now();
        let (servers, dns_ttl) = self.resolve_mail_servers_uncached(domain).await?;
        let lookup_duration = lookup_start.elapsed();
        let servers = Arc::new(servers);

        // Determine cache TTL: use override if set, otherwise use DNS TTL with bounds
        let cache_ttl = self.config.cache_ttl_secs.unwrap_or_else(|| {
            // Apply min/max bounds to DNS TTL
            u64::from(dns_ttl).clamp(
                self.config.min_cache_ttl_secs,
                self.config.max_cache_ttl_secs,
            )
        });

        // Cache the result (lock-free write)
        let expires_at = Instant::now() + Duration::from_secs(cache_ttl);
        let cached_result = CachedResult {
            servers: Arc::clone(&servers),
            expires_at,
        };

        self.cache.insert(domain.to_string(), cached_result);

        // Dispatch DNS cache miss event
        let mut ctx = Context::default();
        ctx.metadata
            .insert(Cow::Borrowed("dns_cache_status"), "miss".to_string());
        ctx.metadata
            .insert(Cow::Borrowed("dns_domain"), domain.to_string());
        ctx.metadata.insert(
            Cow::Borrowed("dns_lookup_duration_ms"),
            lookup_duration.as_millis().to_string(),
        );
        ctx.metadata.insert(
            Cow::Borrowed("dns_cache_size"),
            self.cache.len().to_string(),
        );
        modules::dispatch(modules::Event::Event(modules::Ev::DnsLookup), &mut ctx);

        debug!(
            "Cached result for {domain}, DNS TTL: {dns_ttl}s, cache TTL: {cache_ttl}s, {} server(s)",
            servers.len()
        );
        Ok(servers)
    }

    /// Performs uncached DNS lookup for mail servers.
    ///
    /// Returns a tuple of (servers, ttl) where ttl is the minimum TTL from all records.
    async fn resolve_mail_servers_uncached(
        &self,
        domain: &str,
    ) -> Result<(Vec<MailServer>, u32), DnsError> {
        // Try MX lookup first
        match self.resolver.mx_lookup(domain).await {
            Ok(mx_lookup) => {
                // Extract minimum TTL from all MX records
                let min_ttl = mx_lookup
                    .as_lookup()
                    .records()
                    .iter()
                    .map(hickory_resolver::proto::rr::Record::ttl)
                    .min()
                    .unwrap_or(300); // Default to 5 minutes if no TTL found

                let mut servers: Vec<MailServer> = mx_lookup
                    .iter()
                    .map(|mx| {
                        let host = mx.exchange().to_utf8();
                        let priority = mx.preference();
                        debug!("Found MX record: {host} (priority: {priority})");
                        MailServer::new(host, priority, 25)
                    })
                    .collect();

                if servers.is_empty() {
                    debug!("MX lookup returned no records, falling back to A/AAAA");
                    return self.fallback_to_a_aaaa(domain).await;
                }

                // Sort by priority (lower number = higher priority)
                // RFC 5321 Section 5.1: Equal-priority servers should be randomized for load balancing
                servers.sort_by_key(|s| s.priority);

                // Randomize within each priority group
                Self::randomize_equal_priority(&mut servers);

                debug!(
                    "Resolved {} MX record(s) for {domain} with TTL {min_ttl}s",
                    servers.len()
                );
                Ok((servers, min_ttl))
            }
            Err(err) => {
                // Check if this is NoRecordsFound (no MX records exist)
                if err.is_no_records_found() {
                    debug!("No MX records found for {domain}, falling back to A/AAAA");
                    self.fallback_to_a_aaaa(domain).await
                } else {
                    warn!("MX lookup failed for {domain}: {err}");
                    Err(DnsError::LookupFailed(err))
                }
            }
        }
    }

    /// Falls back to A/AAAA records when no MX records exist (RFC 5321).
    ///
    /// Returns IP addresses as implicit MX records with priority 0, along with the minimum TTL.
    async fn fallback_to_a_aaaa(&self, domain: &str) -> Result<(Vec<MailServer>, u32), DnsError> {
        debug!("Attempting A/AAAA fallback for {domain}");

        match self.resolver.lookup_ip(domain).await {
            Ok(ip_lookup) => {
                // Extract minimum TTL from all A/AAAA records
                // Note: clippy suggests using Record::ttl directly, but the path it suggests doesn't exist
                let min_ttl = ip_lookup
                    .as_lookup()
                    .records()
                    .iter()
                    .map(hickory_resolver::proto::rr::Record::ttl)
                    .min()
                    .unwrap_or(300); // Default to 5 minutes if no TTL found

                let servers: Vec<MailServer> = ip_lookup
                    .iter()
                    .map(|ip| {
                        let host = ip.to_string();
                        debug!("Found {}: {host}", if ip.is_ipv6() { "AAAA" } else { "A" });
                        MailServer::new(host, 0, 25)
                    })
                    .collect();

                if servers.is_empty() {
                    Err(DnsError::NoMailServers(domain.to_string()))
                } else {
                    debug!(
                        "Resolved {} A/AAAA record(s) for {domain} with TTL {min_ttl}s",
                        servers.len()
                    );
                    Ok((servers, min_ttl))
                }
            }
            Err(err) => {
                warn!("A/AAAA lookup failed for {domain}: {err}");
                if err.is_no_records_found() {
                    Err(DnsError::NoMailServers(domain.to_string()))
                } else {
                    Err(DnsError::LookupFailed(err))
                }
            }
        }
    }

    /// Randomize servers within each priority group (RFC 5321 Section 5.1)
    ///
    /// After sorting by priority, servers with equal priority should be randomized
    /// to distribute load across them. This implements the RFC 5321 recommendation
    /// for MX record selection.
    fn randomize_equal_priority(servers: &mut [MailServer]) {
        use rand::seq::SliceRandom;

        if servers.len() <= 1 {
            return;
        }

        // Find priority group boundaries
        let mut start = 0;
        while start < servers.len() {
            let current_priority = servers[start].priority;
            let mut end = start + 1;

            // Find the end of this priority group
            while end < servers.len() && servers[end].priority == current_priority {
                end += 1;
            }

            // Randomize servers within this priority group
            if end - start > 1 {
                servers[start..end].shuffle(&mut rand::rng());
            }

            start = end;
        }
    }

    /// Validates that a domain exists by attempting any DNS lookup.
    ///
    /// # Errors
    ///
    /// Returns `DnsError::DomainNotFound` if the domain does not exist.
    pub async fn validate_domain(&self, domain: &str) -> Result<(), DnsError> {
        match self.resolver.lookup_ip(domain).await {
            Ok(_) => Ok(()),
            Err(err) if err.is_no_records_found() || err.is_nx_domain() => {
                Err(DnsError::DomainNotFound(domain.to_string()))
            }
            Err(err) => Err(DnsError::LookupFailed(err)),
        }
    }

    // ============================================================================
    // Cache Management Methods (for control interface)
    // ============================================================================

    /// Get a snapshot of all cached DNS entries with their remaining TTL
    ///
    /// Returns a `HashMap` mapping domain names to their cached mail servers.
    /// Each entry includes the time remaining until the cache expires.
    ///
    /// As a side effect, this method actively evicts expired entries from the cache,
    /// preventing memory waste in long-running MTAs.
    #[must_use]
    pub fn list_cache(&self) -> std::collections::HashMap<String, Vec<(MailServer, Duration)>> {
        let now = Instant::now();
        let mut result = std::collections::HashMap::new();
        let mut expired_keys = Vec::new();

        for entry in self.cache.iter() {
            let domain = entry.key().clone();
            let cached = entry.value();

            // Check if entry is expired
            if cached.expires_at <= now {
                expired_keys.push(domain);
                continue; // Skip expired entries
            }

            // Calculate remaining TTL
            let ttl_remaining = cached
                .expires_at
                .checked_duration_since(now)
                .unwrap_or(Duration::ZERO);

            // Map servers with their TTL
            let servers_with_ttl: Vec<_> = cached
                .servers
                .iter()
                .map(|server| (server.clone(), ttl_remaining))
                .collect();

            result.insert(domain, servers_with_ttl);
        }

        // Clean up expired entries
        if !expired_keys.is_empty() {
            debug!("Evicting {} expired DNS cache entries", expired_keys.len());
            for key in expired_keys {
                self.cache.remove(&key);
            }
        }

        result
    }

    /// Clear the entire DNS cache
    ///
    /// All cached entries will be removed, forcing fresh DNS lookups
    /// for subsequent `resolve_mail_servers` calls.
    pub fn clear_cache(&self) {
        debug!("Clearing DNS cache ({} entries)", self.cache.len());
        self.cache.clear();
    }

    /// Invalidate the cache entry for a specific domain
    ///
    /// The next call to `resolve_mail_servers` for this domain will
    /// perform a fresh DNS lookup.
    ///
    /// Returns `true` if an entry was removed, `false` if no entry existed.
    pub fn invalidate_domain(&self, domain: &str) -> bool {
        debug!("Invalidating DNS cache for domain: {domain}");
        self.cache.remove(domain).is_some()
    }

    /// Refresh the DNS cache for a specific domain
    ///
    /// Performs a fresh DNS lookup and updates the cache.
    ///
    /// # Errors
    ///
    /// Returns `DnsError` if the DNS lookup fails.
    pub async fn refresh_domain(&self, domain: &str) -> Result<Arc<Vec<MailServer>>, DnsError> {
        debug!("Refreshing DNS cache for domain: {domain}");

        // Remove existing entry
        self.cache.remove(domain);

        // Perform fresh lookup (which will re-populate the cache)
        self.resolve_mail_servers(domain).await
    }

    /// Get cache statistics
    ///
    /// Returns information about cache size and efficiency.
    #[must_use]
    pub fn cache_stats(&self) -> CacheStats {
        let now = Instant::now();
        let mut expired_count = 0;

        for entry in self.cache.iter() {
            if entry.value().expires_at <= now {
                expired_count += 1;
            }
        }

        CacheStats {
            total_entries: self.cache.len(),
            expired_entries: expired_count,
            capacity: self.config.cache_size,
        }
    }
}

/// Statistics about the DNS cache
#[derive(Debug, Clone)]
pub struct CacheStats {
    /// Total number of cached entries
    pub total_entries: usize,
    /// Number of expired entries (stale but not yet evicted)
    pub expired_entries: usize,
    /// Configured cache capacity
    pub capacity: usize,
}

impl Default for DnsResolver {
    fn default() -> Self {
        // Try system DNS configuration first
        Self::new().unwrap_or_else(|e| {
            // System DNS failed, use Cloudflare fallback (1.1.1.1, 1.0.0.1)
            warn!(
                error = %e,
                "System DNS configuration failed, using Cloudflare fallback (1.1.1.1)"
            );

            #[allow(clippy::expect_used, reason = "The default resolver should not error")]
            Self::with_resolver_config(
                ResolverConfig::cloudflare(),
                ResolverOpts::default(),
                DnsConfig::default(),
            )
            .expect("Fallback DNS resolver failed - this should never happen with hardcoded config")
        })
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "Requires network access"]
    async fn test_mx_lookup_gmail() {
        let resolver = DnsResolver::new().unwrap();
        let servers = resolver.resolve_mail_servers("gmail.com").await.unwrap();

        assert!(!servers.is_empty());
        assert!(servers.iter().all(|s| s.port == 25));
        // Verify sorted by priority
        assert!(servers.windows(2).all(|w| w[0].priority <= w[1].priority));
    }

    #[tokio::test]
    #[ignore = "Requires network access"]
    async fn test_a_record_fallback() {
        // Use a domain that has A records but no MX records
        // Note: This test may fail if the domain changes its DNS configuration
        let resolver = DnsResolver::new().unwrap();
        let servers = resolver.resolve_mail_servers("example.com").await;

        // Either MX or A/AAAA should work
        assert!(servers.is_ok());
    }

    #[tokio::test]
    #[ignore = "Requires network access"]
    async fn test_domain_not_found() {
        let resolver = DnsResolver::new().unwrap();
        let result = resolver
            .resolve_mail_servers("this-domain-definitely-does-not-exist-12345.com")
            .await;

        assert!(result.is_err());
    }

    #[test]
    fn test_mail_server_address() {
        let server = MailServer::new("mail.example.com".to_string(), 10, 25);
        assert_eq!(server.address(), "mail.example.com:25");
    }

    #[test]
    fn test_priority_sorting() {
        let mut servers = [
            MailServer::new("mx3.example.com".to_string(), 30, 25),
            MailServer::new("mx1.example.com".to_string(), 10, 25),
            MailServer::new("mx2.example.com".to_string(), 20, 25),
        ];

        servers.sort_by_key(|s| s.priority);

        assert_eq!(servers[0].priority, 10);
        assert_eq!(servers[1].priority, 20);
        assert_eq!(servers[2].priority, 30);
    }

    #[test]
    fn test_dns_error_is_temporary() {
        assert!(DnsError::Timeout("example.com".to_string()).is_temporary());
        assert!(!DnsError::NoMailServers("example.com".to_string()).is_temporary());
        assert!(!DnsError::DomainNotFound("example.com".to_string()).is_temporary());
    }

    #[test]
    fn test_randomize_equal_priority_preserves_priority_order() {
        // Create servers with mixed priorities
        let mut servers = vec![
            MailServer::new("mx1a.example.com".to_string(), 10, 25),
            MailServer::new("mx1b.example.com".to_string(), 10, 25),
            MailServer::new("mx2a.example.com".to_string(), 20, 25),
            MailServer::new("mx2b.example.com".to_string(), 20, 25),
            MailServer::new("mx3.example.com".to_string(), 30, 25),
        ];

        // Randomize
        DnsResolver::randomize_equal_priority(&mut servers);

        // Verify priority boundaries are maintained
        assert_eq!(servers[0].priority, 10);
        assert_eq!(servers[1].priority, 10);
        assert_eq!(servers[2].priority, 20);
        assert_eq!(servers[3].priority, 20);
        assert_eq!(servers[4].priority, 30);
    }

    #[test]
    fn test_randomize_equal_priority_shuffles_within_groups() {
        // Create servers with equal priority
        let original = vec![
            MailServer::new("mx1.example.com".to_string(), 10, 25),
            MailServer::new("mx2.example.com".to_string(), 10, 25),
            MailServer::new("mx3.example.com".to_string(), 10, 25),
            MailServer::new("mx4.example.com".to_string(), 10, 25),
        ];

        // Run randomization multiple times and check if we get different orderings
        // With 4 servers, there are 24 possible orderings. Getting the same order
        // multiple times in a row would be very unlikely if randomization works.
        let mut orderings = std::collections::HashSet::new();

        for _ in 0..10 {
            let mut servers = original.clone();
            DnsResolver::randomize_equal_priority(&mut servers);

            // Create a signature for this ordering
            let signature: Vec<_> = servers.iter().map(|s| s.host.clone()).collect();
            orderings.insert(signature);
        }

        // We should see at least 2 different orderings (very likely more)
        assert!(
            orderings.len() >= 2,
            "Expected randomization to produce different orderings, got only {:?}",
            orderings.len()
        );
    }

    #[test]
    fn test_randomize_equal_priority_single_server() {
        let mut servers = vec![MailServer::new("mx1.example.com".to_string(), 10, 25)];

        // Should not panic with single server
        DnsResolver::randomize_equal_priority(&mut servers);
        assert_eq!(servers.len(), 1);
    }

    #[test]
    fn test_randomize_equal_priority_empty() {
        let mut servers: Vec<MailServer> = vec![];

        // Should not panic with empty slice
        DnsResolver::randomize_equal_priority(&mut servers);
        assert_eq!(servers.len(), 0);
    }
}
