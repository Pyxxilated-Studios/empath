use core::bstr;
use std::{
    collections::HashMap,
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, atomic::AtomicU64},
};

use empath_common::{
    Signal, context, incoming, internal, outgoing, status::Status, tracing,
    traits::FiniteStateMachine,
};
use empath_ffi::modules::{self, validate};
use empath_tracing::traced;
use mailparse::MailParseError;
use serde::Deserialize;
use tokio::io::{AsyncRead, AsyncWrite};

use crate::{State, command::Command, connection::Connection, extensions::Extension};

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
    pub spool: Option<Arc<dyn empath_spool::Spool>>,
    pub banner: String,
    pub init_context: HashMap<String, String>,
}

impl SessionConfig {
    /// Create a new `SessionConfig` builder
    #[must_use]
    pub fn builder() -> SessionConfigBuilder {
        SessionConfigBuilder::default()
    }
}

/// Builder for `SessionConfig`
#[derive(Debug, Default)]
pub struct SessionConfigBuilder {
    extensions: Vec<Extension>,
    tls_context: Option<TlsContext>,
    spool: Option<Arc<dyn empath_spool::Spool>>,
    banner: String,
    init_context: HashMap<String, String>,
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
    pub fn with_spool(mut self, spool: Option<Arc<dyn empath_spool::Spool>>) -> Self {
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
    pub fn with_init_context(mut self, init_context: HashMap<String, String>) -> Self {
        self.init_context = init_context;
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
        }
    }
}

pub struct Session<Stream: AsyncRead + AsyncWrite + Unpin + Send + Sync> {
    queue: Arc<AtomicU64>,
    peer: SocketAddr,
    context: Context,
    extensions: Vec<Extension>,
    banner: String,
    tls_context: Option<TlsContext>,
    spool: Option<Arc<dyn empath_spool::Spool>>,
    connection: Connection<Stream>,
    init_context: HashMap<String, String>,
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
}

impl<Stream: AsyncRead + AsyncWrite + Unpin + Send + Sync> Session<Stream> {
    #[traced(instrument(level = tracing::Level::TRACE, skip_all), timing)]
    pub(crate) fn create(
        queue: Arc<AtomicU64>,
        stream: Stream,
        peer: SocketAddr,
        config: SessionConfig,
    ) -> Self {
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
            queue,
            peer,
            connection: Connection::Plain { stream },
            context: Context::default(),
            extensions: config.extensions,
            tls_context,
            spool: config.spool,
            banner: if config.banner.is_empty() {
                std::env::var("HOSTNAME").unwrap_or_else(|_| "localhost".to_string())
            } else {
                config.banner
            },
            init_context: config.init_context,
            max_message_size,
        }
    }

    #[traced(instrument(level = tracing::Level::TRACE, skip_all, fields(?peer = self.peer), ret), timing(precision = "us"))]
    pub(crate) async fn run(
        mut self,
        signal: tokio::sync::broadcast::Receiver<Signal>,
    ) -> anyhow::Result<()> {
        internal!("Connected");

        async fn run_inner<Stream: AsyncRead + AsyncWrite + Unpin + Send + Sync>(
            mut session: Session<Stream>,
            validate_context: &mut context::Context,
            mut signal: tokio::sync::broadcast::Receiver<Signal>,
        ) -> anyhow::Result<()> {
            loop {
                session.emit(validate_context);
                let (response, ev) = if let Some((status, ref resp)) = validate_context.response {
                    (
                        Some(vec![format!("{} {}", status, resp)]),
                        if status.is_permanent() {
                            Event::ConnectionClose
                        } else {
                            Event::ConnectionKeepAlive
                        },
                    )
                } else {
                    session.response(validate_context).await
                };

                validate_context.response = None;
                session.context.sent = true;

                for response in response.unwrap_or_default() {
                    outgoing!("{response}");

                    session.connection.send(&response).await.map_err(|err| {
                        internal!(level = ERROR, "{err}");
                        std::io::Error::new(std::io::ErrorKind::ConnectionAborted, err.to_string())
                    })?;
                }

                if Event::ConnectionClose == ev {
                    return Ok(());
                } else if let Some(tls_context) = session.tls_context.as_ref()
                    && matches!(session.context.state, State::StartTLS)
                {
                    let (conn, info) = session.connection.upgrade(tls_context).await?;

                    session.connection = conn;

                    validate_context
                        .context
                        .insert("tls".to_string(), "true".to_string());
                    validate_context
                        .context
                        .insert("protocol".to_string(), info.proto());
                    validate_context
                        .context
                        .insert("cipher".to_string(), info.cipher());

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
                        session.context.state = State::Reject;
                        validate_context.response =
                            Some((Status::Error, String::from("STARTTLS failed")));
                    }
                } else {
                    tokio::select! {
                        _ = signal.recv() => {
                            session.context.sent = false;
                            session.context.state = State::Close;
                            validate_context.response =
                                Some((Status::Unavailable, String::from("Server shutting down")));
                        }
                        close = session.receive(validate_context) => {
                            if close.unwrap_or(true) {
                                return Ok(());
                            }
                        }
                    }
                }
            }
        }

        let mut validate_context = context::Context::default();
        self.init_context.clone_into(&mut validate_context.context);

        self.emit(&mut validate_context);

        let result = run_inner(self, &mut validate_context, signal).await;

        internal!("Connection closed");
        modules::dispatch(
            modules::Event::Event(modules::Ev::ConnectionClosed),
            &mut validate_context,
        );

        result
    }

    fn emit(&mut self, validate_context: &mut context::Context) {
        match self.context.state {
            State::Connect => {
                modules::dispatch(
                    modules::Event::Event(modules::Ev::ConnectionOpened),
                    validate_context,
                );

                if !modules::dispatch(
                    modules::Event::Validate(validate::Event::Connect),
                    validate_context,
                ) {
                    self.context.state = State::Reject;
                }
            }
            State::MailFrom => {
                if !modules::dispatch(
                    modules::Event::Validate(validate::Event::MailFrom),
                    validate_context,
                ) {
                    self.context.state = State::Reject;
                }
            }
            State::PostDot => {
                modules::dispatch(
                    modules::Event::Validate(validate::Event::Data),
                    validate_context,
                );
            }
            _ => {}
        }
    }

    /// Handle EHLO/HELP response generation with extensions
    fn response_ehlo_help(&self) -> Response {
        let response = if matches!(self.context.state, State::Ehlo) {
            vec![format!(
                "{}{}{} says hello to {}",
                Status::Ok,
                if self.extensions.is_empty() { ' ' } else { '-' },
                self.banner,
                bstr::ByteStr::new(&self.context.message)
            )]
        } else {
            vec![]
        };

        (
            Some(self.extensions.iter().enumerate().fold(
                response,
                |mut response, (idx, extension)| {
                    response.push(format!(
                        "{}{}{}",
                        if matches!(self.context.state, State::Help) {
                            Status::HelpMessage
                        } else {
                            Status::Ok
                        },
                        if idx == self.extensions.len() - 1 {
                            ' '
                        } else {
                            '-'
                        },
                        extension
                    ));

                    response
                },
            )),
            Event::ConnectionKeepAlive,
        )
    }

    /// Handle `PostDot` state - queue message and optionally spool
    #[traced]
    async fn response_post_dot(&self, validate_context: &mut context::Context) -> Response {
        internal!("PostDot: {validate_context:?}");
        let queue = self
            .queue
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        // Spool the message if a spool controller is configured
        if let Some(spool) = &self.spool
            && let Some(data) = &validate_context.data
        {
            match empath_spool::Message::builder()
                .id(queue)
                .envelope(validate_context.envelope.clone())
                .data(data.clone())
                .helo_id(validate_context.id.clone())
                .extended(validate_context.extended)
                .context(validate_context.context.clone())
                .build()
            {
                Ok(message) => {
                    // Spool the message to persistent storage
                    // We must complete spooling before clearing transaction context
                    if let Err(e) = spool.spool_message(&message).await {
                        internal!(
                            level = ERROR,
                            "Failed to spool message {}: {}",
                            message.id,
                            e
                        );
                    }
                }
                Err(e) => {
                    internal!(
                        level = ERROR,
                        "Failed to build message for queue {}: {}",
                        queue,
                        e
                    );
                }
            }
        }

        // Clear transaction state after successful message acceptance
        // This prevents SIZE parameter from persisting across MAIL transactions
        validate_context.context.remove("declared_size");

        let default = (Status::Ok, format!("Ok: queued as {queue}"));
        let response = validate_context.response.as_ref().unwrap_or(&default);

        (
            Some(vec![format!("{} {}", response.0, response.1)]),
            Event::ConnectionKeepAlive,
        )
    }

    /// Generate the response(s) that should be sent back to the client
    /// depending on the servers state
    #[traced(instrument(level = tracing::Level::TRACE, skip_all, ret), timing(precision = "ns"))]
    async fn response(&mut self, validate_context: &mut context::Context) -> Response {
        if self.context.sent {
            return (None, Event::ConnectionKeepAlive);
        }

        match &self.context.state {
            State::Connect => (
                Some(vec![format!("{} {}", Status::ServiceReady, self.banner)]),
                Event::ConnectionKeepAlive,
            ),
            State::Helo => (
                Some(vec![format!(
                    "{} {} says hello to {}",
                    Status::Ok,
                    self.banner,
                    bstr::ByteStr::new(&self.context.message)
                )]),
                Event::ConnectionKeepAlive,
            ),
            State::Ehlo | State::Help => self.response_ehlo_help(),
            State::StartTLS => {
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
            State::MailFrom => {
                // Validate SIZE parameter if present and max_message_size is set
                // Per RFC 1870 Section 4: check declared size against server maximum
                if self.max_message_size > 0
                    && let Some(declared_size_str) = validate_context.context.get("declared_size")
                    && let Ok(declared_size) = declared_size_str.parse::<usize>()
                    && declared_size > self.max_message_size
                {
                    return (
                        Some(vec![format!(
                            "{} 5.2.3 Declared message size exceeds maximum (declared: {} bytes, maximum: {} bytes)",
                            Status::ExceededStorage,
                            declared_size,
                            self.max_message_size
                        )]),
                        Event::ConnectionKeepAlive,
                    );
                }

                (
                    Some(vec![format!("{} Ok", Status::Ok)]),
                    Event::ConnectionKeepAlive,
                )
            }
            State::RcptTo => (
                Some(vec![format!("{} Ok", Status::Ok)]),
                Event::ConnectionKeepAlive,
            ),
            State::Data => {
                self.context.state = State::Reading;
                (
                    Some(vec![format!(
                        "{} End data with <CR><LF>.<CR><LF>",
                        Status::StartMailInput
                    )]),
                    Event::ConnectionKeepAlive,
                )
            }
            State::PostDot => self.response_post_dot(validate_context).await,
            State::Quit => (
                Some(vec![format!("{} Bye", Status::GoodBye)]),
                Event::ConnectionClose,
            ),
            State::Reading | State::Close => (None, Event::ConnectionKeepAlive),
            State::Invalid | State::InvalidCommandSequence => (
                Some(vec![format!(
                    "{} {}",
                    Status::InvalidCommandSequence,
                    self.context.state
                )]),
                Event::ConnectionClose,
            ),
            State::Reject => {
                let default = (Status::Unavailable, "Unavailable".to_owned());
                let (status, response) = validate_context.response.as_ref().unwrap_or(&default);
                (
                    Some(vec![format!("{} {}", status, response)]),
                    Event::ConnectionClose,
                )
            }
        }
    }

    #[traced(instrument(level = tracing::Level::TRACE, skip_all, ret), timing)]
    async fn receive(&mut self, validate_context: &mut context::Context) -> anyhow::Result<bool> {
        let mut received_data = [0; 4096];

        match self.connection.receive(&mut received_data).await {
            // Consider any errors received here to be fatal
            Err(err) => {
                internal!("Error: {err}");
                Err(err)
            }
            Ok(0) => {
                // Reading 0 bytes means the other side has closed the
                // connection or is done writing, then so are we.
                Ok(true)
            }
            Ok(bytes_read) => {
                let received = &received_data[..bytes_read];

                if self.context.state == State::Reading {
                    // Check if adding received data would exceed limit (BEFORE extending buffer)
                    // This prevents the buffer overflow vulnerability where an attacker could
                    // consume up to max_message_size + 4095 bytes before being rejected
                    // Use checked_add to prevent integer overflow on 32-bit systems
                    if self.max_message_size > 0 {
                        let total_size = self.context.message.len().saturating_add(received.len());

                        if total_size > self.max_message_size {
                            validate_context.response = Some((
                                Status::ExceededStorage,
                                format!(
                                    "Actual message size {} bytes exceeds maximum allowed size {} bytes",
                                    total_size, self.max_message_size
                                ),
                            ));
                            self.context.state = State::Close;
                            self.context.sent = false;
                            return Ok(false);
                        }
                    }

                    self.context.message.extend(received);

                    if self.context.message.ends_with(b"\r\n.\r\n") {
                        // Move the message buffer to avoid double cloning
                        let message = std::mem::take(&mut self.context.message);

                        self.context = Context {
                            state: State::PostDot,
                            message: message.clone(),
                            sent: false,
                        };

                        validate_context.data = Some(message.into());
                    }
                } else {
                    let command = Command::try_from(received).unwrap_or_else(|e| e);
                    let message = command.inner().into_bytes();

                    incoming!("{command}");

                    self.context = Context {
                        state: self.context.state.transition(command, validate_context),
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

    use crate::{
        State,
        session::{Session, SessionConfig},
    };

    #[tokio::test]
    #[cfg_attr(all(target_os = "macos", miri), ignore)]
    async fn session() {
        let banner = "testing";
        let mut context = Context::default();
        let cursor = Cursor::<Vec<u8>>::default();

        let mut session = Session::create(
            Arc::default(),
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
        let banner = "testing";
        let mut context = Context::default();
        let host = "Test";
        let mut cursor = Cursor::<Vec<u8>>::default();
        cursor
            .get_mut()
            .extend_from_slice(format!("HELO {host}").as_bytes());

        let mut session = Session::create(
            Arc::default(),
            cursor,
            "[::]:25".parse().unwrap(),
            SessionConfig::builder()
                .with_banner(banner.to_string())
                .build(),
        );

        let _ = session.response(&mut context).await;
        let response = session.receive(&mut context).await;
        assert!(response.is_ok());
        assert!(!response.unwrap());

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

        let banner = "testing";
        let mut context = Context::default();
        let mut cursor = Cursor::<Vec<u8>>::default();
        let test_data = b"Subject: Test\r\n\r\nHello World\r\n.\r\n";
        cursor.get_mut().extend_from_slice(test_data);

        // Create a mock spool controller
        let mock_spool = Arc::new(empath_spool::MockController::new());

        let mut session = Session::create(
            Arc::default(),
            cursor,
            "[::]:25".parse().unwrap(),
            SessionConfig::builder()
                .with_spool(Some(mock_spool.clone() as Arc<dyn empath_spool::Spool>))
                .with_banner(banner.to_string())
                .build(),
        );

        // Simulate HELO state and receiving DATA
        session.context.state = State::RcptTo;
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

        // Process the data
        let _ = session.response(&mut context).await;
        let response = session.receive(&mut context).await;
        assert!(response.is_ok());

        // Simulate PostDot state
        session.context.state = State::PostDot;
        context.data = Some(test_data.to_vec().into());

        let response = session.response(&mut context).await;
        assert!(response.0.is_some());

        // Wait for the spool operation to complete with a timeout
        mock_spool
            .wait_for_count(1, std::time::Duration::from_secs(5))
            .await
            .expect("Spool operation should complete within timeout");

        // Verify message was spooled
        assert_eq!(mock_spool.message_count(), 1);
        let spooled_msg = mock_spool.get_message(0).unwrap();
        assert_eq!(spooled_msg.data.as_ref(), test_data);
    }

    #[tokio::test]
    #[cfg_attr(all(target_os = "macos", miri), ignore)]
    async fn modules() {
        let banner = "testing";
        let mut context = Context::default();
        let mut cursor = Cursor::<Vec<u8>>::default();
        cursor
            .get_mut()
            .extend_from_slice(b"MAIL FROM: test@gmail.com");

        let module = Module::TestModule(Arc::default());
        let inited = modules::init(vec![module]);
        assert!(inited.is_ok());

        let mut session = Session::create(
            Arc::default(),
            cursor,
            "[::]:25".parse().unwrap(),
            SessionConfig::builder()
                .with_banner(banner.to_string())
                .build(),
        );

        session.context.state = State::Helo;

        let _ = session.response(&mut context).await;
        let response = session.receive(&mut context).await;
        assert!(response.is_ok());
        assert!(!response.unwrap());

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
