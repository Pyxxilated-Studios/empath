use std::borrow::Cow;
use std::net::SocketAddr;
use std::sync::Arc;

use ahash::AHashMap;
use empath_common::audit::{AuditLogger, AuditLoggerBuilder};
use empath_common::context::Capability;
use empath_common::email::BannerHostname;
use empath_common::tracing;
use empath_spool::BackingStore;
use empath_tracing::traced;
use serde::Deserialize;
use typed_builder::TypedBuilder;

use crate::State;
use crate::extensions::Extension;
use crate::tls::TlsContext;
use crate::transaction::{DefaultSmtpTransactionHandler, SmtpTransactionHandler};

// Submodules containing implementation details
mod io;
mod response;
#[cfg(test)]
mod tests;

#[repr(C)]
#[derive(PartialEq, Eq, Debug)]
pub enum Event {
    ConnectionClose,
    ConnectionKeepAlive,
}

#[derive(Debug, Default, Clone, Deserialize)]
pub struct Context {
    pub state: State,
    pub message: Vec<u8>,
    pub sent: bool,
    pub auth_pending: Option<AuthPending>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AuthPending {
    pub mechanism: String,
}

pub type Response = (Option<Vec<String>>, Event);

/// Configuration for creating an SMTP session.
///
/// Use [`SessionConfig::builder()`] to construct instances with the type-safe builder pattern.
#[derive(TypedBuilder)]
#[builder(build_method(into = SessionConfig))]
pub struct SessionConfig {
    /// SMTP extensions supported by this session
    #[builder(default)]
    pub extensions: Vec<Extension>,

    /// SMTP capabilities supported by this session
    #[builder(default)]
    pub capabilities: Vec<Capability>,

    /// TLS context for STARTTLS support
    #[builder(default)]
    pub tls_context: Option<TlsContext>,

    /// Require DKIM verification to pass before accepting message data
    #[builder(default = false)]
    pub require_dkim: bool,

    /// Enforce DMARC policy results when evaluation fails
    #[builder(default = false)]
    pub dmarc_enforce: bool,

    /// Spool controller for message persistence
    #[builder(default)]
    pub spool: Option<Arc<dyn BackingStore>>,

    /// Server banner hostname
    #[builder(default = default_banner())]
    pub banner: BannerHostname,

    /// Initial context key-value pairs
    #[builder(default)]
    pub init_context: AHashMap<Cow<'static, str>, String>,

    /// Timeout configuration for this session
    #[builder(default)]
    pub timeouts: crate::SmtpServerTimeouts,

    /// SPF DNS resolver for this session
    #[builder(default)]
    pub spf_dns_resolver: Option<Arc<dyn crate::spf::SpfDnsResolver>>,

    /// SPF configuration for this session
    #[builder(default)]
    pub spf_config: crate::spf::SpfConfig,

    /// Transaction handler for this session.
    /// If not provided, [`DefaultSmtpTransactionHandler`] will be used.
    #[builder(default)]
    pub transaction_handler: Option<Box<dyn SmtpTransactionHandler>>,

    /// Audit logger for message lifecycle events
    #[builder(default)]
    pub audit_logger: Option<Arc<dyn AuditLogger>>,
}

/// Default banner hostname for sessions.
fn default_banner() -> BannerHostname {
    BannerHostname::new("localhost").unwrap_or_else(|_| {
        // Fallback should always succeed; if validation changes, use empty hostname
        BannerHostname::new("").unwrap_or_else(|_| BannerHostname::new_unchecked("localhost"))
    })
}

impl std::fmt::Debug for SessionConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionConfig")
            .field("extensions", &self.extensions)
            .field("capabilities", &self.capabilities)
            .field("tls_context", &self.tls_context)
            .field("require_dkim", &self.require_dkim)
            .field("dmarc_enforce", &self.dmarc_enforce)
            .field("spool", &self.spool.as_ref().map(|_| "<spool>"))
            .field("banner", &self.banner)
            .field("init_context", &self.init_context)
            .field("timeouts", &self.timeouts)
            .field(
                "spf_dns_resolver",
                &self.spf_dns_resolver.as_ref().map(|_| "<resolver>"),
            )
            .field("spf_config", &self.spf_config)
            .field(
                "transaction_handler",
                &self.transaction_handler.as_ref().map(|_| "<handler>"),
            )
            .field(
                "audit_logger",
                &self.audit_logger.as_ref().map(|_| "<audit_logger>"),
            )
            .finish()
    }
}

/// SMTP Protocol Session State
///
/// Holds only protocol-level state (FSM, message buffer, configuration).
/// No I/O operations - those are handled by `SmtpSessionOrchestrator`.
///
/// This separation enables:
/// - Pure protocol state machine
/// - Testable FSM without I/O
/// - Clear separation of concerns
pub struct Session {
    /// Protocol state (FSM state, message buffer, sent flag)
    pub(crate) context: Context,

    /// SMTP extensions supported by this session
    pub(crate) extensions: Vec<Extension>,
    /// SMTP capabilities supported by this session
    pub(crate) capabilities: Vec<Capability>,

    /// Server banner for greetings
    pub(crate) banner: Arc<str>,

    /// Initial context (session-level key-value pairs)
    pub(crate) init_context: Arc<AHashMap<Cow<'static, str>, String>>,

    /// Maximum message size in bytes as advertised via SIZE extension (RFC 1870).
    ///
    /// A value of 0 means no size limit is enforced (unlimited).
    ///
    /// This is validated at two points:
    /// 1. **MAIL FROM**: Against declared SIZE parameter (RFC 1870 Section 4)
    /// 2. **DATA**: Against actual received bytes (RFC 1870 Section 5)
    ///
    /// When the limit is exceeded, the server rejects with SMTP status code 552
    /// (Exceeded Storage Allocation).
    pub(crate) max_message_size: usize,

    /// Audit logger for message lifecycle events.
    pub(crate) audit_logger: Arc<dyn AuditLogger>,
}

impl Session {
    /// Create a new SMTP session
    ///
    /// Returns both the Session (protocol state) and components needed by the orchestrator.
    #[traced(instrument(level = tracing::Level::TRACE, skip_all), timing)]
    pub(crate) fn create(
        peer: SocketAddr,
        config: SessionConfig,
    ) -> (Self, Box<dyn SmtpTransactionHandler>, Option<TlsContext>) {
        tracing::debug!("Config: {:?}", config);
        tracing::debug!("Extensions: {:?}", config.extensions);

        // Extract max message size from SIZE extension
        let max_message_size = config
            .extensions
            .iter()
            .find_map(|ext| match ext {
                Extension::Size(size) => Some(*size),
                _ => None,
            })
            .unwrap_or(0);

        tracing::debug!("Max message size: {max_message_size}");

        let tls_context = config.extensions.iter().find_map(|ext| match ext {
            Extension::Starttls(profile_set) => match profile_set.active_context() {
                Ok(context) => Some(context),
                Err(err) => {
                    tracing::error!(error = %err, "Invalid active TLS profile");
                    None
                }
            },
            _ => None,
        });

        let audit_logger = config
            .audit_logger
            .unwrap_or_else(|| AuditLoggerBuilder::default().build());

        // Use provided transaction handler or create default
        let transaction_handler = config.transaction_handler.unwrap_or_else(|| {
            Box::new(DefaultSmtpTransactionHandler::new(
                config.spool,
                peer,
                config.spf_dns_resolver,
                config.spf_config,
                config.require_dkim,
                config.dmarc_enforce,
                Arc::clone(&audit_logger),
            ))
        });

        let session = Self {
            context: Context::default(),
            extensions: config.extensions,
            capabilities: config.capabilities,
            banner: Arc::from(config.banner.as_str()),
            init_context: Arc::new(config.init_context),
            max_message_size,
            audit_logger,
        };

        (session, transaction_handler, tls_context)
    }
}
