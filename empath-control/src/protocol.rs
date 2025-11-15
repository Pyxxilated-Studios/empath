//! Control protocol types and serialization

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Current protocol version
pub const PROTOCOL_VERSION: u32 = 1;

/// Request sent to the control server (versioned wrapper)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    /// Protocol version
    pub version: u32,
    /// The actual command to execute
    pub command: RequestCommand,
}

/// Request command types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RequestCommand {
    /// DNS cache management commands
    Dns(DnsCommand),
    /// System management commands
    System(SystemCommand),
    /// Queue management commands
    Queue(QueueCommand),
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

/// Queue management commands
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum QueueCommand {
    /// List messages in the queue
    List {
        /// Filter by status (optional)
        status_filter: Option<String>,
    },
    /// View detailed information about a specific message
    View {
        /// Message ID to view
        message_id: String,
    },
    /// Retry delivery of a message
    Retry {
        /// Message ID to retry
        message_id: String,
        /// Force retry even if not failed
        force: bool,
    },
    /// Delete a message from the queue
    Delete {
        /// Message ID to delete
        message_id: String,
    },
    /// Get queue statistics
    Stats,
}

/// Response from the control server (versioned wrapper)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    /// Protocol version
    pub version: u32,
    /// The actual response payload
    pub payload: ResponsePayload,
}

/// Response payload types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ResponsePayload {
    /// Command succeeded
    Ok,
    /// Command succeeded with data
    Data(Box<ResponseData>),
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
    /// Queue message list
    QueueList(Vec<QueueMessage>),
    /// Queue message details
    QueueMessageDetails(QueueMessageDetails),
    /// Queue statistics
    QueueStats(QueueStats),
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

/// Queue message summary (for list command)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueMessage {
    /// Message ID
    pub id: String,
    /// Sender email address
    pub from: String,
    /// Recipient email addresses
    pub to: Vec<String>,
    /// Domain being delivered to
    pub domain: String,
    /// Delivery status
    pub status: String,
    /// Number of delivery attempts
    pub attempts: u32,
    /// Next retry time (Unix timestamp in seconds)
    pub next_retry: Option<u64>,
    /// Message size in bytes
    pub size: usize,
    /// Time message was spooled (Unix timestamp in seconds)
    pub spooled_at: u64,
}

/// Queue message details (for view command)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueMessageDetails {
    /// Message ID
    pub id: String,
    /// Sender email address
    pub from: String,
    /// Recipient email addresses
    pub to: Vec<String>,
    /// Domain being delivered to
    pub domain: String,
    /// Delivery status
    pub status: String,
    /// Number of delivery attempts
    pub attempts: u32,
    /// Next retry time (Unix timestamp in seconds)
    pub next_retry: Option<u64>,
    /// Last error message (if any)
    pub last_error: Option<String>,
    /// Message size in bytes
    pub size: usize,
    /// Time message was spooled (Unix timestamp in seconds)
    pub spooled_at: u64,
    /// Message headers
    pub headers: HashMap<String, String>,
    /// Message body preview (first 1KB)
    pub body_preview: String,
}

/// Queue statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueStats {
    /// Total messages in queue
    pub total: usize,
    /// Messages by status
    pub by_status: HashMap<String, usize>,
    /// Messages by domain
    pub by_domain: HashMap<String, usize>,
    /// Oldest message age in seconds
    pub oldest_message_age_secs: Option<u64>,
}

impl Request {
    /// Create a new request with the current protocol version
    #[must_use]
    pub const fn new(command: RequestCommand) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            command,
        }
    }

    /// Check if the request version is compatible with the current version
    #[must_use]
    pub const fn is_version_compatible(&self) -> bool {
        // For now, only exact version match is supported
        // Future: implement backward compatibility logic
        self.version == PROTOCOL_VERSION
    }
}

impl Response {
    /// Create an error response
    #[must_use]
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            payload: ResponsePayload::Error(message.into()),
        }
    }

    /// Create a success response with no data
    #[must_use]
    pub const fn ok() -> Self {
        Self {
            version: PROTOCOL_VERSION,
            payload: ResponsePayload::Ok,
        }
    }

    /// Create a response with data
    #[must_use]
    pub fn data(data: ResponseData) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            payload: ResponsePayload::Data(Box::new(data)),
        }
    }

    /// Check if the response indicates success (not an error)
    #[must_use]
    pub const fn is_success(&self) -> bool {
        !matches!(self.payload, ResponsePayload::Error(_))
    }

    /// Check if the response version is compatible with the current version
    #[must_use]
    pub const fn is_version_compatible(&self) -> bool {
        // For now, only exact version match is supported
        // Future: implement backward compatibility logic
        self.version == PROTOCOL_VERSION
    }
}
