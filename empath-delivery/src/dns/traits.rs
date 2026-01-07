//! DNS resolver trait abstraction

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use super::error::DnsError;
use super::types::{CacheStats, MailServer};

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
