//! Shared SPF DNS resolver traits and types.

use std::net::IpAddr;
use std::sync::Arc;

/// DNS lookup errors.
#[derive(Debug, Clone)]
pub enum DnsLookupError {
    /// Lookup failed.
    LookupFailed(String),
    /// Timeout.
    Timeout(String),
}

impl std::fmt::Display for DnsLookupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LookupFailed(msg) => write!(f, "DNS lookup failed: {msg}"),
            Self::Timeout(msg) => write!(f, "DNS timeout: {msg}"),
        }
    }
}

impl std::error::Error for DnsLookupError {}

/// Mail server from MX lookup.
#[derive(Debug, Clone)]
pub struct MailServer {
    /// Hostname of the mail server.
    pub host: String,
    /// Priority (lower is higher priority).
    pub priority: u16,
}

/// DNS resolver for SPF evaluation.
#[async_trait::async_trait]
pub trait SpfDnsResolver: Send + Sync {
    /// Look up TXT records for a domain.
    ///
    /// # Errors
    ///
    /// Returns `DnsLookupError` if the lookup fails.
    async fn txt_lookup(&self, domain: &str) -> Result<Arc<Vec<String>>, DnsLookupError>;

    /// Look up PTR (reverse DNS) records for an IP.
    ///
    /// # Errors
    ///
    /// Returns `DnsLookupError` if the lookup fails.
    async fn ptr_lookup(&self, ip: IpAddr) -> Result<Arc<Vec<String>>, DnsLookupError>;

    /// Look up A/AAAA records for a domain.
    ///
    /// # Errors
    ///
    /// Returns `DnsLookupError` if the lookup fails.
    async fn a_lookup(&self, domain: &str) -> Result<Arc<Vec<IpAddr>>, DnsLookupError>;

    /// Look up MX records for a domain.
    ///
    /// # Errors
    ///
    /// Returns `DnsLookupError` if the lookup fails.
    async fn mx_lookup(&self, domain: &str) -> Result<Arc<Vec<MailServer>>, DnsLookupError>;
}
