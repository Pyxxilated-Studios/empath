pub mod arc;
pub mod auth;
pub mod client;
pub mod command;
pub mod connection;
pub mod dkim;
pub mod dmarc;
pub mod error;
pub mod extensions;
pub mod fsm;
pub mod orchestrator;
pub mod session;
pub mod session_state;
pub mod spf;
pub mod state;
pub mod transaction;

// Re-export commonly used types
use std::borrow::Cow;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, RwLock};

pub use command::MailParameters;
use empath_common::audit::AuditLogger;
use empath_common::context::Capability;
use empath_common::error::ProtocolError;
use empath_common::traits::protocol::Protocol;
use empath_spool::BackingStore;
use empath_tracing::traced;
use serde::Deserialize;
pub use session::{Session, TlsContextInfo, TlsContextRegistry, TlsProfileInfo};
// Re-export the type-safe state machine from the state module
pub use state::State;
use tokio::net::TcpStream;

use crate::extensions::Extension;
use crate::orchestrator::SmtpSessionOrchestrator;
use crate::session::{SessionConfig, TlsProfileSet};

/// SMTP server-side timeout configuration
///
/// These timeouts prevent resource exhaustion from slow or malicious clients
/// and follow RFC 5321 Section 4.5.3.2 recommendations.
#[derive(Clone, Debug, Deserialize)]
pub struct SmtpServerTimeouts {
    /// Timeout for regular SMTP commands (EHLO, MAIL FROM, RCPT TO, etc.)
    ///
    /// RFC 5321 recommends: 5 minutes
    /// Default: 300 seconds (5 minutes)
    #[serde(default = "default_command_timeout")]
    pub command_secs: u64,

    /// Timeout for DATA command response
    ///
    /// RFC 5321 recommends: 2 minutes
    /// Default: 120 seconds (2 minutes)
    #[serde(default = "default_data_init_timeout")]
    pub data_init_secs: u64,

    /// Timeout between data chunks while receiving message body
    ///
    /// RFC 5321 recommends: 3 minutes
    /// Default: 180 seconds (3 minutes)
    #[serde(default = "default_data_block_timeout")]
    pub data_block_secs: u64,

    /// Timeout for processing after final dot terminator
    ///
    /// RFC 5321 recommends: 10 minutes
    /// Default: 600 seconds (10 minutes)
    #[serde(default = "default_data_termination_timeout")]
    pub data_termination_secs: u64,

    /// Maximum total session duration
    ///
    /// Prevents sessions from living indefinitely.
    /// Default: 1800 seconds (30 minutes)
    #[serde(default = "default_connection_timeout")]
    pub connection_secs: u64,
}

impl Default for SmtpServerTimeouts {
    fn default() -> Self {
        Self {
            command_secs: default_command_timeout(),
            data_init_secs: default_data_init_timeout(),
            data_block_secs: default_data_block_timeout(),
            data_termination_secs: default_data_termination_timeout(),
            connection_secs: default_connection_timeout(),
        }
    }
}

const fn default_command_timeout() -> u64 {
    300 // 5 minutes per RFC 5321
}

const fn default_data_init_timeout() -> u64 {
    120 // 2 minutes per RFC 5321
}

const fn default_data_block_timeout() -> u64 {
    180 // 3 minutes per RFC 5321
}

const fn default_data_termination_timeout() -> u64 {
    600 // 10 minutes per RFC 5321
}

const fn default_connection_timeout() -> u64 {
    1800 // 30 minutes
}

#[derive(Default, Deserialize)]
pub struct Smtp;

#[derive(Clone, Default, Deserialize)]
pub struct SmtpArgs {
    #[serde(default)]
    extensions: Vec<Extension>,
    #[serde(default)]
    implicit_tls: Option<TlsProfileSet>,
    #[serde(default)]
    capabilities: Vec<Capability>,
    #[serde(default)]
    require_dkim: bool,
    #[serde(default)]
    dmarc_enforce: bool,
    #[serde(skip)]
    capabilities_state: Option<Arc<RwLock<Vec<Capability>>>>,
    #[serde(skip)]
    spool: Option<Arc<dyn BackingStore>>,
    #[serde(default)]
    pub timeouts: SmtpServerTimeouts,
    #[serde(skip)]
    spf_dns_resolver: Option<Arc<dyn spf::SpfDnsResolver>>,
    #[serde(default)]
    pub spf_config: spf::SpfConfig,
    #[serde(skip)]
    audit_logger: Option<Arc<dyn AuditLogger>>,
}

impl std::fmt::Debug for SmtpArgs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SmtpArgs")
            .field("extensions", &self.extensions)
            .field("implicit_tls", &self.implicit_tls)
            .field("capabilities", &self.capabilities)
            .field("require_dkim", &self.require_dkim)
            .field("dmarc_enforce", &self.dmarc_enforce)
            .field(
                "capabilities_state",
                &self.capabilities_state.as_ref().map(|_| "<shared>"),
            )
            .field("spool", &self.spool.as_ref().map(|_| "<spool>"))
            .field("timeouts", &self.timeouts)
            .field(
                "spf_dns_resolver",
                &self.spf_dns_resolver.as_ref().map(|_| "<resolver>"),
            )
            .field("spf_config", &self.spf_config)
            .field(
                "audit_logger",
                &self.audit_logger.as_ref().map(|_| "<audit_logger>"),
            )
            .finish()
    }
}

impl SmtpArgs {
    /// Create a new `SmtpArgs` builder
    #[must_use]
    pub fn builder() -> Self {
        Self::default()
    }

    /// Set the SMTP extensions supported by this server
    #[must_use]
    pub fn with_extensions(mut self, extensions: Vec<Extension>) -> Self {
        self.extensions = extensions;
        self
    }

    /// Set the SMTP capabilities supported by this server
    #[must_use]
    pub fn with_capabilities(mut self, capabilities: Vec<Capability>) -> Self {
        self.capabilities = capabilities;
        self
    }

    /// Require DKIM verification to pass before accepting message data
    #[must_use]
    pub const fn with_require_dkim(mut self, require_dkim: bool) -> Self {
        self.require_dkim = require_dkim;
        self
    }

    /// Enforce DMARC policy results when evaluation fails.
    #[must_use]
    pub const fn with_dmarc_enforce(mut self, dmarc_enforce: bool) -> Self {
        self.dmarc_enforce = dmarc_enforce;
        self
    }

    /// Set the audit logger for SMTP sessions.
    #[must_use]
    pub fn with_audit_logger(mut self, audit_logger: Arc<dyn AuditLogger>) -> Self {
        self.audit_logger = Some(audit_logger);
        self
    }

    /// Get the currently configured SMTP capabilities
    #[must_use]
    pub fn capabilities(&self) -> &[Capability] {
        &self.capabilities
    }

    /// Set shared SMTP capabilities state for runtime updates
    #[must_use]
    pub fn with_capabilities_state(mut self, state: Arc<RwLock<Vec<Capability>>>) -> Self {
        self.capabilities_state = Some(state);
        self
    }

    /// Set the spool controller for this SMTP server
    #[must_use]
    pub fn with_spool(mut self, spool: Arc<dyn BackingStore>) -> Self {
        self.spool = Some(spool);
        self
    }

    /// Set the timeout configuration for this SMTP server
    #[must_use]
    pub const fn with_timeouts(mut self, timeouts: SmtpServerTimeouts) -> Self {
        self.timeouts = timeouts;
        self
    }

    /// Set the SPF DNS resolver for this SMTP server
    #[must_use]
    pub fn with_spf_dns_resolver(mut self, resolver: Arc<dyn spf::SpfDnsResolver>) -> Self {
        self.spf_dns_resolver = Some(resolver);
        self
    }

    /// Set the SPF configuration for this SMTP server
    #[must_use]
    pub const fn with_spf_config(mut self, config: spf::SpfConfig) -> Self {
        self.spf_config = config;
        self
    }

    /// Register TLS contexts with the shared registry so certs can be reloaded at runtime.
    #[must_use]
    pub fn with_tls_context_registry(self, registry: &Arc<TlsContextRegistry>) -> Self {
        registry.register_extensions(&self.extensions);
        if let Some(profile_set) = &self.implicit_tls {
            registry.register_profile_set(profile_set.clone());
        }
        self
    }
}

impl Protocol for Smtp {
    type Args = SmtpArgs;
    type Context = HashMap<String, String>;
    type Session = SmtpSessionOrchestrator<TcpStream>;

    fn ty() -> &'static str {
        "SMTP"
    }

    #[traced(instrument(level = tracing::Level::TRACE, skip(self, stream, init_context, args)), timing(precision = "ms"))]
    fn handle(
        &self,
        stream: TcpStream,
        peer: SocketAddr,
        init_context: HashMap<String, String>,
        args: Self::Args,
    ) -> Self::Session {
        let implicit_tls_context = args
            .implicit_tls
            .as_ref()
            .and_then(|profile_set| profile_set.active_context().ok());
        let extensions = if implicit_tls_context.is_some() {
            args.extensions
                .iter()
                .filter(|ext| !matches!(ext, Extension::Starttls(_)))
                .cloned()
                .collect()
        } else {
            args.extensions.clone()
        };
        let timeouts = args.timeouts.clone();
        let capabilities = args
            .capabilities_state
            .as_ref()
            .and_then(|state| state.read().ok().map(|guard| guard.clone()))
            .unwrap_or_else(|| args.capabilities.clone());
        let (session, transaction_handler, tls_context) = crate::session::Session::create(
            peer,
            SessionConfig::builder()
                .with_extensions(extensions)
                .with_capabilities(capabilities)
                .with_require_dkim(args.require_dkim)
                .with_dmarc_enforce(args.dmarc_enforce)
                .with_spool(args.spool)
                .with_timeouts(args.timeouts)
                .with_spf_dns_resolver(args.spf_dns_resolver)
                .with_spf_config(args.spf_config)
                .with_audit_logger(args.audit_logger)
                .with_init_context(
                    init_context
                        .into_iter()
                        .map(|(k, v)| (Cow::Owned(k), v))
                        .collect(),
                )
                .build(),
        );

        SmtpSessionOrchestrator::new(
            stream,
            peer,
            session,
            transaction_handler,
            timeouts,
            tls_context,
            implicit_tls_context,
        )
    }

    #[traced(instrument(skip(self, args)), timing(precision = "ns"))]
    fn validate(&mut self, args: &mut Self::Args) -> Result<(), ProtocolError> {
        let mut starttls_profile = None;
        for ext in &args.extensions {
            if let Extension::Starttls(profile_set) = ext {
                starttls_profile = Some(profile_set);
                break;
            }
        }

        if let Some(profile_set) = starttls_profile {
            validate_tls_profiles(profile_set)?;
        }

        if let Some(profile_set) = &args.implicit_tls {
            if starttls_profile.is_some() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "Implicit TLS listeners cannot also advertise STARTTLS",
                )
                .into());
            }
            validate_tls_profiles(profile_set)?;
        }

        Ok(())
    }
}

fn validate_tls_profiles(profile_set: &TlsProfileSet) -> Result<(), ProtocolError> {
    for (profile, tls) in profile_set.profiles() {
        let cert_path = tls.certificate_path();
        if !cert_path.try_exists()? {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!(
                    "Unable to find TLS certificate {} for profile '{profile}'",
                    cert_path.display()
                ),
            )
            .into());
        }

        let key_path = tls.key_path();
        if !key_path.try_exists()? {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!(
                    "Unable to find TLS key {} for profile '{profile}'",
                    key_path.display()
                ),
            )
            .into());
        }
    }

    let _ = profile_set.active_context().map_err(|err| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("Invalid TLS profile selection: {err}"),
        )
    })?;
    Ok(())
}
