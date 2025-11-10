use core::bstr;
use std::{borrow::Cow, net::SocketAddr, path::PathBuf, sync::Arc};

use ahash::AHashMap;
use empath_common::{
    Signal, context, error::SessionError, incoming, internal, outgoing, status::Status, tracing,
};
use empath_ffi::modules::{self, validate};
use empath_tracing::traced;
use mailparse::MailParseError;
use serde::Deserialize;
use tokio::io::{AsyncRead, AsyncWrite};

use crate::{State, command::Command, connection::Connection, extensions::Extension, state};

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

#[allow(dead_code)]
pub struct SMTPError {
    pub status: Status,
    pub message: String,
}

pub type Response = (Option<Vec<String>>, Event);

impl From<MailParseError> for SMTPError {
    fn from(err: MailParseError) -> Self {
        Self {
            status: Status::Error,
            message: err.to_string(),
        }
    }
}

impl From<modules::Error> for SMTPError {
    fn from(value: modules::Error) -> Self {
        Self {
            status: Status::Error,
            message: format!("{value}"),
        }
    }
}

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
    context: Context,
    extensions: Vec<Extension>,
    banner: Arc<str>,
    tls_context: Option<TlsContext>,
    spool: Option<Arc<dyn empath_spool::BackingStore>>,
    connection: Connection<Stream>,
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
    max_message_size: usize,
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
        use crate::state::State;

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

                    if modules::dispatch(
                        modules::Event::Validate(validate::Event::StartTls),
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
                    // Get state-aware timeout
                    let timeout_secs = session.get_timeout_secs();
                    let timeout_duration = std::time::Duration::from_secs(timeout_secs);

                    tokio::select! {
                        _ = signal.recv() => {
                            session.context.sent = false;
                            session.context.state = State::Close(state::Close);
                            validate_context.response =
                                Some((Status::Unavailable, Cow::Borrowed("Server shutting down")));
                        }
                        result = tokio::time::timeout(timeout_duration, session.receive(validate_context)) => {
                            if let Ok(close) = result {
                                if close.unwrap_or(true) {
                                    return Ok(());
                                }
                            } else {
                                // Timeout occurred
                                tracing::warn!(
                                    peer = ?session.peer,
                                    state = ?session.context.state,
                                    timeout_secs = timeout_secs,
                                    "Client connection timed out"
                                );
                                return Err(SessionError::Timeout(timeout_secs));
                            }
                        }
                    }
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
        modules::dispatch(
            modules::Event::Event(modules::Ev::ConnectionClosed),
            &mut validate_context,
        );

        result
    }

    /// Handle validation and work for each state
    ///
    /// Flow:
    /// 1. Dispatch to core module first (sets default responses, validation)
    /// 2. Then dispatch to user modules (can override responses, reject)
    /// 3. If validation passed, do the work (spooling)
    #[traced(instrument(level = tracing::Level::TRACE, skip_all), timing)]
    async fn emit(&mut self, validate_context: &mut context::Context) {
        match self.context.state {
            State::Connect(_) => {
                modules::dispatch(
                    modules::Event::Event(modules::Ev::ConnectionOpened),
                    validate_context,
                );

                if !modules::dispatch(
                    modules::Event::Validate(validate::Event::Connect),
                    validate_context,
                ) {
                    self.context.state = State::Reject(state::Reject);
                }
            }
            State::Helo(_) | State::Ehlo(_) => {
                if !modules::dispatch(
                    modules::Event::Validate(validate::Event::Ehlo),
                    validate_context,
                ) {
                    self.context.state = State::Reject(state::Reject);
                }
            }
            State::MailFrom(_) => {
                if !modules::dispatch(
                    modules::Event::Validate(validate::Event::MailFrom),
                    validate_context,
                ) {
                    // Don't change state for validation failures like SIZE - just return error
                    return;
                }
            }
            State::RcptTo(_) => {
                if !modules::dispatch(
                    modules::Event::Validate(validate::Event::RcptTo),
                    validate_context,
                ) {
                    self.context.state = State::Reject(state::Reject);
                }
            }
            State::PostDot(_) => {
                // Dispatch validation
                let valid = modules::dispatch(
                    modules::Event::Validate(validate::Event::Data),
                    validate_context,
                );

                // If validation passed, do the work (spooling)
                if valid {
                    // Check if any module set a rejection response
                    // Positive responses are < 400 (2xx and 3xx codes)
                    let should_spool = validate_context
                        .response
                        .as_ref()
                        .is_none_or(|(status, _)| !status.is_temporary() && !status.is_permanent());

                    if should_spool {
                        self.spool_message(validate_context).await;
                    }
                }
            }
            _ => {}
        }
    }

    /// Spool message after validation passes
    #[traced(instrument(level = tracing::Level::TRACE, skip_all), timing)]
    async fn spool_message(&self, validate_context: &mut context::Context) {
        internal!("Spooling message");

        let tracking_id = if let Some(spool) = &self.spool
            && validate_context.data.is_some()
        {
            // Clone the context for spooling
            let context_to_spool = validate_context.clone();

            match spool.write(context_to_spool).await {
                Ok(id) => Some(id),
                Err(e) => {
                    internal!(level = ERROR, "Failed to spool message: {e}");
                    validate_context.response = Some((
                        Status::ActionUnavailable,
                        Cow::Borrowed("Please try again later"),
                    ));
                    return;
                }
            }
        } else {
            None
        };

        // Clear transaction state after successful acceptance
        validate_context.metadata.remove("declared_size");

        // Set success response with tracking ID
        validate_context.response = Some((
            Status::Ok,
            tracking_id.as_ref().map_or_else(
                || Cow::Borrowed("Ok: queued"),
                |id| Cow::Owned(format!("Ok: queued as {id}")),
            ),
        ));
    }

    /// Format and return the response to send to the client
    ///
    /// This is a pure formatter - all validation and work happens in `emit()`.
    /// Just formats the response based on state and what `emit()` set in the context.
    #[traced(instrument(level = tracing::Level::TRACE, skip_all, ret), timing(precision = "ns"))]
    async fn response(&mut self, validate_context: &mut context::Context) -> Response {
        if self.context.sent {
            return (None, Event::ConnectionKeepAlive);
        }

        // Emit events, do validation and work first
        self.emit(validate_context).await;

        // If emit() set a response in the context, use it
        // Only close connection for Reject state, not all permanent errors
        if let Some((status, ref message)) = validate_context.response {
            let event = if matches!(self.context.state, State::Reject(_)) && status.is_permanent() {
                Event::ConnectionClose
            } else {
                Event::ConnectionKeepAlive
            };

            return (Some(vec![format!("{status} {message}")]), event);
        }

        // Otherwise, provide default responses for states not handled by emit()
        match &self.context.state {
            State::Helo(_) => (
                Some(vec![format!(
                    "{} {} says hello to {}",
                    Status::Ok,
                    self.banner,
                    bstr::ByteStr::new(&self.context.message)
                )]),
                Event::ConnectionKeepAlive,
            ),
            State::StartTls(_) => {
                if self.tls_context.is_some() {
                    (
                        Some(vec![format!("{} Ready to begin TLS", Status::ServiceReady)]),
                        Event::ConnectionKeepAlive,
                    )
                } else {
                    (
                        Some(vec![format!("{} TLS not available", Status::Error)]),
                        Event::ConnectionClose,
                    )
                }
            }
            State::Data(_) => {
                self.context.state = State::Reading(state::Reading);

                // Pre-allocate message buffer based on SIZE parameter if declared
                if let Some(params) = validate_context.envelope.mail_params()
                    && let Some(Some(size_str)) = params.get("SIZE")
                    && let Ok(declared_size) = size_str.parse::<usize>()
                {
                    // Reserve capacity to avoid reallocations during message receipt
                    self.context.message.reserve(declared_size);
                }

                (
                    Some(vec![format!(
                        "{} End data with <CR><LF>.<CR><LF>",
                        Status::StartMailInput
                    )]),
                    Event::ConnectionKeepAlive,
                )
            }
            State::Quit(_) => (
                Some(vec![format!("{} Bye", Status::GoodBye)]),
                Event::ConnectionClose,
            ),
            State::Invalid(_) => (
                Some(vec![format!(
                    "{} {}",
                    Status::InvalidCommandSequence,
                    self.context.state
                )]),
                Event::ConnectionClose,
            ),
            State::Reject(_) => {
                // Reject should have response set by emit(), but provide fallback
                (
                    Some(vec![format!("{} Unavailable", Status::Unavailable)]),
                    Event::ConnectionClose,
                )
            }
            // States handled by emit() (Connect, Ehlo, MailFrom, RcptTo, PostDot) should have set a response
            // States like Reading, Close, and others that don't need responses
            _ => (None, Event::ConnectionKeepAlive),
        }
    }

    #[traced(instrument(level = tracing::Level::TRACE, skip_all, ret), timing)]
    async fn receive(
        &mut self,
        validate_context: &mut context::Context,
    ) -> Result<bool, SessionError> {
        let mut received_data = [0; 4096];

        match self.connection.receive(&mut received_data).await {
            // Consider any errors received here to be fatal
            Err(err) => {
                internal!("Error: {err}");
                Err(SessionError::Protocol(err.to_string()))
            }
            Ok(0) => {
                // Reading 0 bytes means the other side has closed the
                // connection or is done writing, then so are we.
                Ok(true)
            }
            Ok(bytes_read) => {
                let received = &received_data[..bytes_read];

                if matches!(self.context.state, State::Reading(_)) {
                    // Check if adding received data would exceed limit (BEFORE extending buffer)
                    // This prevents the buffer overflow vulnerability where an attacker could
                    // consume up to max_message_size + 4095 bytes before being rejected
                    // Use checked_add to prevent integer overflow on 32-bit systems
                    if self.max_message_size > 0 {
                        let total_size = self.context.message.len().saturating_add(received.len());

                        if total_size > self.max_message_size {
                            validate_context.response = Some((
                                Status::ExceededStorage,
                                Cow::Owned(format!(
                                    "Actual message size {} bytes exceeds maximum allowed size {} bytes",
                                    total_size, self.max_message_size
                                )),
                            ));
                            self.context.state = State::Close(state::Close);
                            self.context.sent = false;
                            return Ok(false);
                        }
                    }

                    self.context.message.extend(received);

                    if self.context.message.ends_with(b"\r\n.\r\n") {
                        // Move the message buffer to avoid double cloning
                        let message = std::mem::take(&mut self.context.message);

                        self.context = Context {
                            state: State::PostDot(state::PostDot),
                            message: message.clone(),
                            sent: false,
                        };

                        validate_context.data = Some(message.into());
                    }
                } else {
                    let command = Command::try_from(received).unwrap_or_else(|e| e);
                    let message = command.inner().as_bytes().to_vec();

                    incoming!("{command}");

                    self.context = Context {
                        state: self
                            .context
                            .state
                            .clone()
                            .transition(command, validate_context),
                        message,
                        sent: false,
                    };

                    tracing::debug!("Transitioned to {:#?}", self.context);
                }

                Ok(false)
            }
        }
    }
}

#[cfg(test)]
mod test {
    use std::{io::Cursor, sync::Arc};

    use empath_common::{context::Context, status::Status};
    use empath_ffi::modules::{self, Module};
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
        use std::sync::Arc;

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

        let module = Module::TestModule(Arc::default());
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

        // TODO: Need to fix the above so it spawns its own server that actually does the emitting
        //       Would possibly be better to make a mock SMTP client or something that we can test with?
        // if let Module::TestModule(mute) = MODULE_STORE.read().unwrap().first().unwrap() {
        //     assert!(
        //         mute.lock()
        //             .unwrap()
        //             .validators_called
        //             .contains(&validate::Event::MailFrom)
        //     );
        // } else {
        //     panic!("Expected TestModule to exist");
        // }
    }
}
