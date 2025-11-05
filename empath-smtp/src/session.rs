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

#[derive(Debug, Clone, Deserialize)]
pub struct Context {
    pub state: State,
    pub message: Vec<u8>,
    pub sent: bool,
}

impl Default for Context {
    fn default() -> Self {
        Self {
            state: State::Connect,
            message: Vec::default(),
            sent: false,
        }
    }
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
    pub(crate) certificate: PathBuf,
    pub(crate) key: PathBuf,
}

#[derive(Debug)]
pub struct SessionConfig {
    pub extensions: Vec<Extension>,
    pub tls_context: Option<TlsContext>,
    pub spool: Option<Arc<dyn empath_spool::Spool>>,
    pub banner: String,
    pub init_context: HashMap<String, String>,
}

pub struct Session<Stream: AsyncRead + AsyncWrite + Unpin + Send + Sync> {
    queue: Arc<AtomicU64>,
    peer: SocketAddr,
    context: Context,
    extensions: Arc<[Extension]>,
    banner: String,
    tls_context: Option<TlsContext>,
    spool: Option<Arc<dyn empath_spool::Spool>>,
    connection: Connection<Stream>,
    init_context: HashMap<String, String>,
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
        let mut extensions = config.extensions;
        if config.tls_context.is_some() {
            extensions.push(Extension::Starttls);
        }
        extensions.push(Extension::Help);

        tracing::debug!("Extensions: {extensions:?}");

        Self {
            queue,
            peer,
            connection: Connection::Plain { stream },
            context: Context::default(),
            extensions: extensions.into(),
            tls_context: config.tls_context,
            spool: config.spool,
            banner: if config.banner.is_empty() {
                "localhost".to_string()
            } else {
                config.banner
            },
            init_context: config.init_context,
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
                let (mut response, mut ev) = session.response(validate_context).await;
                if let Some((status, ref resp)) = validate_context.response {
                    response = Some(vec![format!("{} {}", status, resp)]);
                    ev = if status.is_permanent() {
                        Event::ConnectionClose
                    } else {
                        Event::ConnectionKeepAlive
                    }
                }

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
                    && session.context.state == State::StartTLS
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

        modules::dispatch(
            modules::Event::Event(modules::Ev::ConnectionOpened),
            &mut validate_context,
        );

        if !modules::dispatch(
            modules::Event::Validate(validate::Event::Connect),
            &mut validate_context,
        ) {
            self.context.state = State::Reject;
        }

        let result = run_inner(self, &mut validate_context, signal).await;

        internal!("Connection closed");
        modules::dispatch(
            modules::Event::Event(modules::Ev::ConnectionClosed),
            &mut validate_context,
        );

        result
    }

    /// Handle EHLO/HELP response generation with extensions
    fn response_ehlo_help(&self) -> Response {
        let response = if self.context.state == State::Ehlo {
            vec![format!(
                "{}{}Hello {}",
                Status::Ok,
                if self.extensions.is_empty() { ' ' } else { '-' },
                std::str::from_utf8(&self.context.message).unwrap()
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
                        if self.context.state == State::Help {
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
    async fn response_post_dot(&self, validate_context: &context::Context) -> Response {
        internal!("PostDot: {validate_context:?}");
        let queue = self
            .queue
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        // Spool the message if a spool controller is configured
        if let Some(spool) = &self.spool
            && let Some(data) = &validate_context.data
        {
            let message = empath_spool::Message::new(
                queue,
                validate_context.envelope.clone(),
                data.clone(),
                validate_context.id.clone(),
                validate_context.extended,
                validate_context.context.clone(),
            );

            // Spawn a task to spool the message asynchronously
            let spool_clone = spool.clone();
            let _ = tokio::spawn(async move {
                if let Err(e) = spool_clone.spool_message(&message).await {
                    internal!(
                        level = ERROR,
                        "Failed to spool message {}: {}",
                        message.id,
                        e
                    );
                }
            })
            .await;
        }

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

        if State::PostDot == self.context.state {
            modules::dispatch(
                modules::Event::Validate(validate::Event::Data),
                validate_context,
            );
        }

        match self.context.state {
            State::Connect => (
                Some(vec![format!("{} {}", Status::ServiceReady, self.banner)]),
                Event::ConnectionKeepAlive,
            ),
            State::Helo => (
                Some(vec![format!(
                    "{} Hello {}",
                    Status::Ok,
                    std::str::from_utf8(&self.context.message).unwrap()
                )]),
                Event::ConnectionKeepAlive,
            ),
            State::Ehlo | State::Help => self.response_ehlo_help(),
            State::StartTLS if self.tls_context.is_some() => (
                Some(vec![format!("{} Ready to begin TLS", Status::ServiceReady)]),
                Event::ConnectionKeepAlive,
            ),
            State::MailFrom => {
                if modules::dispatch(
                    modules::Event::Validate(validate::Event::MailFrom),
                    validate_context,
                ) {
                    (
                        Some(vec![format!("{} Ok", Status::Ok)]),
                        Event::ConnectionKeepAlive,
                    )
                } else {
                    (
                        Some(vec![format!("{} Goodbye", Status::Error)]),
                        Event::ConnectionClose,
                    )
                }
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
            State::InvalidCommandSequence => (
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
            _ => (
                Some(vec![format!(
                    "{} Invalid command",
                    Status::InvalidCommandSequence,
                )]),
                Event::ConnectionClose,
            ),
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
                    self.context.message.extend(received);

                    if self.context.message.ends_with(b"\r\n.\r\n") {
                        self.context = Context {
                            state: State::PostDot,
                            message: self.context.message.clone(),
                            sent: false,
                        };

                        validate_context.data = Some(self.context.message.clone().into());
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
    use std::{collections::HashMap, io::Cursor, sync::Arc};

    use empath_common::{context::Context, status::Status};
    use empath_ffi::modules::{self, MODULE_STORE, Module, validate};

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
            SessionConfig {
                extensions: Vec::default(),
                tls_context: None,
                spool: None,
                banner: banner.to_string(),
                init_context: HashMap::default(),
            },
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
            SessionConfig {
                extensions: Vec::default(),
                tls_context: None,
                spool: None,
                banner: banner.to_string(),
                init_context: HashMap::default(),
            },
        );

        let _ = session.response(&mut context).await;
        let response = session.receive(&mut context).await;
        assert!(response.is_ok());
        assert!(!response.unwrap());

        let response = session.response(&mut context).await;
        assert!(response.0.is_some());
        assert_eq!(
            response.0.unwrap().first().unwrap(),
            &format!("{} Hello {host}", Status::Ok)
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
            SessionConfig {
                extensions: Vec::default(),
                tls_context: None,
                spool: Some(mock_spool.clone() as Arc<dyn empath_spool::Spool>),
                banner: banner.to_string(),
                init_context: HashMap::default(),
            },
        );

        // Simulate HELO state and receiving DATA
        session.context.state = State::RcptTo;
        let mut sender_addrs = mailparse::addrparse("test@example.com").unwrap();
        context
            .envelope
            .sender_mut()
            .replace(sender_addrs.remove(0));
        context
            .envelope
            .recipients_mut()
            .replace(mailparse::addrparse("recipient@example.com").unwrap());

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
            SessionConfig {
                extensions: Vec::default(),
                tls_context: None,
                spool: None,
                banner: banner.to_string(),
                init_context: HashMap::default(),
            },
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

        if let Module::TestModule(mute) = MODULE_STORE.read().unwrap().first().unwrap() {
            assert!(
                mute.lock()
                    .unwrap()
                    .validators_called
                    .contains(&validate::Event::MailFrom)
            );
        } else {
            panic!("Expected TestModule to exist");
        }
    }
}
