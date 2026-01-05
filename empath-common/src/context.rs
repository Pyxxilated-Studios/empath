use std::borrow::Cow;
use std::fmt::{self, Debug, Display};
use std::sync::Arc;
use std::time::SystemTime;

use ahash::AHashMap;
use serde::{Deserialize, Serialize};
use ulid::Ulid;

use crate::email::{EmailAddress, ServerId};
use crate::envelope::Envelope;
use crate::message_api::MessageParts;
use crate::message_id::SpooledMessageId;
use crate::status::Status;

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

const TLS_CONTEXT_KEY: &str = "tls";
const TLS_PROTOCOL_CONTEXT_KEY: &str = "protocol";
const TLS_CIPHER_CONTEXT_KEY: &str = "cipher";

const AUTH_CONTEXT_KEY: &str = "auth";
const AUTH_MECHANISM_CONTEXT_KEY: &str = "auth_mechanism";
const AUTH_USER_CONTEXT_KEY: &str = "auth_user";
const AUTHZID_CONTEXT_KEY: &str = "authzid";
const AUTH_PASSWORD_CONTEXT_KEY: &str = "auth_password";

const SPF_RESULT_CONTEXT_KEY: &str = "spf_result";
const SPF_DOMAIN_CONTEXT_KEY: &str = "spf_domain";
const SPF_HELO_DOMAIN_CONTEXT_KEY: &str = "spf_helo_domain";
const SPF_EXPLANATION_CONTEXT_KEY: &str = "spf_explanation";

const DMARC_DOMAIN_CONTEXT_KEY: &str = "dmarc_domain";
const DMARC_RESULT_CONTEXT_KEY: &str = "dmarc_result";
const DMARC_POLICY_CONTEXT_KEY: &str = "dmarc_policy";
const DMARC_SPF_ALIGNED_CONTEXT_KEY: &str = "dmarc_spf_aligned";
const DMARC_DKIM_ALIGNED_CONTEXT_KEY: &str = "dmarc_dkim_aligned";
const DMARC_ERROR_CONTEXT_KEY: &str = "dmarc_error";
const DMARC_REASON_CONTEXT_KEY: &str = "dmarc_reason";
const DMARC_ACTION_CONTEXT_KEY: &str = "dmarc_action";
const DMARC_SAMPLED_CONTEXT_KEY: &str = "dmarc_sampled";
const DMARC_SAMPLE_RATE_BPS_CONTEXT_KEY: &str = "dmarc_sample_rate_bps";

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
    use std::sync::Arc;

    use super::{Context, DeliveryPriorityTier, DeliveryStatus};
    use crate::address::AddressList;
    use crate::address_parser::Mailbox;
    use crate::envelope::Envelope;

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

    #[test]
    fn tls_context_accessors_round_trip() {
        let mut context = Context::default();

        context.set_tls_info("TLS1.3", "AES256-GCM");
        assert!(context.is_tls());
        assert_eq!(context.tls_protocol(), Some("TLS1.3"));
        assert_eq!(context.tls_cipher(), Some("AES256-GCM"));

        context.clear_tls_info();
        assert!(!context.is_tls());
        assert_eq!(context.tls_protocol(), None);
        assert_eq!(context.tls_cipher(), None);
    }

    #[test]
    fn auth_context_accessors_round_trip() {
        let mut context = Context::default();

        context.set_authenticated(true);
        context.set_auth_mechanism("PLAIN");
        context.set_auth_user("alice");
        context.set_authzid(Some("admin".to_string()));
        context.set_auth_password("secret");

        assert!(context.is_authenticated());
        assert_eq!(context.auth_mechanism(), Some("PLAIN"));
        assert_eq!(context.auth_user(), Some("alice"));
        assert_eq!(context.authzid(), Some("admin"));
        assert_eq!(context.auth_password(), Some("secret"));

        context.set_authenticated(false);
        context.set_authzid(None);
        context.clear_auth_password();

        assert!(!context.is_authenticated());
        assert_eq!(context.authzid(), None);
        assert_eq!(context.auth_password(), None);
    }

    #[test]
    fn spf_dmarc_context_accessors_round_trip() {
        let mut context = Context::default();

        context.set_spf_result("pass");
        context.set_spf_domain("example.com");
        context.set_spf_helo_domain("mail.example.com");
        context.set_spf_explanation("ok");

        assert_eq!(context.spf_result(), Some("pass"));
        assert_eq!(context.spf_domain(), Some("example.com"));
        assert_eq!(context.spf_helo_domain(), Some("mail.example.com"));
        assert_eq!(context.spf_explanation(), Some("ok"));

        context.set_dmarc_domain("example.com");
        context.set_dmarc_result("pass");
        context.set_dmarc_policy("none");
        context.set_dmarc_error("none");
        context.set_dmarc_reason("aligned");
        context.set_dmarc_action("accept");
        context.set_dmarc_sample_rate_bps(2500);

        assert_eq!(context.dmarc_domain(), Some("example.com"));
        assert_eq!(context.dmarc_result(), Some("pass"));
        assert_eq!(context.dmarc_policy(), Some("none"));
        assert_eq!(context.dmarc_error(), Some("none"));
        assert_eq!(context.dmarc_reason(), Some("aligned"));
        assert_eq!(context.dmarc_action(), Some("accept"));
        assert_eq!(context.dmarc_sample_rate_bps(), Some("2500"));
    }

    #[test]
    fn metadata_round_trip_preserves_transactional_fields() {
        let mut envelope = Envelope::default();
        *envelope.sender_mut() = Some(
            Mailbox {
                local_part: "sender".to_string(),
                domain: "example.com".to_string(),
            }
            .into(),
        );
        *envelope.recipients_mut() = Some(AddressList(vec![
            Mailbox {
                local_part: "rcpt".to_string(),
                domain: "example.net".to_string(),
            }
            .into(),
        ]));

        let context = Context::builder()
            .with_envelope(Arc::new(envelope))
            .with_id("ctx-1")
            .with_banner("banner.example.com")
            .with_max_message_size(1024)
            .with_transaction_value(std::borrow::Cow::Borrowed("spf_result"), "pass")
            .build();

        let metadata = context.to_metadata();
        let rebuilt = Context::from_metadata(metadata);

        assert_eq!(rebuilt.id(), "ctx-1");
        assert_eq!(rebuilt.banner.as_ref(), "banner.example.com");
        assert_eq!(rebuilt.max_message_size, 1024);
        assert_eq!(rebuilt.sender(), "sender@example.com");
        assert_eq!(rebuilt.recipients(), vec!["RCPT TO:rcpt@example.net"]);
        assert_eq!(rebuilt.spf_result(), Some("pass"));
    }

    #[test]
    fn message_helpers_use_cached_length_and_lossy_decode() {
        let data = Arc::from(b"Body\xff".as_slice());
        let context = Context::builder().with_data(Some(data)).build();

        assert_eq!(context.data_len, Some(5));
        assert!(context.message().starts_with("Body"));
        assert_eq!(context.message_cow(), "Body\u{FFFD}");
    }

    #[test]
    fn delivery_status_labels_and_filters() {
        let status = DeliveryStatus::Retry {
            attempts: 2,
            last_error: "temp".to_string(),
        };

        assert_eq!(status.label(), "Retry");
        assert!(status.matches_filter("retry"));
        assert!(!status.matches_filter("failed"));
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
    /// Audit logger for message lifecycle events.
    #[serde(skip)]
    pub audit_logger: Option<Arc<dyn crate::audit::AuditLogger>>,
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
            audit_logger: None,
        }
    }
}

impl Context {
    /// Create a builder for ergonomic `Context` construction.
    #[must_use]
    pub fn builder() -> ContextBuilder {
        ContextBuilder::default()
    }

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
            audit_logger: None,
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

    fn set_session_value(&mut self, key: &'static str, value: impl Into<String>) {
        self.session.insert(Cow::Borrowed(key), value.into());
    }

    fn session_value(&self, key: &'static str) -> Option<&str> {
        self.session.get(key).map(String::as_str)
    }

    fn remove_session_value(&mut self, key: &'static str) {
        self.session.remove(key);
    }

    fn set_transaction_value(&mut self, key: &'static str, value: impl Into<String>) {
        self.transaction.insert(Cow::Borrowed(key), value.into());
    }

    fn transaction_value(&self, key: &'static str) -> Option<&str> {
        self.transaction.get(key).map(String::as_str)
    }

    /// Returns true if TLS is active for this session.
    #[must_use]
    pub fn is_tls(&self) -> bool {
        self.session_value(TLS_CONTEXT_KEY)
            .is_some_and(|value| value == "true")
    }

    /// Returns the TLS protocol, if recorded.
    #[must_use]
    pub fn tls_protocol(&self) -> Option<&str> {
        self.session_value(TLS_PROTOCOL_CONTEXT_KEY)
    }

    /// Returns the TLS cipher, if recorded.
    #[must_use]
    pub fn tls_cipher(&self) -> Option<&str> {
        self.session_value(TLS_CIPHER_CONTEXT_KEY)
    }

    /// Record TLS state and negotiated details for this session.
    pub fn set_tls_info(&mut self, protocol: impl Into<String>, cipher: impl Into<String>) {
        self.set_session_value(TLS_CONTEXT_KEY, "true");
        self.set_session_value(TLS_PROTOCOL_CONTEXT_KEY, protocol);
        self.set_session_value(TLS_CIPHER_CONTEXT_KEY, cipher);
    }

    /// Clear TLS metadata from this session.
    pub fn clear_tls_info(&mut self) {
        self.remove_session_value(TLS_CONTEXT_KEY);
        self.remove_session_value(TLS_PROTOCOL_CONTEXT_KEY);
        self.remove_session_value(TLS_CIPHER_CONTEXT_KEY);
    }

    /// Returns true if authentication is marked as successful.
    #[must_use]
    pub fn is_authenticated(&self) -> bool {
        self.session_value(AUTH_CONTEXT_KEY)
            .is_some_and(|value| value == "true")
    }

    /// Record authentication success or clear the auth marker.
    pub fn set_authenticated(&mut self, authenticated: bool) {
        if authenticated {
            self.set_session_value(AUTH_CONTEXT_KEY, "true");
        } else {
            self.remove_session_value(AUTH_CONTEXT_KEY);
        }
    }

    /// Record the SMTP AUTH mechanism in use.
    pub fn set_auth_mechanism(&mut self, mechanism: impl Into<String>) {
        self.set_session_value(AUTH_MECHANISM_CONTEXT_KEY, mechanism);
    }

    /// Record the authenticated user.
    pub fn set_auth_user(&mut self, user: impl Into<String>) {
        self.set_session_value(AUTH_USER_CONTEXT_KEY, user);
    }

    /// Record the optional authzid for SMTP AUTH.
    pub fn set_authzid(&mut self, authzid: Option<String>) {
        if let Some(authzid) = authzid {
            self.set_session_value(AUTHZID_CONTEXT_KEY, authzid);
        } else {
            self.remove_session_value(AUTHZID_CONTEXT_KEY);
        }
    }

    /// Record the authentication password (ephemeral).
    pub fn set_auth_password(&mut self, password: impl Into<String>) {
        self.set_session_value(AUTH_PASSWORD_CONTEXT_KEY, password);
    }

    /// Remove the stored authentication password.
    pub fn clear_auth_password(&mut self) {
        self.remove_session_value(AUTH_PASSWORD_CONTEXT_KEY);
    }

    /// Returns the authentication mechanism, if any.
    #[must_use]
    pub fn auth_mechanism(&self) -> Option<&str> {
        self.session_value(AUTH_MECHANISM_CONTEXT_KEY)
    }

    /// Returns the authenticated user, if any.
    #[must_use]
    pub fn auth_user(&self) -> Option<&str> {
        self.session_value(AUTH_USER_CONTEXT_KEY)
    }

    /// Returns the authzid, if any.
    #[must_use]
    pub fn authzid(&self) -> Option<&str> {
        self.session_value(AUTHZID_CONTEXT_KEY)
    }

    /// Returns the authentication password, if recorded.
    #[must_use]
    pub fn auth_password(&self) -> Option<&str> {
        self.session_value(AUTH_PASSWORD_CONTEXT_KEY)
    }

    /// Store SPF evaluation result.
    pub fn set_spf_result(&mut self, result: impl Into<String>) {
        self.set_transaction_value(SPF_RESULT_CONTEXT_KEY, result);
    }

    /// Returns the SPF evaluation result.
    #[must_use]
    pub fn spf_result(&self) -> Option<&str> {
        self.transaction_value(SPF_RESULT_CONTEXT_KEY)
    }

    /// Store SPF sender domain.
    pub fn set_spf_domain(&mut self, domain: impl Into<String>) {
        self.set_transaction_value(SPF_DOMAIN_CONTEXT_KEY, domain);
    }

    /// Returns the SPF sender domain.
    #[must_use]
    pub fn spf_domain(&self) -> Option<&str> {
        self.transaction_value(SPF_DOMAIN_CONTEXT_KEY)
    }

    /// Store SPF HELO domain.
    pub fn set_spf_helo_domain(&mut self, domain: impl Into<String>) {
        self.set_transaction_value(SPF_HELO_DOMAIN_CONTEXT_KEY, domain);
    }

    /// Returns the SPF HELO domain.
    #[must_use]
    pub fn spf_helo_domain(&self) -> Option<&str> {
        self.transaction_value(SPF_HELO_DOMAIN_CONTEXT_KEY)
    }

    /// Store SPF explanation.
    pub fn set_spf_explanation(&mut self, explanation: impl Into<String>) {
        self.set_transaction_value(SPF_EXPLANATION_CONTEXT_KEY, explanation);
    }

    /// Returns the SPF explanation, if any.
    #[must_use]
    pub fn spf_explanation(&self) -> Option<&str> {
        self.transaction_value(SPF_EXPLANATION_CONTEXT_KEY)
    }

    /// Store DMARC domain.
    pub fn set_dmarc_domain(&mut self, domain: impl Into<String>) {
        self.set_transaction_value(DMARC_DOMAIN_CONTEXT_KEY, domain);
    }

    /// Returns the DMARC domain.
    #[must_use]
    pub fn dmarc_domain(&self) -> Option<&str> {
        self.transaction_value(DMARC_DOMAIN_CONTEXT_KEY)
    }

    /// Store DMARC result.
    pub fn set_dmarc_result(&mut self, result: impl Into<String>) {
        self.set_transaction_value(DMARC_RESULT_CONTEXT_KEY, result);
    }

    /// Returns the DMARC result.
    #[must_use]
    pub fn dmarc_result(&self) -> Option<&str> {
        self.transaction_value(DMARC_RESULT_CONTEXT_KEY)
    }

    /// Store DMARC policy.
    pub fn set_dmarc_policy(&mut self, policy: impl Into<String>) {
        self.set_transaction_value(DMARC_POLICY_CONTEXT_KEY, policy);
    }

    /// Returns the DMARC policy.
    #[must_use]
    pub fn dmarc_policy(&self) -> Option<&str> {
        self.transaction_value(DMARC_POLICY_CONTEXT_KEY)
    }

    /// Store DMARC SPF alignment.
    pub fn set_dmarc_spf_aligned(&mut self, aligned: bool) {
        self.set_transaction_value(DMARC_SPF_ALIGNED_CONTEXT_KEY, aligned.to_string());
    }

    /// Store DMARC DKIM alignment.
    pub fn set_dmarc_dkim_aligned(&mut self, aligned: bool) {
        self.set_transaction_value(DMARC_DKIM_ALIGNED_CONTEXT_KEY, aligned.to_string());
    }

    /// Store DMARC error details.
    pub fn set_dmarc_error(&mut self, error: impl Into<String>) {
        self.set_transaction_value(DMARC_ERROR_CONTEXT_KEY, error);
    }

    /// Returns the DMARC error, if any.
    #[must_use]
    pub fn dmarc_error(&self) -> Option<&str> {
        self.transaction_value(DMARC_ERROR_CONTEXT_KEY)
    }

    /// Store DMARC reason.
    pub fn set_dmarc_reason(&mut self, reason: impl Into<String>) {
        self.set_transaction_value(DMARC_REASON_CONTEXT_KEY, reason);
    }

    /// Returns the DMARC reason, if any.
    #[must_use]
    pub fn dmarc_reason(&self) -> Option<&str> {
        self.transaction_value(DMARC_REASON_CONTEXT_KEY)
    }

    /// Store DMARC enforcement action.
    pub fn set_dmarc_action(&mut self, action: impl Into<String>) {
        self.set_transaction_value(DMARC_ACTION_CONTEXT_KEY, action);
    }

    /// Returns the DMARC enforcement action, if any.
    #[must_use]
    pub fn dmarc_action(&self) -> Option<&str> {
        self.transaction_value(DMARC_ACTION_CONTEXT_KEY)
    }

    /// Store DMARC sampled flag.
    pub fn set_dmarc_sampled(&mut self, sampled: bool) {
        self.set_transaction_value(DMARC_SAMPLED_CONTEXT_KEY, sampled.to_string());
    }

    /// Store DMARC sample rate (basis points).
    pub fn set_dmarc_sample_rate_bps(&mut self, rate: u32) {
        self.set_transaction_value(DMARC_SAMPLE_RATE_BPS_CONTEXT_KEY, rate.to_string());
    }

    /// Returns the DMARC sample rate (basis points), if any.
    #[must_use]
    pub fn dmarc_sample_rate_bps(&self) -> Option<&str> {
        self.transaction_value(DMARC_SAMPLE_RATE_BPS_CONTEXT_KEY)
    }
}

#[derive(Debug, Default)]
pub struct ContextBuilder {
    context: Context,
}

impl ContextBuilder {
    #[must_use]
    pub fn with_envelope(mut self, envelope: Arc<Envelope>) -> Self {
        self.context.envelope = envelope;
        self
    }

    #[must_use]
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.context.id = id.into();
        self
    }

    #[must_use]
    pub fn with_data(mut self, data: Option<Arc<[u8]>>) -> Self {
        self.context.data = data;
        if self.context.data_len.is_none() {
            self.context.data_len = self.context.data.as_ref().map(|d| d.len());
        }
        self
    }

    #[must_use]
    pub fn with_response(mut self, response: Option<(Status, Cow<'static, str>)>) -> Self {
        self.context.response = response;
        self
    }

    #[must_use]
    pub fn with_session_value(mut self, key: Cow<'static, str>, value: impl Into<String>) -> Self {
        self.context.session.insert(key, value.into());
        self
    }

    #[must_use]
    pub fn with_transaction_value(
        mut self,
        key: Cow<'static, str>,
        value: impl Into<String>,
    ) -> Self {
        self.context.transaction.insert(key, value.into());
        self
    }

    #[must_use]
    pub fn with_banner(mut self, banner: impl Into<String>) -> Self {
        self.context.banner = Arc::from(banner.into());
        self
    }

    #[must_use]
    pub const fn with_max_message_size(mut self, max_message_size: usize) -> Self {
        self.context.max_message_size = max_message_size;
        self
    }

    #[must_use]
    pub fn with_capabilities(mut self, capabilities: Vec<Capability>) -> Self {
        self.context.capabilities = capabilities;
        self
    }

    #[must_use]
    pub fn with_delivery(mut self, delivery: Option<DeliveryContext>) -> Self {
        self.context.delivery = delivery;
        self
    }

    #[must_use]
    pub const fn with_tracking_id(mut self, tracking_id: Option<SpooledMessageId>) -> Self {
        self.context.tracking_id = tracking_id;
        self
    }

    #[must_use]
    pub fn with_audit_logger(
        mut self,
        audit_logger: Option<Arc<dyn crate::audit::AuditLogger>>,
    ) -> Self {
        self.context.audit_logger = audit_logger;
        self
    }

    #[must_use]
    pub fn build(self) -> Context {
        self.context
    }
}
