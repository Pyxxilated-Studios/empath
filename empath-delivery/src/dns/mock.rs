//! Mock DNS resolver for testing

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use dashmap::DashMap;

use super::error::DnsError;
use super::traits::{DnsFuture, DnsResolver};
use super::types::{CacheStats, MailServer};

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
