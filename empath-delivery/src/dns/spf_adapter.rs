//! SPF DNS resolver adapter

use std::sync::Arc;

use empath_common::spf::{DnsLookupError, MailServer as SpfMailServer, SpfDnsResolver};

use super::error::DnsError;
use super::traits::DnsResolver;

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
