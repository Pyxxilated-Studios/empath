use std::{
    borrow::Cow,
    fmt::{self, Debug, Display},
    sync::Arc,
    time::SystemTime,
};

use ahash::AHashMap;
use mailparse::MailAddr;
use serde::{Deserialize, Serialize};

use crate::{envelope::Envelope, status::Status};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Capability {
    Auth,
}

/// Status of a message in the delivery queue
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum DeliveryStatus {
    /// Message is pending delivery
    #[default]
    Pending,
    /// Message delivery is in progress
    InProgress,
    /// Message was successfully delivered
    Completed,
    /// Message delivery failed permanently
    Failed(String),
    /// Message delivery failed temporarily, will retry
    Retry { attempts: u32, last_error: String },
    /// Message expired before successful delivery
    Expired,
}

impl Display for DeliveryStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pending => write!(f, "pending"),
            Self::InProgress => write!(f, "inprogress"),
            Self::Completed => write!(f, "completed"),
            Self::Failed(_) => write!(f, "failed"),
            Self::Retry { .. } => write!(f, "retry"),
            Self::Expired => write!(f, "expired"),
        }
    }
}

impl DeliveryStatus {
    /// Check if this status matches the given filter string.
    ///
    /// The comparison is case-insensitive and matches against the display
    /// representation of the status (e.g., "pending", "failed", "retry").
    /// This provides a stable API contract unlike Debug formatting.
    #[must_use]
    pub fn matches_filter(&self, filter: &str) -> bool {
        self.to_string().eq_ignore_ascii_case(filter)
    }
}

/// Represents a single delivery attempt
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeliveryAttempt {
    /// Timestamp of the attempt
    pub timestamp: SystemTime,
    /// Error message if the attempt failed
    pub error: Option<String>,
    /// SMTP server that was contacted
    pub server: String,
}

/// Delivery-specific context information for outbound mail delivery.
///
/// This struct maintains the complete delivery state for a message throughout
/// its lifecycle, including retry tracking, attempt history, and status.
/// By storing this in the Context, we enable:
/// - Persistent queue state across restarts
/// - Module access to delivery metadata via the module API
/// - Single source of truth for message state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeliveryContext {
    /// The message ID being delivered
    pub message_id: String,
    /// The recipient domain being delivered to (Arc for cheap cloning)
    pub domain: Arc<str>,
    /// The mail server (MX host:port) being used for delivery
    pub server: Option<String>,
    /// Error message if delivery failed
    pub error: Option<String>,
    /// Number of delivery attempts made
    pub attempts: Option<u32>,

    // Queue state fields for persistent delivery tracking
    /// Current delivery status
    #[serde(default)]
    pub status: DeliveryStatus,
    /// List of delivery attempts with timestamps and errors
    #[serde(default)]
    pub attempt_history: Vec<DeliveryAttempt>,
    /// When this message was first queued for delivery
    #[serde(default = "default_system_time")]
    pub queued_at: SystemTime,
    /// When the next retry should be attempted (None for immediate retry)
    #[serde(default)]
    pub next_retry_at: Option<SystemTime>,
    /// Index of the current mail server being tried (for MX fallback)
    #[serde(default)]
    pub current_server_index: usize,
}

const fn default_system_time() -> SystemTime {
    SystemTime::UNIX_EPOCH
}

impl Default for DeliveryContext {
    fn default() -> Self {
        Self {
            message_id: String::new(),
            domain: Arc::from(""),
            server: None,
            error: None,
            attempts: None,
            status: DeliveryStatus::default(),
            attempt_history: Vec::new(),
            queued_at: SystemTime::UNIX_EPOCH,
            next_retry_at: None,
            current_server_index: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Context {
    pub extended: bool,
    pub envelope: Envelope,
    pub id: String,
    pub data: Option<Arc<[u8]>>,
    pub response: Option<(Status, Cow<'static, str>)>,
    /// Session metadata and custom attributes
    pub metadata: AHashMap<Cow<'static, str>, String>,
    /// Server banner/hostname for greeting messages
    pub banner: Arc<str>,
    /// Maximum message size in bytes (0 = unlimited)
    pub max_message_size: usize,
    pub capabilities: Vec<Capability>,
    /// Delivery-specific context (populated during outbound delivery)
    pub delivery: Option<DeliveryContext>,
    /// Spool tracking ID (assigned when message is spooled)
    pub tracking_id: Option<String>,
}

impl Default for Context {
    fn default() -> Self {
        Self {
            extended: false,
            envelope: Envelope::default(),
            id: String::new(),
            data: None,
            response: None,
            metadata: AHashMap::new(),
            banner: Arc::from(""),
            max_message_size: 0,
            capabilities: Vec::new(),
            delivery: None,
            tracking_id: None,
        }
    }
}

impl Context {
    /// Returns a reference to the id of this [`Context`].
    #[inline]
    pub const fn id(&self) -> &str {
        self.id.as_str()
    }

    #[inline]
    pub fn message(&self) -> String {
        self.data.as_deref().map_or_else(Default::default, |data| {
            charset::Charset::for_encoding(encoding_rs::UTF_8)
                .decode(data)
                .0
                .to_string()
        })
    }

    /// Returns the sender of this [`Context`].
    #[inline]
    pub fn sender(&self) -> String {
        self.envelope
            .sender()
            .map_or_default(|sender| match &**sender {
                MailAddr::Single(addr) => addr.to_string(),
                MailAddr::Group(_) => String::default(),
            })
    }

    /// Returns the recipients of this [`Context`].
    pub fn recipients(&self) -> Vec<String> {
        self.envelope.recipients().map_or_default(|addrs| {
            addrs
                .iter()
                .map(|addr| match &**addr {
                    mailparse::MailAddr::Group(group) => {
                        format!("RCPT TO:{}", group.group_name)
                    }
                    mailparse::MailAddr::Single(single) => {
                        format!(
                            "RCPT TO:{}{}",
                            single.display_name.as_deref().unwrap_or(""),
                            single.addr
                        )
                    }
                })
                .collect()
        })
    }
}
