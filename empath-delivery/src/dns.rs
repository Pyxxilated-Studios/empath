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
    sync::Arc,
    time::{Duration, Instant},
};

use dashmap::DashMap;
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

        let resolver = TokioResolver::builder(TokioConnectionProvider::default())?
            .with_options(opts)
            .build();

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
                return Ok(Arc::clone(&cached.servers));
            }
            debug!("Cache entry expired for {domain}");
        }

        // Cache miss or expired, perform DNS lookup
        let (servers, dns_ttl) = self.resolve_mail_servers_uncached(domain).await?;
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
                servers.sort_by_key(|s| s.priority);
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
                #[allow(clippy::redundant_closure_for_method_calls)]
                let min_ttl = ip_lookup
                    .as_lookup()
                    .records()
                    .iter()
                    .map(|r| r.ttl())
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
}

impl Default for DnsResolver {
    fn default() -> Self {
        Self::new().expect("Failed to create default DNS resolver")
    }
}

#[cfg(test)]
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
}
