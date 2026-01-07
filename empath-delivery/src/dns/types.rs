//! DNS types and cached result structures

use std::sync::Arc;
use std::time::Instant;

use hickory_resolver::proto::rr::Record;
use serde::{Deserialize, Serialize};

use super::error::DnssecStatus;

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

/// Cached DNS result with expiration time.
#[derive(Debug, Clone)]
pub(super) struct CachedResult {
    /// The resolved mail servers (Arc for cheap cloning on cache hits)
    pub(super) servers: Arc<Vec<MailServer>>,
    /// When this cache entry expires
    pub(super) expires_at: Instant,
    /// DNSSEC validation status for the cached lookup
    pub(super) dnssec_status: Option<DnssecStatus>,
}

/// Cached TXT record result
#[derive(Debug, Clone)]
pub(super) struct CachedTxtResult {
    /// TXT records for the domain
    pub(super) txt_records: Arc<Vec<String>>,
    /// When this cache entry expires
    pub(super) expires_at: Instant,
    /// DNSSEC validation status for the cached lookup
    pub(super) dnssec_status: Option<DnssecStatus>,
}

/// Cached PTR record result
#[derive(Debug, Clone)]
pub(super) struct CachedPtrResult {
    /// PTR (reverse DNS) records for the IP
    pub(super) ptr_names: Arc<Vec<String>>,
    /// When this cache entry expires
    pub(super) expires_at: Instant,
    /// DNSSEC validation status for the cached lookup
    pub(super) dnssec_status: Option<DnssecStatus>,
}

pub(super) fn dnssec_status_from_records<'a>(
    records: impl Iterator<Item = &'a Record>,
) -> DnssecStatus {
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
