use std::{
    borrow::Cow,
    fmt::{self, Debug, Display},
    sync::Arc,
    time::SystemTime,
};

use ahash::AHashMap;
use serde::{Deserialize, Serialize};
use ulid::Ulid;

use crate::{
    email::{EmailAddress, ServerId},
    envelope::Envelope,
    message_api::MessageParts,
    message_id::SpooledMessageId,
    status::Status,
};

#[derive(Debug, Clone)]
pub struct ModuleContext {
    pub module_type: String,
    pub module_name: String,
}

impl ModuleContext {
    #[must_use]
    pub fn new(module_type: impl Into<String>, module_name: impl Into<String>) -> Self {
        Self {
            module_type: module_type.into(),
            module_name: module_name.into(),
        }
    }
}

/// Transaction context key used to request a specific outbound binding.
pub const OUTBOUND_BINDING_CONTEXT_KEY: &str = "outbound_binding";
/// Transaction context key used to set delivery priority tier.
pub const PRIORITY_TIER_CONTEXT_KEY: &str = "priority_tier";
/// Transaction context key used to persist OpenTelemetry trace IDs.
pub const TRACE_ID_CONTEXT_KEY: &str = "trace_id";
/// Transaction context key used to persist OpenTelemetry span IDs.
pub const SPAN_ID_CONTEXT_KEY: &str = "span_id";

/// Delivery priority tier for queue scheduling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum DeliveryPriorityTier {
    High,
    #[default]
    Normal,
    Low,
}

impl DeliveryPriorityTier {
    #[must_use]
    pub const fn rank(self) -> u8 {
        match self {
            Self::High => 0,
            Self::Normal => 1,
            Self::Low => 2,
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "high" | "0" => Some(Self::High),
            "normal" | "1" => Some(Self::Normal),
            "low" | "2" => Some(Self::Low),
            _ => None,
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::High => "high",
            Self::Normal => "normal",
            Self::Low => "low",
        }
    }
}

impl Display for DeliveryPriorityTier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::DeliveryPriorityTier;

    #[test]
    fn parse_priority_tier() {
        assert_eq!(
            DeliveryPriorityTier::parse("high"),
            Some(DeliveryPriorityTier::High)
        );
        assert_eq!(
            DeliveryPriorityTier::parse("normal"),
            Some(DeliveryPriorityTier::Normal)
        );
        assert_eq!(
            DeliveryPriorityTier::parse("low"),
            Some(DeliveryPriorityTier::Low)
        );
        assert_eq!(
            DeliveryPriorityTier::parse("0"),
            Some(DeliveryPriorityTier::High)
        );
        assert_eq!(
            DeliveryPriorityTier::parse("1"),
            Some(DeliveryPriorityTier::Normal)
        );
        assert_eq!(
            DeliveryPriorityTier::parse("2"),
            Some(DeliveryPriorityTier::Low)
        );
        assert_eq!(DeliveryPriorityTier::parse("other"), None);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum Capability {
    Auth,
}

impl Capability {
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Auth => "AUTH",
        }
    }

    #[must_use]
    pub const fn parse(value: &str) -> Option<Self> {
        if value.eq_ignore_ascii_case("AUTH") {
            Some(Self::Auth)
        } else {
            None
        }
    }
}

impl Display for Capability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
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

pub static PENDING_STR: &str = "Pending";
pub static IN_PROGRESS_STR: &str = "In Progress";
pub static COMPLETED_STR: &str = "Completed";
pub static FAILED_STR: &str = "Failed";
pub static RETRY_STR: &str = "Retry";
pub static EXPIRED_STR: &str = "Expired";

impl Display for DeliveryStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pending => write!(f, "{PENDING_STR}"),
            Self::InProgress => write!(f, "{IN_PROGRESS_STR}"),
            Self::Completed => write!(f, "{COMPLETED_STR}"),
            Self::Failed(_) => write!(f, "{FAILED_STR}"),
            Self::Retry { .. } => write!(f, "{RETRY_STR}"),
            Self::Expired => write!(f, "{EXPIRED_STR}"),
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

    /// Human-readable label that can be reused without allocations.
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            Self::Pending => PENDING_STR,
            Self::InProgress => IN_PROGRESS_STR,
            Self::Completed => COMPLETED_STR,
            Self::Failed(_) => FAILED_STR,
            Self::Retry { .. } => RETRY_STR,
            Self::Expired => EXPIRED_STR,
        }
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
    pub server: ServerId,
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
    pub message_id: SpooledMessageId,
    /// The recipient domain being delivered to
    pub domain: crate::domain::Domain,
    /// The mail server (MX host:port) being used for delivery
    pub server: Option<ServerId>,
    /// Error message if delivery failed
    pub error: Option<String>,
    /// Number of delivery attempts made
    pub attempts: Option<u32>,
    /// Selected outbound binding name (if any)
    #[serde(default)]
    pub binding: Option<String>,

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
    /// Delivery priority tier for scheduling.
    #[serde(default)]
    pub priority_tier: DeliveryPriorityTier,
}

const fn default_system_time() -> SystemTime {
    SystemTime::UNIX_EPOCH
}

impl Default for DeliveryContext {
    fn default() -> Self {
        Self {
            // Defaults should be deterministic; use nil ULID as a neutral placeholder.
            message_id: SpooledMessageId::new(Ulid::nil()),
            domain: crate::domain::Domain::new(""),
            server: None,
            error: None,
            attempts: None,
            binding: None,
            status: DeliveryStatus::default(),
            attempt_history: Vec::new(),
            queued_at: SystemTime::UNIX_EPOCH,
            next_retry_at: None,
            current_server_index: 0,
            priority_tier: DeliveryPriorityTier::default(),
        }
    }
}

/// Lightweight persisted context snapshot for spooling.
///
/// This omits session-only state and cached message parsing to keep metadata
/// storage and serialization lean.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextMetadata {
    pub extended: bool,
    pub envelope: Arc<Envelope>,
    pub id: String,
    pub data_len: Option<usize>,
    pub response: Option<(Status, Cow<'static, str>)>,
    pub transaction: AHashMap<Cow<'static, str>, String>,
    pub banner: Arc<str>,
    pub max_message_size: usize,
    pub capabilities: Vec<Capability>,
    pub delivery: Option<DeliveryContext>,
    pub tracking_id: Option<SpooledMessageId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Context {
    pub extended: bool,
    pub envelope: Arc<Envelope>,
    pub id: String,
    pub data: Option<Arc<[u8]>>,
    /// Cached body length when available to avoid recomputing sizes.
    pub data_len: Option<usize>,
    pub response: Option<(Status, Cow<'static, str>)>,
    /// Cached parsed message parts (headers/body) for mutation.
    #[serde(skip)]
    pub message_parts: Option<MessageParts>,
    /// Session-level context (persists across RSET, tied to connection lifetime)
    ///
    /// Examples: TLS state ("tls", "protocol", "cipher"), connection properties,
    /// client authentication state, session-wide tracking info
    ///
    /// **Not persisted to spool** - session context is ephemeral and only relevant
    /// during active SMTP sessions. Spooled messages only need transaction-level data.
    #[serde(skip)]
    pub session: AHashMap<Cow<'static, str>, String>,
    /// Transaction-level context (cleared on RSET, specific to current mail transaction)
    ///
    /// Examples: declared SIZE parameter, spam scores, per-message validation results,
    /// recipient-specific data, DNS lookup results
    ///
    /// **Persisted to spool** - this data travels with the message through delivery
    /// and is available to delivery modules via the FFI API.
    pub transaction: AHashMap<Cow<'static, str>, String>,
    /// Server banner/hostname for greeting messages
    pub banner: Arc<str>,
    /// Maximum message size in bytes (0 = unlimited)
    pub max_message_size: usize,
    pub capabilities: Vec<Capability>,
    /// Delivery-specific context (populated during outbound delivery)
    pub delivery: Option<DeliveryContext>,
    /// Spool tracking ID (assigned when message is spooled)
    pub tracking_id: Option<SpooledMessageId>,
    /// Tracks whether parsed message parts have been mutated and need persistence.
    #[serde(skip)]
    pub message_dirty: bool,
    /// Active module identity for mutation audit/metrics.
    #[serde(skip)]
    pub module_context: Option<ModuleContext>,
}

impl Default for Context {
    fn default() -> Self {
        Self {
            extended: false,
            envelope: Arc::new(Envelope::default()),
            id: String::new(),
            data: None,
            data_len: None,
            response: None,
            message_parts: None,
            session: AHashMap::new(),
            transaction: AHashMap::new(),
            banner: Arc::from(""),
            max_message_size: 0,
            capabilities: Vec::new(),
            delivery: None,
            tracking_id: None,
            message_dirty: false,
            module_context: None,
        }
    }
}

impl Context {
    /// Build a lightweight metadata snapshot for spooling.
    #[must_use]
    pub fn to_metadata(&self) -> ContextMetadata {
        ContextMetadata {
            extended: self.extended,
            envelope: Arc::clone(&self.envelope),
            id: self.id.clone(),
            data_len: self
                .data_len
                .or_else(|| self.data.as_ref().map(|d| d.len())),
            response: self.response.clone(),
            transaction: self.transaction.clone(),
            banner: Arc::clone(&self.banner),
            max_message_size: self.max_message_size,
            capabilities: self.capabilities.clone(),
            delivery: self.delivery.clone(),
            tracking_id: self.tracking_id.clone(),
        }
    }

    /// Reconstruct a `Context` from a metadata snapshot.
    #[must_use]
    pub fn from_metadata(metadata: ContextMetadata) -> Self {
        Self {
            extended: metadata.extended,
            envelope: metadata.envelope,
            id: metadata.id,
            data: None,
            data_len: metadata.data_len,
            response: metadata.response,
            message_parts: None,
            session: AHashMap::new(),
            transaction: metadata.transaction,
            banner: metadata.banner,
            max_message_size: metadata.max_message_size,
            capabilities: metadata.capabilities,
            delivery: metadata.delivery,
            tracking_id: metadata.tracking_id,
            message_dirty: false,
            module_context: None,
        }
    }

    /// Returns a reference to the id of this [`Context`].
    #[inline]
    pub const fn id(&self) -> &str {
        self.id.as_str()
    }

    #[inline]
    pub fn message(&self) -> String {
        self.message_cow().into_owned()
    }

    /// Returns a lossy UTF-8 view of the message body without forcing allocation.
    #[inline]
    pub fn message_cow(&self) -> Cow<'_, str> {
        self.data.as_deref().map_or_else(
            || Cow::Borrowed(""),
            |data| {
                charset::Charset::for_encoding(encoding_rs::UTF_8)
                    .decode(data)
                    .0
            },
        )
    }

    /// Returns the sender of this [`Context`].
    #[inline]
    pub fn sender(&self) -> String {
        self.sender_email()
            .map_or_else(String::new, ToString::to_string)
    }

    /// Returns the sender as a validated [`EmailAddress`].
    #[inline]
    pub fn sender_email(&self) -> Option<&EmailAddress> {
        self.envelope.sender().map(super::address::Address::email)
    }

    /// Returns the recipients of this [`Context`] as protocol-formatted strings.
    pub fn recipients(&self) -> Vec<String> {
        self.envelope.recipients().map_or_else(Vec::new, |addrs| {
            let mut recipients = Vec::with_capacity(addrs.len());
            for addr in addrs.iter() {
                recipients.push(format!("RCPT TO:{addr}"));
            }
            recipients
        })
    }

    /// Returns an iterator over recipient addresses without allocating.
    pub fn recipient_addresses_iter(&self) -> impl Iterator<Item = &crate::address::Address> + '_ {
        self.envelope
            .recipients()
            .into_iter()
            .flat_map(|addrs| addrs.iter())
    }

    /// Returns an iterator over recipient emails without allocating.
    pub fn recipient_emails_iter(&self) -> impl Iterator<Item = &EmailAddress> + '_ {
        self.recipient_addresses_iter()
            .map(crate::address::Address::email)
    }

    /// Returns the recipients as validated [`EmailAddress`] values.
    #[inline]
    pub fn recipient_emails(&self) -> Vec<EmailAddress> {
        self.envelope.recipients().map_or_else(Vec::new, |addrs| {
            addrs.iter().map(|addr| addr.email().clone()).collect()
        })
    }

    /// Load and cache parsed message parts for mutation.
    ///
    /// Returns `Ok(None)` if no message data is present.
    ///
    /// # Errors
    /// Returns `MessageParseError` if the raw message cannot be parsed.
    fn ensure_message_parts(&mut self) -> Result<(), crate::error::MessageParseError> {
        if self.message_parts.is_none()
            && let Some(raw) = &self.data
        {
            let parts = MessageParts::from_bytes(raw)?;
            self.message_parts = Some(parts);
        }
        Ok(())
    }

    /// Run a mutable operation over message parts and persist back into `data`.
    ///
    /// Returns `Ok(None)` when no message data is present.
    ///
    /// # Errors
    /// Returns `MessageParseError` if the message body cannot be parsed.
    pub fn with_message_parts_mut<F, R>(
        &mut self,
        mut f: F,
    ) -> Result<Option<R>, crate::error::MessageParseError>
    where
        F: FnMut(&mut MessageParts) -> R,
    {
        self.ensure_message_parts()?;
        let Some(parts) = self.message_parts.as_mut() else {
            return Ok(None);
        };

        let result = f(parts);
        self.message_dirty = parts.is_dirty();
        self.persist_message_parts();
        Ok(Some(result))
    }

    /// Persist cached message parts back into the raw message buffer.
    ///
    /// Call after header/body mutations to update `data`.
    pub fn persist_message_parts(&mut self) {
        if self.message_dirty {
            if let Some(parts) = &mut self.message_parts {
                self.data = Some(Arc::from(parts.clone().into_bytes()));
                parts.clear_dirty();
            }
            self.message_dirty = false;
        }
    }

    /// Convenience to fetch a header-only view without cloning the full message.
    /// Convenience to get a headers view (non-mutating).
    ///
    /// # Errors
    /// Returns `MessageParseError` if the message cannot be parsed into parts.
    pub fn headers_view(
        &mut self,
    ) -> Result<Option<crate::message_api::HeadersView<'_>>, crate::error::MessageParseError> {
        self.ensure_message_parts()?;
        Ok(self
            .message_parts
            .as_ref()
            .map(crate::message_api::MessageParts::headers_view))
    }

    /// Convenience to fetch a body view (non-mutating).
    ///
    /// # Errors
    /// Returns `MessageParseError` if the message cannot be parsed into parts.
    pub fn body_view(
        &mut self,
    ) -> Result<Option<crate::message_api::BodyView<'_>>, crate::error::MessageParseError> {
        self.ensure_message_parts()?;
        Ok(self
            .message_parts
            .as_ref()
            .map(crate::message_api::MessageParts::body_view))
    }
}
