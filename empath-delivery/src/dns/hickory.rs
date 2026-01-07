//! Production DNS resolver using Hickory DNS

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use empath_common::context::Context;
use empath_ffi::modules::{self, ModuleRegistry};
use empath_tracing::traced;
use hickory_resolver::TokioResolver;
use hickory_resolver::config::{ResolverConfig, ResolverOpts};
use hickory_resolver::name_server::TokioConnectionProvider;
use tracing::{debug, warn};

use super::config::DnsConfig;
use super::error::{DnsError, DnssecStatus};
use super::traits::{DnsFuture, DnsResolver};
use super::types::{
    CacheStats, CachedPtrResult, CachedResult, CachedTxtResult, MailServer,
    dnssec_status_from_records,
};

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
    pub(super) fn randomize_equal_priority(servers: &mut [MailServer]) {
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
