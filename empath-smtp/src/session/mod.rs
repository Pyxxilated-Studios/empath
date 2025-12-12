use std::{
    borrow::Cow,
    io as std_io,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
};

use ahash::AHashMap;
use empath_common::{email::BannerHostname, tracing};
use empath_tracing::traced;
use parking_lot::RwLock;
use serde::{Deserialize, Deserializer, de::Error as _};
use tokio_rustls::rustls::{
    ServerConfig,
    pki_types::{
        CertificateDer, PrivateKeyDer,
        pem::{Error as PemError, PemObject},
    },
};

use crate::{
    State,
    extensions::Extension,
    transaction_handler::{DefaultSmtpTransactionHandler, SmtpTransactionHandler},
};

// Submodules containing implementation details
mod io;
mod response;

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
}

pub type Response = (Option<Vec<String>>, Event);

pub struct TlsContext {
    inner: Arc<TlsContextInner>,
}

#[derive(Debug)]
struct TlsContextInner {
    certificate: PathBuf,
    key: PathBuf,
    server_config: RwLock<Option<Arc<ServerConfig>>>,
}

#[derive(Debug, Deserialize)]
struct RawTlsContext {
    certificate: PathBuf,
    key: PathBuf,
}

impl Clone for TlsContext {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl std::fmt::Debug for TlsContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TlsContext")
            .field("certificate", &self.inner.certificate)
            .field("key", &self.inner.key)
            .finish()
    }
}

impl<'de> Deserialize<'de> for TlsContext {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = RawTlsContext::deserialize(deserializer)?;
        Self::new(raw.certificate, raw.key).map_err(D::Error::custom)
    }
}

impl TlsContext {
    /// Create a TLS context from certificate and key paths.
    ///
    /// # Errors
    ///
    /// Fails if the certificate or private key file cannot be read or parsed.
    pub fn new(certificate: PathBuf, key: PathBuf) -> crate::error::TlsResult<Self> {
        Ok(Self {
            inner: Arc::new(TlsContextInner {
                certificate,
                key,
                server_config: RwLock::new(None),
            }),
        })
    }

    fn build_server_config(
        cert_path: &Path,
        key_path: &Path,
    ) -> crate::error::TlsResult<ServerConfig> {
        let certificate = CertificateDer::from_pem_file(cert_path).map_err(|source| {
            crate::error::TlsError::CertificateLoad {
                path: cert_path.display().to_string(),
                source: std_io::Error::new(std_io::ErrorKind::InvalidData, source),
            }
        })?;

        let key = PrivateKeyDer::from_pem_file(key_path).map_err(|err: PemError| {
            crate::error::TlsError::KeyLoad {
                path: key_path.display().to_string(),
                reason: err.to_string(),
            }
        })?;

        ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![certificate], key)
            .map_err(|e| crate::error::TlsError::Rustls(e.to_string()))
    }

    pub(crate) fn certificate_path(&self) -> &Path {
        &self.inner.certificate
    }

    pub(crate) fn key_path(&self) -> &Path {
        &self.inner.key
    }

    pub(crate) fn server_config(&self) -> crate::error::TlsResult<Arc<ServerConfig>> {
        {
            let guard = self.inner.server_config.read();
            if let Some(config) = guard.clone() {
                return Ok(config);
            }
        }

        let updated = Self::build_server_config(self.certificate_path(), self.key_path())?;
        let config = Arc::new(updated);

        {
            let mut guard = self.inner.server_config.write();
            *guard = Some(Arc::clone(&config));
        }

        Ok(config)
    }

    pub(crate) fn reload(&self) -> crate::error::TlsResult<()> {
        let updated = Self::build_server_config(self.certificate_path(), self.key_path())?;
        let config = Arc::new(updated);

        {
            let mut guard = self.inner.server_config.write();
            *guard = Some(Arc::clone(&config));
        }

        Ok(())
    }
}

/// Registry used to keep track of every configured TLS context.
#[derive(Default)]
pub struct TlsContextRegistry {
    contexts: RwLock<Vec<TlsContext>>,
}

impl TlsContextRegistry {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            contexts: RwLock::new(Vec::new()),
        }
    }

    /// Register all TLS contexts referenced by the provided extensions.
    pub fn register_extensions(&self, extensions: &[Extension]) {
        let mut guard = self.contexts.write();
        for ext in extensions {
            if let Extension::Starttls(ctx) = ext {
                guard.push(ctx.clone());
            }
        }
    }

    /// Reload all registered contexts and return the number reloaded.
    ///
    /// # Errors
    ///
    /// Propagates any TLS reload error encountered while rebuilding a context.
    pub fn reload_all(&self) -> crate::error::TlsResult<usize> {
        let contexts = {
            let guard = self.contexts.read();
            guard.clone()
        };

        let mut reloaded = 0;
        for ctx in contexts {
            ctx.reload()?;
            reloaded += 1;
        }

        Ok(reloaded)
    }

    #[must_use]
    pub fn count(&self) -> usize {
        self.contexts.read().len()
    }
}

pub struct SessionConfig {
    pub extensions: Vec<Extension>,
    pub tls_context: Option<TlsContext>,
    pub spool: Option<Arc<dyn empath_spool::BackingStore>>,
    pub banner: BannerHostname,
    pub init_context: AHashMap<Cow<'static, str>, String>,
    pub timeouts: crate::SmtpServerTimeouts,
    pub spf_dns_resolver: Option<Arc<dyn crate::spf::SpfDnsResolver>>,
    pub spf_config: crate::spf::SpfConfig,
    pub transaction_handler: Option<Box<dyn SmtpTransactionHandler>>,
}

impl std::fmt::Debug for SessionConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionConfig")
            .field("extensions", &self.extensions)
            .field("tls_context", &self.tls_context)
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
            .finish()
    }
}

impl SessionConfig {
    /// Create a new `SessionConfig` builder
    #[must_use]
    pub fn builder() -> SessionConfigBuilder {
        SessionConfigBuilder::default()
    }
}

/// Builder for `SessionConfig`
pub struct SessionConfigBuilder {
    extensions: Vec<Extension>,
    tls_context: Option<TlsContext>,
    spool: Option<Arc<dyn empath_spool::BackingStore>>,
    banner: BannerHostname,
    init_context: AHashMap<Cow<'static, str>, String>,
    timeouts: crate::SmtpServerTimeouts,
    spf_dns_resolver: Option<Arc<dyn crate::spf::SpfDnsResolver>>,
    spf_config: crate::spf::SpfConfig,
    transaction_handler: Option<Box<dyn SmtpTransactionHandler>>,
}

impl std::fmt::Debug for SessionConfigBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionConfigBuilder")
            .field("extensions", &self.extensions)
            .field("tls_context", &self.tls_context)
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
            .finish()
    }
}

impl Default for SessionConfigBuilder {
    fn default() -> Self {
        Self {
            extensions: Vec::new(),
            tls_context: None,
            spool: None,
            banner: BannerHostname::new("localhost").unwrap_or_else(|_| {
                // Fallback should always succeed; if validation changes, use empty hostname
                BannerHostname::new("")
                    .unwrap_or_else(|_| BannerHostname::new_unchecked("localhost"))
            }),
            init_context: AHashMap::new(),
            timeouts: crate::SmtpServerTimeouts::default(),
            spf_dns_resolver: None,
            spf_config: crate::spf::SpfConfig::default(),
            transaction_handler: None,
        }
    }
}

impl SessionConfigBuilder {
    /// Set the SMTP extensions supported by this session
    #[must_use]
    pub fn with_extensions(mut self, extensions: Vec<Extension>) -> Self {
        self.extensions = extensions;
        self
    }

    /// Set the TLS context for STARTTLS support
    #[must_use]
    pub fn with_tls_context(mut self, tls_context: Option<TlsContext>) -> Self {
        self.tls_context = tls_context;
        self
    }

    /// Set the spool controller for message persistence
    #[must_use]
    pub fn with_spool(mut self, spool: Option<Arc<dyn empath_spool::BackingStore>>) -> Self {
        self.spool = spool;
        self
    }

    /// Set the server banner hostname
    #[must_use]
    pub fn with_banner(mut self, banner: BannerHostname) -> Self {
        self.banner = banner;
        self
    }

    /// Set the initial context key-value pairs
    #[must_use]
    pub fn with_init_context(mut self, init_context: AHashMap<Cow<'static, str>, String>) -> Self {
        self.init_context = init_context;
        self
    }

    /// Set the timeout configuration for this session
    #[must_use]
    pub const fn with_timeouts(mut self, timeouts: crate::SmtpServerTimeouts) -> Self {
        self.timeouts = timeouts;
        self
    }

    /// Set the SPF DNS resolver for this session
    #[must_use]
    pub fn with_spf_dns_resolver(
        mut self,
        resolver: Option<Arc<dyn crate::spf::SpfDnsResolver>>,
    ) -> Self {
        self.spf_dns_resolver = resolver;
        self
    }

    /// Set the SPF configuration for this session
    #[must_use]
    pub const fn with_spf_config(mut self, config: crate::spf::SpfConfig) -> Self {
        self.spf_config = config;
        self
    }

    /// Set the transaction handler for this session
    ///
    /// If not provided, `DefaultSmtpTransactionHandler` will be used.
    #[must_use]
    pub fn with_transaction_handler(
        mut self,
        handler: Option<Box<dyn SmtpTransactionHandler>>,
    ) -> Self {
        self.transaction_handler = handler;
        self
    }

    /// Build the final `SessionConfig`
    #[must_use]
    pub fn build(self) -> SessionConfig {
        SessionConfig {
            extensions: self.extensions,
            tls_context: self.tls_context,
            spool: self.spool,
            banner: self.banner,
            init_context: self.init_context,
            timeouts: self.timeouts,
            spf_dns_resolver: self.spf_dns_resolver,
            spf_config: self.spf_config,
            transaction_handler: self.transaction_handler,
        }
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
            Extension::Starttls(context) => Some(context.clone()),
            _ => None,
        });

        // Use provided transaction handler or create default
        let transaction_handler = config.transaction_handler.unwrap_or_else(|| {
            Box::new(DefaultSmtpTransactionHandler::new(
                config.spool,
                peer,
                config.spf_dns_resolver,
                config.spf_config,
            ))
        });

        let session = Self {
            context: Context::default(),
            extensions: config.extensions,
            banner: Arc::from(config.banner.as_str()),
            init_context: Arc::new(config.init_context),
            max_message_size,
        };

        (session, transaction_handler, tls_context)
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::unreachable
)]
mod test {
    use std::sync::{Arc, RwLock};

    use empath_common::{
        address::{Address, AddressList},
        context::Context,
        status::Status,
    };
    use empath_ffi::modules::{self, MODULE_STORE, Module, validate::Event};
    use empath_spool::{BackingStore, TestBackingStore};

    use crate::{
        State,
        session::{Session, SessionConfig},
        state,
        transaction_handler::SmtpTransactionHandler,
    };

    #[tokio::test]
    #[cfg_attr(miri, ignore)]
    async fn session() {
        // Initialize modules to add core module
        let _ = modules::init(vec![]);

        let banner = empath_common::email::BannerHostname::new("testing").unwrap();
        let mut context = Context {
            banner: banner.as_str().into(),
            max_message_size: 0,
            ..Default::default()
        };

        let (mut session, _handler, _tls) = Session::create(
            "[::]:25".parse().unwrap(),
            SessionConfig::builder().with_banner(banner.clone()).build(),
        );

        let response = session.response(&mut context).await;
        assert!(response.0.is_some());
        assert_eq!(
            response.0.unwrap().first().unwrap(),
            &format!("{} {}", Status::ServiceReady, banner)
        );
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)]
    async fn helo() {
        // Initialize modules to add core module
        let _ = modules::init(vec![]);

        let banner = empath_common::email::BannerHostname::new("testing").unwrap();
        let mut context = Context {
            banner: banner.as_str().into(),
            max_message_size: 0,
            ..Default::default()
        };

        let host = "Test";

        let (mut session, _handler, _tls) = Session::create(
            "[::]:25".parse().unwrap(),
            SessionConfig::builder().with_banner(banner.clone()).build(),
        );

        let _ = session.response(&mut context).await;
        context.response = None; // Clear response like run_inner does

        // Handle HELO command
        session.handle_command_reception(format!("HELO {host}").as_bytes(), &mut context);

        // HELO doesn't need emit(), it's not validated
        let response = session.response(&mut context).await;
        assert!(response.0.is_some());
        assert_eq!(
            response.0.unwrap().first().unwrap(),
            &format!("{} {banner} says hello to {host}", Status::Ok)
        );
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)]
    async fn spool_integration() {
        // Initialize modules to add core module
        let _ = modules::init(vec![]);

        let banner = empath_common::email::BannerHostname::new("testing").unwrap();
        let mut context = Context {
            banner: banner.as_str().into(),
            max_message_size: 0,
            ..Default::default()
        };

        let test_data = b"Subject: Test\r\n\r\nHello World\r\n.\r\n";

        // Create a mock spool controller
        let mock_spool = Arc::new(TestBackingStore::default());

        let (mut session, mut transaction_handler, _tls) = Session::create(
            "[::]:25".parse().unwrap(),
            SessionConfig::builder()
                .with_spool(Some(mock_spool.clone()))
                .with_banner(banner.clone())
                .build(),
        );

        // Simulate HELO state and receiving DATA
        session.context.state = State::RcptTo(state::RcptTo {
            sender: None,
            params: crate::MailParameters::new(),
        });
        let sender_mailbox =
            empath_common::address_parser::parse_forward_path("<test@example.com>").unwrap();
        Arc::make_mut(&mut context.envelope)
            .sender_mut()
            .replace(Address::from(sender_mailbox));
        let recipient_mailbox =
            empath_common::address_parser::parse_forward_path("<recipient@example.com>").unwrap();
        Arc::make_mut(&mut context.envelope)
            .recipients_mut()
            .replace(AddressList::from(vec![Address::from(recipient_mailbox)]));

        let _ = session.response(&mut context).await;

        // Handle DATA command to receive message
        session.handle_data_reception(test_data, &mut context);

        // Simulate PostDot state (handle_data_reception transitions here when it sees \r\n.\r\n)
        assert!(matches!(session.context.state, State::PostDot(_)));

        // Handle message spooling
        transaction_handler.handle_message(&mut context).await;

        let response = session.response(&mut context).await;
        assert!(response.0.is_some());

        // Wait for the spool operation to complete with a timeout
        mock_spool
            .wait_for_count(1, std::time::Duration::from_secs(5))
            .await
            .expect("Spool operation should complete within timeout");

        // Verify message was spooled
        assert_eq!(mock_spool.message_count(), 1);
        let ids = mock_spool.list().await.unwrap();
        let spooled_msg_id = ids.first().unwrap();
        let spooled_msg = mock_spool.read(spooled_msg_id).await.unwrap();
        assert_eq!(spooled_msg.data.as_deref(), Some(test_data.as_ref()));
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)]
    async fn modules() {
        let banner = empath_common::email::BannerHostname::new("testing").unwrap();
        let mut context = Context {
            banner: banner.as_str().into(),
            max_message_size: 0,
            ..Default::default()
        };

        let module = Module::TestModule(RwLock::default());
        let inited = modules::init(vec![module]);
        assert!(inited.is_ok());

        let (mut session, mut transaction_handler, _tls) = Session::create(
            "[::]:25".parse().unwrap(),
            SessionConfig::builder().with_banner(banner.clone()).build(),
        );

        session.context.state = State::Helo(state::Helo {
            id: "test".to_string(),
        });

        let _ = session.response(&mut context).await;

        // Handle MAIL FROM command
        session.handle_command_reception(b"MAIL FROM: test@gmail.com", &mut context);

        // After handle_command_reception, state should be MailFrom
        // Validate the command (this is where modules are dispatched)
        transaction_handler.validate_mail_from(&mut context).await;

        let response = session.response(&mut context).await;
        assert!(response.0.is_some());
        assert_eq!(
            response.0.unwrap().first().unwrap(),
            &format!("{} Ok", Status::Ok)
        );

        let modules = MODULE_STORE.get().cloned().unwrap();
        for module in modules.iter() {
            if let Module::TestModule(mute) = module {
                assert!(
                    mute.read()
                        .unwrap()
                        .validators_called
                        .contains(&Event::MailFrom)
                );

                return;
            }
        }

        panic!("Expected TestModule to exist");
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)]
    async fn test_injectable_transaction_handler() {
        use std::{
            borrow::Cow,
            sync::{
                Arc,
                atomic::{AtomicUsize, Ordering},
            },
        };

        use async_trait::async_trait;

        // Mock transaction handler that rejects all connections
        struct RejectAllHandler {
            call_count: Arc<AtomicUsize>,
        }

        #[async_trait]
        impl SmtpTransactionHandler for RejectAllHandler {
            async fn validate_connect(&mut self, ctx: &mut Context) -> bool {
                self.call_count.fetch_add(1, Ordering::SeqCst);
                ctx.response = Some((Status::Unavailable, Cow::Borrowed("Connection rejected")));
                false
            }

            async fn validate_ehlo(&mut self, _ctx: &mut Context) -> bool {
                false
            }

            async fn validate_mail_from(&mut self, _ctx: &mut Context) -> bool {
                false
            }

            async fn validate_rcpt_to(&mut self, _ctx: &mut Context) -> bool {
                false
            }

            async fn handle_message(&mut self, _ctx: &mut Context) -> bool {
                false
            }
        }

        let banner = empath_common::email::BannerHostname::new("testing").unwrap();
        let mut context = Context {
            banner: banner.as_str().into(),
            max_message_size: 0,
            ..Default::default()
        };

        let call_count = Arc::new(AtomicUsize::new(0));
        let handler = RejectAllHandler {
            call_count: Arc::clone(&call_count),
        };

        let (mut session, mut transaction_handler, _tls) = Session::create(
            "[::]:25".parse().unwrap(),
            SessionConfig::builder()
                .with_banner(banner.clone())
                .with_transaction_handler(Some(Box::new(handler)))
                .build(),
        );

        // Test that the mock handler is called during validate_connect
        let valid = transaction_handler.validate_connect(&mut context).await;
        assert_eq!(call_count.load(Ordering::SeqCst), 1);
        assert!(!valid); // Handler should reject

        // Test that validation fails
        assert!(context.response.is_some());
        let (status, message) = context.response.as_ref().unwrap();
        assert_eq!(*status, Status::Unavailable);
        assert_eq!(message, "Connection rejected");

        // Manually transition to Reject state (orchestrator would do this)
        session.context.state = State::Reject(state::Reject);
        assert!(matches!(session.context.state, State::Reject(_)));
    }
}
