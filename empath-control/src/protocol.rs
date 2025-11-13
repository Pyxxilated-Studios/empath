//! Control protocol types and serialization

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Request sent to the control server
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Request {
    /// DNS cache management commands
    Dns(DnsCommand),
    /// System management commands
    System(SystemCommand),
}

/// DNS cache management commands
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DnsCommand {
    /// List all entries in the DNS cache
    ListCache,
    /// Clear the entire DNS cache
    ClearCache,
    /// Refresh DNS records for a specific domain
    RefreshDomain(String),
    /// Set an MX override for a domain
    SetOverride {
        /// The domain to override
        domain: String,
        /// The mail server host:port to use
        mx_server: String,
    },
    /// Remove an MX override for a domain
    RemoveOverride(String),
    /// List all configured MX overrides
    ListOverrides,
}

/// System management commands
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SystemCommand {
    /// Health check / ping
    Ping,
    /// Get system status and statistics
    Status,
}

/// Response from the control server
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Response {
    /// Command succeeded
    Ok,
    /// Command succeeded with data
    Data(ResponseData),
    /// Command failed with error message
    Error(String),
}

/// Response data types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ResponseData {
    /// DNS cache entries (domain -> list of mail servers)
    DnsCache(HashMap<String, Vec<CachedMailServer>>),
    /// MX overrides (domain -> mail server address)
    MxOverrides(HashMap<String, String>),
    /// System status information
    SystemStatus(SystemStatus),
    /// Simple string message
    Message(String),
}

/// Cached mail server information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedMailServer {
    /// Mail server hostname or IP
    pub host: String,
    /// MX priority (lower = higher priority)
    pub priority: u16,
    /// Port number
    pub port: u16,
    /// Time remaining until cache expires (in seconds)
    pub ttl_remaining_secs: u64,
}

/// System status information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemStatus {
    /// Server version
    pub version: String,
    /// Uptime in seconds
    pub uptime_secs: u64,
    /// Number of messages in queue
    pub queue_size: usize,
    /// DNS cache statistics
    pub dns_cache_entries: usize,
}

impl Response {
    /// Create an error response
    #[must_use]
    pub fn error(message: impl Into<String>) -> Self {
        Self::Error(message.into())
    }

    /// Create a success response with no data
    #[must_use]
    pub const fn ok() -> Self {
        Self::Ok
    }

    /// Create a response with data
    #[must_use]
    pub const fn data(data: ResponseData) -> Self {
        Self::Data(data)
    }
}
