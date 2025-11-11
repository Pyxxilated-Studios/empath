use std::{borrow::Cow, net::SocketAddr, path::PathBuf, sync::Arc};

use ahash::AHashMap;
use empath_common::{
    Signal, context, error::SessionError, internal, outgoing, status::Status, tracing,
};
use empath_tracing::traced;
use serde::Deserialize;
use tokio::io::{AsyncRead, AsyncWrite};

use crate::{State, connection::Connection, extensions::Extension, state};

// Submodules containing implementation details
mod events;
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

#[derive(Clone, Debug, Deserialize)]
pub struct TlsContext {
    pub certificate: PathBuf,
    pub key: PathBuf,
}

#[derive(Debug)]
pub struct SessionConfig {
    pub extensions: Vec<Extension>,
    pub tls_context: Option<TlsContext>,
    pub spool: Option<Arc<dyn empath_spool::BackingStore>>,
    pub banner: String,
    pub init_context: AHashMap<Cow<'static, str>, String>,
    pub timeouts: crate::SmtpServerTimeouts,
}

impl SessionConfig {
    /// Create a new `SessionConfig` builder
    #[must_use]
    pub fn builder() -> SessionConfigBuilder {
        SessionConfigBuilder::default()
    }
}

/// Builder for `SessionConfig`
#[derive(Debug)]
pub struct SessionConfigBuilder {
    extensions: Vec<Extension>,
    tls_context: Option<TlsContext>,
    spool: Option<Arc<dyn empath_spool::BackingStore>>,
    banner: String,
    init_context: AHashMap<Cow<'static, str>, String>,
    timeouts: crate::SmtpServerTimeouts,
}

impl Default for SessionConfigBuilder {
    fn default() -> Self {
        Self {
            extensions: Vec::new(),
            tls_context: None,
            spool: None,
            banner: String::new(),
            init_context: AHashMap::new(),
            timeouts: crate::SmtpServerTimeouts::default(),
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
    pub fn with_banner(mut self, banner: String) -> Self {
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
        }
    }
}

pub struct Session<Stream: AsyncRead + AsyncWrite + Unpin + Send + Sync> {
    peer: SocketAddr,
    pub(super) context: Context,
    extensions: Vec<Extension>,
    pub(super) banner: Arc<str>,
    pub(super) tls_context: Option<TlsContext>,
    pub(super) spool: Option<Arc<dyn empath_spool::BackingStore>>,
    pub(super) connection: Connection<Stream>,
    init_context: Arc<AHashMap<Cow<'static, str>, String>>,
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
    pub(super) max_message_size: usize,
    /// Server-side timeout configuration
    timeouts: crate::SmtpServerTimeouts,
    /// Start time for tracking connection lifetime
    start_time: std::time::Instant,
}

impl<Stream: AsyncRead + AsyncWrite + Unpin + Send + Sync> Session<Stream> {
    #[traced(instrument(level = tracing::Level::TRACE, skip_all), timing)]
    pub(crate) fn create(stream: Stream, peer: SocketAddr, config: SessionConfig) -> Self {
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

        Self {
            peer,
            connection: Connection::Plain {
                stream,
                read_buf: Vec::new(),
                read_pos: 0,
                read_len: 0,
            },
            context: Context::default(),
            extensions: config.extensions,
            tls_context,
            spool: config.spool,
            banner: if config.banner.is_empty() {
                std::env::var("HOSTNAME")
                    .unwrap_or_else(|_| "localhost".to_string())
                    .into()
            } else {
                config.banner.into()
            },
            init_context: Arc::new(config.init_context),
            max_message_size,
            timeouts: config.timeouts,
            start_time: std::time::Instant::now(),
        }
    }

    /// Get the appropriate timeout for the current state
    ///
    /// Returns timeout in seconds based on RFC 5321 recommendations:
    /// - DATA block reading: 3 minutes (waiting for message content)
    /// - DATA initiation: 2 minutes (for DATA command itself)
    /// - `PostDot`: 10 minutes (for processing after final dot)
    /// - Regular commands: 5 minutes (EHLO, MAIL FROM, RCPT TO, etc.)
    const fn get_timeout_secs(&self) -> u64 {
        match &self.context.state {
            State::Reading(_) => self.timeouts.data_block_secs,
            State::Data(_) => self.timeouts.data_init_secs,
            State::PostDot(_) => self.timeouts.data_termination_secs,
            _ => self.timeouts.command_secs,
        }
    }

    #[traced(instrument(level = tracing::Level::TRACE, skip_all, fields(?peer = self.peer), ret), timing(precision = "us"))]
    #[allow(clippy::too_many_lines)]
    pub(crate) async fn run(
        mut self,
        signal: tokio::sync::broadcast::Receiver<Signal>,
    ) -> Result<(), SessionError> {
        internal!("Connected");

        async fn run_inner<Stream: AsyncRead + AsyncWrite + Unpin + Send + Sync>(
            mut session: Session<Stream>,
            validate_context: &mut context::Context,
            mut signal: tokio::sync::broadcast::Receiver<Signal>,
        ) -> Result<(), SessionError> {
            loop {
                // Check if connection has exceeded maximum lifetime
                let connection_duration = session.start_time.elapsed();
                let max_duration = std::time::Duration::from_secs(session.timeouts.connection_secs);
                if connection_duration >= max_duration {
                    tracing::warn!(
                        peer = ?session.peer,
                        duration_secs = ?connection_duration.as_secs(),
                        max_secs = session.timeouts.connection_secs,
                        "Connection exceeded maximum lifetime, closing"
                    );
                    return Err(SessionError::Timeout(session.timeouts.connection_secs));
                }

                // Then generate the response based on what emit() set
                let (response, ev) = session.response(validate_context).await;

                validate_context.response = None;
                session.context.sent = true;

                for response in response.unwrap_or_default() {
                    outgoing!("{response}");

                    session.connection.send(&response).await.map_err(|err| {
                        internal!(level = ERROR, "{err}");
                        SessionError::Protocol(format!("Failed to send response: {err}"))
                    })?;
                }

                if Event::ConnectionClose == ev {
                    return Ok(());
                } else if let Some(tls_context) = session.tls_context.as_ref()
                    && matches!(session.context.state, State::StartTls(_))
                {
                    // Handle TLS upgrade inline to avoid borrowing issues
                    let (conn, info) = session
                        .connection
                        .upgrade(tls_context)
                        .await
                        .map_err(|e| SessionError::Protocol(e.to_string()))?;

                    session.connection = conn;

                    validate_context
                        .metadata
                        .insert(Cow::Borrowed("tls"), "true".to_string());
                    validate_context
                        .metadata
                        .insert(Cow::Borrowed("protocol"), info.proto());
                    validate_context
                        .metadata
                        .insert(Cow::Borrowed("cipher"), info.cipher());

                    session.context = Context {
                        sent: true,
                        ..Default::default()
                    };

                    if empath_ffi::modules::dispatch(
                        empath_ffi::modules::Event::Validate(
                            empath_ffi::modules::validate::Event::StartTls,
                        ),
                        validate_context,
                    ) {
                        internal!(
                            level = DEBUG,
                            "Connection successfully upgraded with {info:#?}"
                        );
                    } else {
                        session.context.sent = false;
                        session.context.state = State::Reject(state::Reject);
                        validate_context.response =
                            Some((Status::Error, Cow::Borrowed("STARTTLS failed")));
                    }
                } else {
                    session
                        .handle_command_loop(validate_context, &mut signal)
                        .await?;
                }
            }
        }

        let mut validate_context = context::Context {
            banner: Arc::clone(&self.banner),
            max_message_size: self.max_message_size,
            capabilities: self
                .extensions
                .iter()
                .flat_map(std::convert::TryInto::try_into)
                .collect(),
            // Fast path: if init_context is empty, use default. Otherwise copy entries.
            // This avoids HashMap clone in the common case (empty init_context)
            metadata: if self.init_context.is_empty() {
                AHashMap::new()
            } else {
                self.init_context
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect()
            },
            ..Default::default()
        };

        self.emit(&mut validate_context).await;

        let result = run_inner(self, &mut validate_context, signal).await;

        internal!("Connection closed");
        empath_ffi::modules::dispatch(
            empath_ffi::modules::Event::Event(empath_ffi::modules::Ev::ConnectionClosed),
            &mut validate_context,
        );

        result
    }

    /// Handle the main command receive loop with timeout and shutdown handling
    ///
    /// # Errors
    /// Returns `SessionError` if a timeout occurs or connection error happens.
    async fn handle_command_loop(
        &mut self,
        validate_context: &mut context::Context,
        signal: &mut tokio::sync::broadcast::Receiver<Signal>,
    ) -> Result<(), SessionError> {
        // Get state-aware timeout
        let timeout_secs = self.get_timeout_secs();
        let timeout_duration = std::time::Duration::from_secs(timeout_secs);

        tokio::select! {
            _ = signal.recv() => {
                self.context.sent = false;
                self.context.state = State::Close(state::Close);
                validate_context.response =
                    Some((Status::Unavailable, Cow::Borrowed("Server shutting down")));
                Ok(())
            }
            result = tokio::time::timeout(timeout_duration, self.receive(validate_context)) => {
                if let Ok(close) = result {
                    if close.unwrap_or(true) {
                        return Ok(());
                    }
                } else {
                    // Timeout occurred
                    tracing::warn!(
                        peer = ?self.peer,
                        state = ?self.context.state,
                        timeout_secs = timeout_secs,
                        "Client connection timed out"
                    );
                    return Err(SessionError::Timeout(timeout_secs));
                }
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod test {
    use std::{
        io::Cursor,
        sync::{Arc, RwLock},
    };

    use empath_common::{context::Context, status::Status};
    use empath_ffi::modules::{self, MODULE_STORE, Module, validate::Event};
    use empath_spool::{BackingStore, TestBackingStore};

    use crate::{
        State,
        session::{Session, SessionConfig},
        state,
    };

    #[tokio::test]
    #[cfg_attr(all(target_os = "macos", miri), ignore)]
    async fn session() {
        // Initialize modules to add core module
        let _ = modules::init(vec![]);

        let banner = "testing";
        let mut context = Context {
            banner: banner.into(),
            max_message_size: 0,
            ..Default::default()
        };

        let cursor = Cursor::<Vec<u8>>::default();

        let mut session = Session::create(
            cursor,
            "[::]:25".parse().unwrap(),
            SessionConfig::builder()
                .with_banner(banner.to_string())
                .build(),
        );

        let response = session.response(&mut context).await;
        assert!(response.0.is_some());
        assert_eq!(
            response.0.unwrap().first().unwrap(),
            &format!("{} {banner}", Status::ServiceReady)
        );

        let response = session.receive(&mut context).await;
        assert!(response.is_ok_and(|v| v));
    }

    #[tokio::test]
    #[cfg_attr(all(target_os = "macos", miri), ignore)]
    async fn helo() {
        // Initialize modules to add core module
        let _ = modules::init(vec![]);

        let banner = "testing";
        let mut context = Context {
            banner: banner.into(),
            max_message_size: 0,
            ..Default::default()
        };

        let host = "Test";
        let mut cursor = Cursor::<Vec<u8>>::default();
        cursor
            .get_mut()
            .extend_from_slice(format!("HELO {host}").as_bytes());

        let mut session = Session::create(
            cursor,
            "[::]:25".parse().unwrap(),
            SessionConfig::builder()
                .with_banner(banner.to_string())
                .build(),
        );

        let _ = session.response(&mut context).await;
        context.response = None; // Clear response like run_inner does

        // Receive HELO command
        let response = session.receive(&mut context).await;
        assert!(response.is_ok());
        assert!(!response.unwrap());

        // HELO doesn't need emit(), it's not validated
        let response = session.response(&mut context).await;
        assert!(response.0.is_some());
        assert_eq!(
            response.0.unwrap().first().unwrap(),
            &format!("{} {banner} says hello to {host}", Status::Ok)
        );

        let response = session.receive(&mut context).await;
        assert!(response.is_ok_and(|v| v));
    }

    #[tokio::test]
    #[cfg_attr(all(target_os = "macos", miri), ignore)]
    async fn spool_integration() {
        // Initialize modules to add core module
        let _ = modules::init(vec![]);

        let banner = "testing";
        let mut context = Context {
            banner: banner.into(),
            max_message_size: 0,
            ..Default::default()
        };

        let mut cursor = Cursor::<Vec<u8>>::default();
        let test_data = b"Subject: Test\r\n\r\nHello World\r\n.\r\n";
        cursor.get_mut().extend_from_slice(test_data);

        // Create a mock spool controller
        let mock_spool = Arc::new(TestBackingStore::default());

        let mut session = Session::create(
            cursor,
            "[::]:25".parse().unwrap(),
            SessionConfig::builder()
                .with_spool(Some(mock_spool.clone()))
                .with_banner(banner.to_string())
                .build(),
        );

        // Simulate HELO state and receiving DATA
        session.context.state = State::RcptTo(state::RcptTo {
            sender: None,
            params: crate::MailParameters::new(),
        });
        let mut sender_addrs = mailparse::addrparse("test@example.com").unwrap();
        context
            .envelope
            .sender_mut()
            .replace(sender_addrs.remove(0).into());
        context.envelope.recipients_mut().replace(
            mailparse::addrparse("recipient@example.com")
                .unwrap()
                .into(),
        );

        let _ = session.response(&mut context).await;
        let response = session.receive(&mut context).await;
        assert!(response.is_ok());

        // Simulate PostDot state
        session.context.state = State::PostDot(state::PostDot);
        context.data = Some(test_data.to_vec().into());

        let response = session.response(&mut context).await;
        assert!(response.0.is_some());

        // Wait for the spool operation to complete with a timeout
        mock_spool
            .wait_for_count(1, std::time::Duration::from_secs(5))
            .await
            .expect("Spool operation should complete within timeout");

        // Verify message was spooled
        assert_eq!(mock_spool.message_count().await, 1);
        let ids = mock_spool.list().await.unwrap();
        let spooled_msg_id = ids.first().unwrap();
        let spooled_msg = mock_spool.read(spooled_msg_id).await.unwrap();
        assert_eq!(spooled_msg.data.as_deref(), Some(test_data.as_ref()));
    }

    #[tokio::test]
    #[cfg_attr(all(target_os = "macos", miri), ignore)]
    async fn modules() {
        let banner = "testing";
        let mut context = Context {
            banner: banner.into(),
            max_message_size: 0,
            ..Default::default()
        };

        let mut cursor = Cursor::<Vec<u8>>::default();
        cursor
            .get_mut()
            .extend_from_slice(b"MAIL FROM: test@gmail.com");

        let module = Module::TestModule(RwLock::default());
        let inited = modules::init(vec![module]);
        assert!(inited.is_ok());

        let mut session = Session::create(
            cursor,
            "[::]:25".parse().unwrap(),
            SessionConfig::builder()
                .with_banner(banner.to_string())
                .build(),
        );

        session.context.state = State::Helo(state::Helo {
            id: "test".to_string(),
        });

        let _ = session.response(&mut context).await;
        let response = session.receive(&mut context).await;
        assert!(response.is_ok());
        assert!(!response.unwrap());

        // After receive, state should be MailFrom - need to emit before response
        let response = session.response(&mut context).await;
        assert!(response.0.is_some());
        assert_eq!(
            response.0.unwrap().first().unwrap(),
            &format!("{} Ok", Status::Ok)
        );

        let response = session.receive(&mut context).await;
        assert!(response.is_ok_and(|v| v));

        for module in MODULE_STORE.get().cloned().unwrap().iter() {
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
}
