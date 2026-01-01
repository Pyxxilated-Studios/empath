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
//!
//! # Trait Abstraction
//!
//! The `DnsResolver` trait provides a testable abstraction over DNS resolution.
//! - `HickoryDnsResolver`: Production implementation using Hickory DNS
//! - `MockDnsResolver`: Test implementation with configurable responses

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use empath_common::context::Context;
use empath_common::spf::{DnsLookupError, MailServer as SpfMailServer, SpfDnsResolver};
use empath_ffi::modules::{self, ModuleRegistry};
use empath_tracing::traced;
use hickory_resolver::TokioResolver;
use hickory_resolver::config::{ResolverConfig, ResolverOpts};
use hickory_resolver::name_server::TokioConnectionProvider;
use hickory_resolver::proto::rr::Record;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{self, debug, warn};

/// Errors that can occur during DNS resolution.
#[derive(Debug, Clone, Error)]
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

    /// DNSSEC validation failed (bogus or indeterminate).
    #[error("DNSSEC validation failed for {target}: {status}")]
    DnssecValidationFailed {
        target: String,
        status: DnssecStatus,
    },
}

impl DnsError {
    /// Returns `true` if this error is temporary and should be retried.
    #[must_use]
    pub const fn is_temporary(&self) -> bool {
        matches!(self, Self::Timeout(_) | Self::LookupFailed(_))
            || matches!(
                self,
                Self::DnssecValidationFailed {
                    status: DnssecStatus::Indeterminate,
                    ..
                }
            )
    }
}

/// DNSSEC validation status for a lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DnssecStatus {
    Secure,
    Insecure,
    Bogus,
    Indeterminate,
}

impl DnssecStatus {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Secure => "secure",
            Self::Insecure => "insecure",
            Self::Bogus => "bogus",
            Self::Indeterminate => "indeterminate",
        }
    }

    #[must_use]
    pub const fn is_failure(self) -> bool {
        matches!(self, Self::Bogus | Self::Indeterminate)
    }
}

impl std::fmt::Display for DnssecStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
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
    /// DNSSEC validation status for the cached lookup
    dnssec_status: Option<DnssecStatus>,
}

/// Cached TXT record result
#[derive(Debug, Clone)]
struct CachedTxtResult {
    /// TXT records for the domain
    txt_records: Arc<Vec<String>>,
    /// When this cache entry expires
    expires_at: Instant,
    /// DNSSEC validation status for the cached lookup
    dnssec_status: Option<DnssecStatus>,
}

/// Cached PTR record result
#[derive(Debug, Clone)]
struct CachedPtrResult {
    /// PTR (reverse DNS) records for the IP
    ptr_names: Arc<Vec<String>>,
    /// When this cache entry expires
    expires_at: Instant,
    /// DNSSEC validation status for the cached lookup
    dnssec_status: Option<DnssecStatus>,
}

fn dnssec_status_from_records<'a>(records: impl Iterator<Item = &'a Record>) -> DnssecStatus {
    let mut saw_secure = false;
    let mut saw_insecure = false;

    for record in records {
        let proof = record.proof();
        if proof.is_bogus() {
            return DnssecStatus::Bogus;
        }
        if proof.is_secure() {
            saw_secure = true;
        } else if proof.is_insecure() {
            saw_insecure = true;
        }
    }

    if saw_secure {
        DnssecStatus::Secure
    } else if saw_insecure {
        DnssecStatus::Insecure
    } else {
        DnssecStatus::Indeterminate
    }
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

/// Type alias for the future returned by DNS resolution methods
pub type DnsFuture<'a, T> =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<T, DnsError>> + Send + 'a>>;

/// Trait for DNS resolution abstraction.
///
/// Allows for different DNS resolver implementations (production, mock, etc.)
/// while maintaining a consistent interface for the delivery pipeline.
///
/// # Implementations
///
/// - `HickoryDnsResolver`: Production implementation using Hickory DNS
/// - `MockDnsResolver`: Test implementation with configurable responses
pub trait DnsResolver: Send + Sync + std::fmt::Debug {
    /// Resolves mail servers for a domain following RFC 5321 section 5.1.
    ///
    /// # Errors
    ///
    /// Returns `DnsError` if:
    /// - The domain does not exist
    /// - No mail servers (MX, A, or AAAA) are found
    /// - The DNS query fails or times out
    fn resolve_mail_servers<'a>(&'a self, domain: &'a str) -> DnsFuture<'a, Arc<Vec<MailServer>>>;

    /// Validates that a domain exists by attempting any DNS lookup.
    ///
    /// # Errors
    ///
    /// Returns `DnsError::DomainNotFound` if the domain does not exist.
    fn validate_domain<'a>(&'a self, domain: &'a str) -> DnsFuture<'a, ()>;

    /// Get cache statistics
    ///
    /// Returns information about cache size and efficiency.
    fn cache_stats(&self) -> CacheStats;

    /// Clear the entire DNS cache
    ///
    /// All cached entries will be removed, forcing fresh DNS lookups
    /// for subsequent `resolve_mail_servers` calls.
    fn clear_cache(&self);

    /// Invalidate the cache entry for a specific domain
    ///
    /// Returns `true` if an entry was removed, `false` if no entry existed.
    fn invalidate_domain(&self, domain: &str) -> bool;

    /// Refresh the DNS cache for a specific domain
    ///
    /// Performs a fresh DNS lookup and updates the cache.
    ///
    /// # Errors
    ///
    /// Returns `DnsError` if the DNS lookup fails.
    fn refresh_domain<'a>(&'a self, domain: &'a str) -> DnsFuture<'a, Arc<Vec<MailServer>>>;

    /// Get a snapshot of all cached DNS entries with their remaining TTL
    ///
    /// Returns a `HashMap` mapping domain names to their cached mail servers.
    fn list_cache(&self) -> HashMap<String, Vec<(MailServer, Duration)>>;

    /// Look up TXT records for a domain (for SPF, DKIM, DMARC evaluation)
    ///
    /// # Errors
    ///
    /// Returns `DnsError` if:
    /// - The domain does not exist
    /// - The DNS query fails or times out
    fn txt_lookup<'a>(&'a self, domain: &'a str) -> DnsFuture<'a, Arc<Vec<String>>>;

    /// Look up PTR (reverse DNS) records for an IP address (for SPF ptr mechanism)
    ///
    /// # Errors
    ///
    /// Returns `DnsError` if:
    /// - The IP has no PTR record
    /// - The DNS query fails or times out
    fn ptr_lookup(&self, ip: std::net::IpAddr) -> DnsFuture<'_, Arc<Vec<String>>>;

    /// Look up A/AAAA records for a domain (for SPF a/mx mechanism evaluation)
    ///
    /// # Errors
    ///
    /// Returns `DnsError` if:
    /// - The domain does not exist
    /// - No A/AAAA records found
    /// - The DNS query fails or times out
    fn a_lookup<'a>(&'a self, domain: &'a str) -> DnsFuture<'a, Arc<Vec<std::net::IpAddr>>>;
}

/// Production DNS resolver using Hickory DNS with concurrent caching.
///
/// Uses `DashMap` for lock-free concurrent cache access, providing better
/// throughput under high load compared to mutex-based caching.
#[derive(Debug)]
pub struct HickoryDnsResolver {
    resolver: TokioResolver,
    cache: Arc<DashMap<String, CachedResult>>,
    txt_cache: Arc<DashMap<String, CachedTxtResult>>,
    ptr_cache: Arc<DashMap<String, CachedPtrResult>>,
    a_cache: Arc<DashMap<String, CachedPtrResult>>, /* Reuse for A/AAAA records (stores IPs as strings) */
    config: DnsConfig,
}

impl HickoryDnsResolver {
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
        opts.validate = dns_config.dnssec_enabled;

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
        let txt_cache = Arc::new(DashMap::new());
        let ptr_cache = Arc::new(DashMap::new());
        let a_cache = Arc::new(DashMap::new());

        Ok(Self {
            resolver,
            cache,
            txt_cache,
            ptr_cache,
            a_cache,
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
        Ok(Self::build_with_resolver_config(
            resolver_config,
            opts,
            dns_config,
        ))
    }

    fn build_with_resolver_config(
        resolver_config: ResolverConfig,
        mut opts: ResolverOpts,
        dns_config: DnsConfig,
    ) -> Self {
        opts.validate = dns_config.dnssec_enabled;
        let resolver =
            TokioResolver::builder_with_config(resolver_config, TokioConnectionProvider::default())
                .with_options(opts)
                .build();

        let cache = Arc::new(DashMap::new());
        let txt_cache = Arc::new(DashMap::new());
        let ptr_cache = Arc::new(DashMap::new());
        let a_cache = Arc::new(DashMap::new());

        Self {
            resolver,
            cache,
            txt_cache,
            ptr_cache,
            a_cache,
            config: dns_config,
        }
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
    #[traced(instrument(level = tracing::Level::INFO, skip(self), fields(domain = %domain)), timing(precision = "ms"))]
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
                // DNS lookup results are transaction context
                ctx.transaction
                    .insert(Cow::Borrowed("dns_cache_status"), "hit".to_string());
                ctx.transaction
                    .insert(Cow::Borrowed("dns_query_type"), "mx".to_string());
                ctx.transaction
                    .insert(Cow::Borrowed("dns_domain"), domain.to_string());
                ctx.transaction.insert(
                    Cow::Borrowed("dns_cache_size"),
                    self.cache.len().to_string(),
                );
                if let Some(status) = cached.dnssec_status {
                    ctx.transaction
                        .insert(Cow::Borrowed("dnssec_status"), status.as_str().to_string());
                }
                let _ = ModuleRegistry::dispatch_with_registry(
                    ModuleRegistry::global(),
                    modules::Event::Event(modules::Ev::DnsLookup),
                    &mut ctx,
                )
                .await;

                if let Some(status) = cached.dnssec_status
                    && self.config.dnssec_strict
                    && status.is_failure()
                {
                    return Err(DnsError::DnssecValidationFailed {
                        target: domain.to_string(),
                        status,
                    });
                }

                return Ok(Arc::clone(&cached.servers));
            }
            debug!("Cache entry expired for {domain}");
        }

        // Cache miss or expired, perform DNS lookup
        let lookup_start = Instant::now();
        let (servers, dns_ttl, dnssec_status) = self.resolve_mail_servers_uncached(domain).await?;
        let lookup_duration = lookup_start.elapsed();
        let servers = Arc::new(servers);

        let dnssec_failure =
            dnssec_status.is_some_and(|status| self.config.dnssec_strict && status.is_failure());

        // Determine cache TTL: use override if set, otherwise use DNS TTL with bounds
        let cache_ttl = self.config.cache_ttl_secs.unwrap_or_else(|| {
            // Apply min/max bounds to DNS TTL
            u64::from(dns_ttl).clamp(
                self.config.min_cache_ttl_secs,
                self.config.max_cache_ttl_secs,
            )
        });

        if !dnssec_failure {
            // Cache the result (lock-free write)
            let expires_at = Instant::now() + Duration::from_secs(cache_ttl);
            let cached_result = CachedResult {
                servers: Arc::clone(&servers),
                expires_at,
                dnssec_status,
            };

            self.cache.insert(domain.to_string(), cached_result);
        }

        // Dispatch DNS cache miss event
        let mut ctx = Context::default();
        // DNS lookup results are transaction context
        ctx.transaction
            .insert(Cow::Borrowed("dns_cache_status"), "miss".to_string());
        ctx.transaction
            .insert(Cow::Borrowed("dns_query_type"), "mx".to_string());
        ctx.transaction
            .insert(Cow::Borrowed("dns_domain"), domain.to_string());
        ctx.transaction.insert(
            Cow::Borrowed("dns_lookup_duration_ms"),
            lookup_duration.as_millis().to_string(),
        );
        ctx.transaction.insert(
            Cow::Borrowed("dns_cache_size"),
            self.cache.len().to_string(),
        );
        if let Some(status) = dnssec_status {
            ctx.transaction
                .insert(Cow::Borrowed("dnssec_status"), status.as_str().to_string());
        }
        let _ = ModuleRegistry::dispatch_with_registry(
            ModuleRegistry::global(),
            modules::Event::Event(modules::Ev::DnsLookup),
            &mut ctx,
        )
        .await;

        if dnssec_failure && let Some(status) = dnssec_status {
            return Err(DnsError::DnssecValidationFailed {
                target: domain.to_string(),
                status,
            });
        }

        if !dnssec_failure {
            debug!(
                "Cached result for {domain}, DNS TTL: {dns_ttl}s, cache TTL: {cache_ttl}s, {} server(s)",
                servers.len()
            );
        }
        Ok(servers)
    }

    /// Performs uncached DNS lookup for mail servers.
    ///
    /// Returns a tuple of (servers, ttl) where ttl is the minimum TTL from all records.
    async fn resolve_mail_servers_uncached(
        &self,
        domain: &str,
    ) -> Result<(Vec<MailServer>, u32, Option<DnssecStatus>), DnsError> {
        // Try MX lookup first
        match self.resolver.mx_lookup(domain).await {
            Ok(mx_lookup) => {
                let dnssec_status = self
                    .config
                    .dnssec_enabled
                    .then(|| dnssec_status_from_records(mx_lookup.as_lookup().record_iter()));
                if let Some(status) = dnssec_status {
                    debug!(
                        dnssec_status = %status,
                        dns_query_type = "mx",
                        target = %domain,
                        "DNSSEC validation status"
                    );
                }

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
                Ok((servers, min_ttl, dnssec_status))
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
    async fn fallback_to_a_aaaa(
        &self,
        domain: &str,
    ) -> Result<(Vec<MailServer>, u32, Option<DnssecStatus>), DnsError> {
        debug!("Attempting A/AAAA fallback for {domain}");

        match self.resolver.lookup_ip(domain).await {
            Ok(ip_lookup) => {
                let dnssec_status = self
                    .config
                    .dnssec_enabled
                    .then(|| dnssec_status_from_records(ip_lookup.as_lookup().record_iter()));
                if let Some(status) = dnssec_status {
                    debug!(
                        dnssec_status = %status,
                        dns_query_type = "a",
                        target = %domain,
                        "DNSSEC validation status"
                    );
                }

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
                    Ok((servers, min_ttl, dnssec_status))
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
    pub async fn validate_domain_impl(&self, domain: &str) -> Result<(), DnsError> {
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
    pub fn list_cache_impl(&self) -> HashMap<String, Vec<(MailServer, Duration)>> {
        let now = Instant::now();
        let mut result = HashMap::new();
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
    pub fn clear_cache_impl(&self) {
        debug!("Clearing DNS cache ({} entries)", self.cache.len());
        self.cache.clear();
    }

    /// Invalidate the cache entry for a specific domain
    ///
    /// The next call to `resolve_mail_servers` for this domain will
    /// perform a fresh DNS lookup.
    ///
    /// Returns `true` if an entry was removed, `false` if no entry existed.
    pub fn invalidate_domain_impl(&self, domain: &str) -> bool {
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
    pub async fn refresh_domain_impl(
        &self,
        domain: &str,
    ) -> Result<Arc<Vec<MailServer>>, DnsError> {
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
    pub fn cache_stats_impl(&self) -> CacheStats {
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

    /// Look up TXT records for a domain (for SPF, DKIM, DMARC evaluation)
    ///
    /// # Errors
    ///
    /// Returns `DnsError` if the DNS query fails or times out.
    #[traced(instrument(level = tracing::Level::INFO, skip(self), fields(domain = %domain)), timing(precision = "ms"))]
    pub async fn txt_lookup_impl(&self, domain: &str) -> Result<Arc<Vec<String>>, DnsError> {
        debug!("Looking up TXT records for domain: {domain}");

        // Check cache first
        if let Some(cached) = self.txt_cache.get(domain) {
            if cached.expires_at > Instant::now() {
                debug!(
                    "TXT cache hit for {domain}, {} record(s)",
                    cached.txt_records.len()
                );
                if let Some(status) = cached.dnssec_status
                    && self.config.dnssec_strict
                    && status.is_failure()
                {
                    return Err(DnsError::DnssecValidationFailed {
                        target: domain.to_string(),
                        status,
                    });
                }
                return Ok(Arc::clone(&cached.txt_records));
            }
            debug!("TXT cache entry expired for {domain}");
        }

        // Perform DNS lookup
        match self.resolver.txt_lookup(domain).await {
            Ok(txt_lookup) => {
                let dnssec_status = self
                    .config
                    .dnssec_enabled
                    .then(|| dnssec_status_from_records(txt_lookup.as_lookup().record_iter()));
                if let Some(status) = dnssec_status {
                    debug!(
                        dnssec_status = %status,
                        dns_query_type = "txt",
                        target = %domain,
                        "DNSSEC validation status"
                    );
                }
                if let Some(status) = dnssec_status
                    && self.config.dnssec_strict
                    && status.is_failure()
                {
                    return Err(DnsError::DnssecValidationFailed {
                        target: domain.to_string(),
                        status,
                    });
                }

                // Extract minimum TTL
                let min_ttl = txt_lookup
                    .as_lookup()
                    .records()
                    .iter()
                    .map(hickory_resolver::proto::rr::Record::ttl)
                    .min()
                    .unwrap_or(300);

                // Extract TXT records (concatenate all character strings)
                let txt_records: Vec<String> = txt_lookup
                    .iter()
                    .map(|txt| {
                        txt.iter()
                            .map(|bytes| String::from_utf8_lossy(bytes).to_string())
                            .collect::<String>()
                    })
                    .collect();

                debug!(
                    "Resolved {} TXT record(s) for {domain} with TTL {min_ttl}s",
                    txt_records.len()
                );

                // Apply TTL bounds
                let ttl_secs = self.config.cache_ttl_secs.unwrap_or_else(|| {
                    u64::from(min_ttl)
                        .max(self.config.min_cache_ttl_secs)
                        .min(self.config.max_cache_ttl_secs)
                });

                let expires_at = Instant::now() + Duration::from_secs(ttl_secs);
                let txt_records = Arc::new(txt_records);

                // Cache the result
                self.txt_cache.insert(
                    domain.to_string(),
                    CachedTxtResult {
                        txt_records: Arc::clone(&txt_records),
                        expires_at,
                        dnssec_status,
                    },
                );

                Ok(txt_records)
            }
            Err(err) => {
                warn!("TXT lookup failed for {domain}: {err}");
                if err.is_no_records_found() {
                    // No TXT records is not an error for SPF - it means no SPF record
                    Ok(Arc::new(Vec::new()))
                } else {
                    Err(DnsError::LookupFailed(err))
                }
            }
        }
    }

    /// Look up PTR (reverse DNS) records for an IP address
    ///
    /// # Errors
    ///
    /// Returns `DnsError` if the DNS query fails or times out.
    #[traced(instrument(level = tracing::Level::INFO, skip(self), fields(ip = %ip)), timing(precision = "ms"))]
    pub async fn ptr_lookup_impl(
        &self,
        ip: std::net::IpAddr,
    ) -> Result<Arc<Vec<String>>, DnsError> {
        let cache_key = ip.to_string();
        debug!("Looking up PTR records for IP: {ip}");

        // Check cache first
        if let Some(cached) = self.ptr_cache.get(&cache_key) {
            if cached.expires_at > Instant::now() {
                debug!(
                    "PTR cache hit for {ip}, {} record(s)",
                    cached.ptr_names.len()
                );
                if let Some(status) = cached.dnssec_status
                    && self.config.dnssec_strict
                    && status.is_failure()
                {
                    return Err(DnsError::DnssecValidationFailed {
                        target: cache_key,
                        status,
                    });
                }
                return Ok(Arc::clone(&cached.ptr_names));
            }
            debug!("PTR cache entry expired for {ip}");
        }

        // Perform reverse DNS lookup
        match self.resolver.reverse_lookup(ip).await {
            Ok(ptr_lookup) => {
                let dnssec_status = self
                    .config
                    .dnssec_enabled
                    .then(|| dnssec_status_from_records(ptr_lookup.as_lookup().record_iter()));
                if let Some(status) = dnssec_status {
                    debug!(
                        dnssec_status = %status,
                        dns_query_type = "ptr",
                        target = %ip,
                        "DNSSEC validation status"
                    );
                }
                if let Some(status) = dnssec_status
                    && self.config.dnssec_strict
                    && status.is_failure()
                {
                    return Err(DnsError::DnssecValidationFailed {
                        target: cache_key,
                        status,
                    });
                }

                // Extract minimum TTL
                let min_ttl = ptr_lookup
                    .as_lookup()
                    .records()
                    .iter()
                    .map(hickory_resolver::proto::rr::Record::ttl)
                    .min()
                    .unwrap_or(300);

                // Extract PTR names
                let ptr_names: Vec<String> = ptr_lookup.iter().map(|name| name.to_utf8()).collect();

                debug!(
                    "Resolved {} PTR record(s) for {ip} with TTL {min_ttl}s",
                    ptr_names.len()
                );

                // Apply TTL bounds
                let ttl_secs = self.config.cache_ttl_secs.unwrap_or_else(|| {
                    u64::from(min_ttl)
                        .max(self.config.min_cache_ttl_secs)
                        .min(self.config.max_cache_ttl_secs)
                });

                let expires_at = Instant::now() + Duration::from_secs(ttl_secs);
                let ptr_names = Arc::new(ptr_names);

                // Cache the result
                self.ptr_cache.insert(
                    cache_key,
                    CachedPtrResult {
                        ptr_names: Arc::clone(&ptr_names),
                        expires_at,
                        dnssec_status,
                    },
                );

                Ok(ptr_names)
            }
            Err(err) => {
                warn!("PTR lookup failed for {ip}: {err}");
                if err.is_no_records_found() {
                    // No PTR records - return empty list
                    Ok(Arc::new(Vec::new()))
                } else {
                    Err(DnsError::LookupFailed(err))
                }
            }
        }
    }

    /// Look up A/AAAA records for a domain (for SPF a/mx mechanism evaluation)
    ///
    /// # Errors
    ///
    /// Returns `DnsError` if the DNS query fails or times out.
    #[traced(instrument(level = tracing::Level::INFO, skip(self), fields(domain = %domain)), timing(precision = "ms"))]
    pub async fn a_lookup_impl(
        &self,
        domain: &str,
    ) -> Result<Arc<Vec<std::net::IpAddr>>, DnsError> {
        debug!("Looking up A/AAAA records for domain: {domain}");

        // Check cache first
        if let Some(cached) = self.a_cache.get(domain) {
            if cached.expires_at > Instant::now() {
                // Parse cached IPs from strings
                let ips: Vec<std::net::IpAddr> = cached
                    .ptr_names
                    .iter()
                    .filter_map(|s| s.parse().ok())
                    .collect();
                debug!("A/AAAA cache hit for {domain}, {} record(s)", ips.len());
                if let Some(status) = cached.dnssec_status
                    && self.config.dnssec_strict
                    && status.is_failure()
                {
                    return Err(DnsError::DnssecValidationFailed {
                        target: domain.to_string(),
                        status,
                    });
                }
                return Ok(Arc::new(ips));
            }
            debug!("A/AAAA cache entry expired for {domain}");
        }

        // Perform DNS lookup
        match self.resolver.lookup_ip(domain).await {
            Ok(ip_lookup) => {
                let dnssec_status = self
                    .config
                    .dnssec_enabled
                    .then(|| dnssec_status_from_records(ip_lookup.as_lookup().record_iter()));
                if let Some(status) = dnssec_status {
                    debug!(
                        dnssec_status = %status,
                        dns_query_type = "a",
                        target = %domain,
                        "DNSSEC validation status"
                    );
                }
                if let Some(status) = dnssec_status
                    && self.config.dnssec_strict
                    && status.is_failure()
                {
                    return Err(DnsError::DnssecValidationFailed {
                        target: domain.to_string(),
                        status,
                    });
                }

                // Extract minimum TTL
                let min_ttl = ip_lookup
                    .as_lookup()
                    .records()
                    .iter()
                    .map(hickory_resolver::proto::rr::Record::ttl)
                    .min()
                    .unwrap_or(300);

                // Extract IP addresses
                let ips: Vec<std::net::IpAddr> = ip_lookup.iter().collect();

                debug!(
                    "Resolved {} A/AAAA record(s) for {domain} with TTL {min_ttl}s",
                    ips.len()
                );

                // Apply TTL bounds
                let ttl_secs = self.config.cache_ttl_secs.unwrap_or_else(|| {
                    u64::from(min_ttl)
                        .max(self.config.min_cache_ttl_secs)
                        .min(self.config.max_cache_ttl_secs)
                });

                let expires_at = Instant::now() + Duration::from_secs(ttl_secs);

                // Store IPs as strings in cache (reusing CachedPtrResult structure)
                let ip_strings: Vec<String> =
                    ips.iter().map(std::string::ToString::to_string).collect();
                let ips_arc = Arc::new(ips);

                self.a_cache.insert(
                    domain.to_string(),
                    CachedPtrResult {
                        ptr_names: Arc::new(ip_strings),
                        expires_at,
                        dnssec_status,
                    },
                );

                Ok(ips_arc)
            }
            Err(err) => {
                warn!("A/AAAA lookup failed for {domain}: {err}");
                if err.is_no_records_found() {
                    Ok(Arc::new(Vec::new()))
                } else {
                    Err(DnsError::LookupFailed(err))
                }
            }
        }
    }
}

/// Implement the `DnsResolver` trait for `HickoryDnsResolver`
///
/// This forwards trait method calls to the implementation methods.
impl DnsResolver for HickoryDnsResolver {
    fn resolve_mail_servers<'a>(&'a self, domain: &'a str) -> DnsFuture<'a, Arc<Vec<MailServer>>> {
        Box::pin(self.resolve_mail_servers(domain))
    }

    fn validate_domain<'a>(&'a self, domain: &'a str) -> DnsFuture<'a, ()> {
        Box::pin(self.validate_domain_impl(domain))
    }

    fn cache_stats(&self) -> CacheStats {
        self.cache_stats_impl()
    }

    fn clear_cache(&self) {
        self.clear_cache_impl();
    }

    fn invalidate_domain(&self, domain: &str) -> bool {
        self.invalidate_domain_impl(domain)
    }

    fn refresh_domain<'a>(&'a self, domain: &'a str) -> DnsFuture<'a, Arc<Vec<MailServer>>> {
        Box::pin(self.refresh_domain_impl(domain))
    }

    fn list_cache(&self) -> HashMap<String, Vec<(MailServer, Duration)>> {
        self.list_cache_impl()
    }

    fn txt_lookup<'a>(&'a self, domain: &'a str) -> DnsFuture<'a, Arc<Vec<String>>> {
        Box::pin(self.txt_lookup_impl(domain))
    }

    fn ptr_lookup(&self, ip: std::net::IpAddr) -> DnsFuture<'_, Arc<Vec<String>>> {
        Box::pin(self.ptr_lookup_impl(ip))
    }

    fn a_lookup<'a>(&'a self, domain: &'a str) -> DnsFuture<'a, Arc<Vec<std::net::IpAddr>>> {
        Box::pin(self.a_lookup_impl(domain))
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

impl Default for HickoryDnsResolver {
    fn default() -> Self {
        match Self::new() {
            Ok(resolver) => resolver,
            Err(err) => {
                warn!(
                    error = %err,
                    "System DNS configuration failed, using Cloudflare fallback (1.1.1.1)"
                );
                let dns_config = DnsConfig::default();
                let mut opts = ResolverOpts::default();
                opts.timeout = Duration::from_secs(dns_config.timeout_secs);
                Self::build_with_resolver_config(ResolverConfig::cloudflare(), opts, dns_config)
            }
        }
    }
}

/// Mock DNS resolver for testing.
///
/// Allows tests to configure DNS responses without making real network calls.
/// Useful for testing delivery logic with various DNS scenarios (multiple MX records,
/// DNS failures, timeouts, etc.).
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use empath_delivery::{MockDnsResolver, MailServer, DnsResolver, DnsError};
///
/// # async {
/// let mut mock = MockDnsResolver::new();
///
/// // Configure successful response
/// mock.add_response("example.com", Ok(vec![
///     MailServer::new("mx1.example.com".to_string(), 10, 25),
///     MailServer::new("mx2.example.com".to_string(), 20, 25),
/// ]));
///
/// // Configure DNS failure
/// mock.add_response("bad.example.com",
///     Err(DnsError::Timeout("bad.example.com".to_string())));
///
/// // Use mock in tests
/// let servers = mock.resolve_mail_servers("example.com").await;
/// assert!(servers.is_ok());
/// # };
/// ```
#[derive(Debug, Default)]
pub struct MockDnsResolver {
    /// Configured responses for MX/A/AAAA lookups
    responses: DashMap<String, Result<Arc<Vec<MailServer>>, DnsError>>,
    /// Configured responses for TXT record lookups
    txt_responses: DashMap<String, Result<Arc<Vec<String>>, DnsError>>,
    /// Configured responses for PTR record lookups
    ptr_responses: DashMap<String, Result<Arc<Vec<String>>, DnsError>>,
    /// Configured responses for A/AAAA record lookups
    a_responses: DashMap<String, Result<Arc<Vec<std::net::IpAddr>>, DnsError>>,
}

impl MockDnsResolver {
    /// Create a new empty mock resolver
    #[must_use]
    pub fn new() -> Self {
        Self {
            responses: DashMap::new(),
            txt_responses: DashMap::new(),
            ptr_responses: DashMap::new(),
            a_responses: DashMap::new(),
        }
    }

    /// Add a mock response for a domain
    ///
    /// # Arguments
    ///
    /// * `domain` - The domain to configure a response for
    /// * `result` - The result to return when this domain is queried
    ///
    /// # Examples
    ///
    /// ```
    /// use empath_delivery::{MockDnsResolver, MailServer, DnsError};
    ///
    /// let mut mock = MockDnsResolver::new();
    ///
    /// // Success case
    /// mock.add_response("example.com", Ok(vec![
    ///     MailServer::new("mx.example.com".to_string(), 10, 25),
    /// ]));
    ///
    /// // Failure case
    /// mock.add_response("bad.example.com",
    ///     Err(DnsError::Timeout("bad.example.com".to_string())));
    /// ```
    pub fn add_response(
        &self,
        domain: impl Into<String>,
        result: Result<Vec<MailServer>, DnsError>,
    ) {
        let domain = domain.into();
        let result = result.map(Arc::new);
        self.responses.insert(domain, result);
    }

    /// Add a mock TXT record response for a domain
    pub fn add_txt(&self, domain: impl Into<String>, result: Result<Vec<String>, DnsError>) {
        let domain = domain.into();
        let result = result.map(Arc::new);
        self.txt_responses.insert(domain, result);
    }

    /// Add a mock PTR record response for an IP
    pub fn add_ptr(&self, ip: impl Into<String>, result: Result<Vec<String>, DnsError>) {
        let ip = ip.into();
        let result = result.map(Arc::new);
        self.ptr_responses.insert(ip, result);
    }

    /// Add a mock A/AAAA record response for a domain
    pub fn add_a(
        &self,
        domain: impl Into<String>,
        result: Result<Vec<std::net::IpAddr>, DnsError>,
    ) {
        let domain = domain.into();
        let result = result.map(Arc::new);
        self.a_responses.insert(domain, result);
    }

    /// Clear all configured responses
    pub fn clear(&self) {
        self.responses.clear();
        self.txt_responses.clear();
        self.ptr_responses.clear();
        self.a_responses.clear();
    }
}

impl DnsResolver for MockDnsResolver {
    fn resolve_mail_servers<'a>(&'a self, domain: &'a str) -> DnsFuture<'a, Arc<Vec<MailServer>>> {
        let result = self.responses.get(domain).map_or_else(
            || {
                Err(DnsError::NoMailServers(format!(
                    "No mock response configured for domain: {domain}"
                )))
            },
            |entry| entry.clone(),
        );

        Box::pin(async move { result })
    }

    fn validate_domain<'a>(&'a self, domain: &'a str) -> DnsFuture<'a, ()> {
        let result = self.responses.get(domain).map_or_else(
            || Err(DnsError::DomainNotFound(domain.to_string())),
            |_| Ok(()),
        );

        Box::pin(async move { result })
    }

    fn cache_stats(&self) -> CacheStats {
        // Mock resolver has no real cache
        CacheStats {
            total_entries: 0,
            expired_entries: 0,
            capacity: 0,
        }
    }

    fn clear_cache(&self) {
        // Mock resolver has no cache to clear
    }

    fn invalidate_domain(&self, _domain: &str) -> bool {
        // Mock resolver has no cache to invalidate
        false
    }

    fn refresh_domain<'a>(&'a self, domain: &'a str) -> DnsFuture<'a, Arc<Vec<MailServer>>> {
        // For mock, just return the configured response
        self.resolve_mail_servers(domain)
    }

    fn list_cache(&self) -> HashMap<String, Vec<(MailServer, Duration)>> {
        // Mock resolver has no cache
        HashMap::new()
    }

    fn txt_lookup<'a>(&'a self, domain: &'a str) -> DnsFuture<'a, Arc<Vec<String>>> {
        let result = self.txt_responses.get(domain).map_or_else(
            || Ok(Arc::new(Vec::new())), // No TXT records configured - return empty
            |entry| entry.clone(),
        );

        Box::pin(async move { result })
    }

    fn ptr_lookup(&self, ip: std::net::IpAddr) -> DnsFuture<'_, Arc<Vec<String>>> {
        let ip_str = ip.to_string();
        let result = self.ptr_responses.get(&ip_str).map_or_else(
            || Ok(Arc::new(Vec::new())), // No PTR records configured - return empty
            |entry| entry.clone(),
        );

        Box::pin(async move { result })
    }

    fn a_lookup<'a>(&'a self, domain: &'a str) -> DnsFuture<'a, Arc<Vec<std::net::IpAddr>>> {
        let result = self.a_responses.get(domain).map_or_else(
            || Ok(Arc::new(Vec::new())), // No A records configured - return empty
            |entry| entry.clone(),
        );

        Box::pin(async move { result })
    }
}

/// Adapter that implements `SpfDnsResolver` for any `DnsResolver`
///
/// This allows SPF evaluation to use the production DNS resolver with caching.
pub struct SpfDnsAdapter<D: ?Sized> {
    resolver: Arc<D>,
}

impl<D: ?Sized> SpfDnsAdapter<D> {
    /// Create a new SPF DNS resolver adapter
    #[must_use]
    pub const fn new(resolver: Arc<D>) -> Self {
        Self { resolver }
    }
}

#[async_trait::async_trait]
impl<D: ?Sized + DnsResolver> SpfDnsResolver for SpfDnsAdapter<D> {
    async fn txt_lookup(&self, domain: &str) -> Result<Arc<Vec<String>>, DnsLookupError> {
        self.resolver.txt_lookup(domain).await.map_err(|e| match e {
            DnsError::Timeout(msg) => DnsLookupError::Timeout(msg),
            _ => DnsLookupError::LookupFailed(e.to_string()),
        })
    }

    async fn ptr_lookup(&self, ip: std::net::IpAddr) -> Result<Arc<Vec<String>>, DnsLookupError> {
        self.resolver.ptr_lookup(ip).await.map_err(|e| match e {
            DnsError::Timeout(msg) => DnsLookupError::Timeout(msg),
            _ => DnsLookupError::LookupFailed(e.to_string()),
        })
    }

    async fn a_lookup(&self, domain: &str) -> Result<Arc<Vec<std::net::IpAddr>>, DnsLookupError> {
        self.resolver.a_lookup(domain).await.map_err(|e| match e {
            DnsError::Timeout(msg) => DnsLookupError::Timeout(msg),
            _ => DnsLookupError::LookupFailed(e.to_string()),
        })
    }

    async fn mx_lookup(&self, domain: &str) -> Result<Arc<Vec<SpfMailServer>>, DnsLookupError> {
        self.resolver
            .resolve_mail_servers(domain)
            .await
            .map_err(|e| match e {
                DnsError::Timeout(msg) => DnsLookupError::Timeout(msg),
                _ => DnsLookupError::LookupFailed(e.to_string()),
            })
            .map(|servers| {
                // Convert empath_delivery::MailServer to SpfMailServer
                let spf_servers: Vec<SpfMailServer> = servers
                    .iter()
                    .map(|s| SpfMailServer {
                        host: s.host.clone(),
                        priority: s.priority,
                    })
                    .collect();
                Arc::new(spf_servers)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = std::result::Result<(), String>;

    #[tokio::test]
    #[ignore = "Requires network access"]
    async fn test_mx_lookup_gmail() -> TestResult {
        let resolver = HickoryDnsResolver::new().map_err(|err| err.to_string())?;
        let servers = resolver
            .resolve_mail_servers("gmail.com")
            .await
            .map_err(|err| err.to_string())?;

        assert!(!servers.is_empty());
        assert!(servers.iter().all(|s| s.port == 25));
        // Verify sorted by priority
        assert!(servers.windows(2).all(|w| w[0].priority <= w[1].priority));
        Ok(())
    }

    #[tokio::test]
    #[ignore = "Requires network access"]
    async fn test_a_record_fallback() -> TestResult {
        // Use a domain that has A records but no MX records
        // Note: This test may fail if the domain changes its DNS configuration
        let resolver = HickoryDnsResolver::new().map_err(|err| err.to_string())?;
        let servers = resolver.resolve_mail_servers("example.com").await;

        // Either MX or A/AAAA should work
        assert!(servers.is_ok());
        Ok(())
    }

    #[tokio::test]
    #[ignore = "Requires network access"]
    async fn test_domain_not_found() -> TestResult {
        let resolver = HickoryDnsResolver::new().map_err(|err| err.to_string())?;
        let result = resolver
            .resolve_mail_servers("this-domain-definitely-does-not-exist-12345.com")
            .await;

        assert!(result.is_err());
        Ok(())
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
        assert!(
            DnsError::DnssecValidationFailed {
                target: "example.com".to_string(),
                status: DnssecStatus::Indeterminate,
            }
            .is_temporary()
        );
        assert!(
            !DnsError::DnssecValidationFailed {
                target: "example.com".to_string(),
                status: DnssecStatus::Bogus,
            }
            .is_temporary()
        );
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
        HickoryDnsResolver::randomize_equal_priority(&mut servers);

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
            HickoryDnsResolver::randomize_equal_priority(&mut servers);

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
        HickoryDnsResolver::randomize_equal_priority(&mut servers);
        assert_eq!(servers.len(), 1);
    }

    #[test]
    fn test_randomize_equal_priority_empty() {
        let mut servers: Vec<MailServer> = vec![];

        // Should not panic with empty slice
        HickoryDnsResolver::randomize_equal_priority(&mut servers);
        assert_eq!(servers.len(), 0);
    }
}
