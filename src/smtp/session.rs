use std::{
    net::SocketAddr,
    sync::{atomic::AtomicU64, Arc},
};

use mailparse::MailParseError;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncWrite};

use crate::{
    ffi::modules,
    incoming, internal, outgoing,
    smtp::{command::Command, session::modules::validate},
    traits::fsm::FiniteStateMachine,
};

use super::{connection::Connection, context, extensions::Extension, status::Status, State};

#[repr(C)]
#[derive(PartialEq, Eq)]
pub enum Event {
    ConnectionClose,
    ConnectionKeepAlive,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct TlsContext {
    pub(crate) certificate: String,
    pub(crate) key: String,
}

impl TlsContext {
    pub(crate) fn is_available(&self) -> bool {
        !self.certificate.is_empty() && !self.key.is_empty()
    }
}

pub struct Session<Stream: AsyncRead + AsyncWrite + Unpin + Send + Sync> {
    queue: Arc<AtomicU64>,
    peer: SocketAddr,
    context: Context,
    extensions: Arc<[Extension]>,
    banner: String,
    tls_context: TlsContext,
    connection: Connection<Stream>,
}

impl<Stream: AsyncRead + AsyncWrite + Unpin + Send + Sync> Session<Stream> {
    pub(crate) fn create(
        queue: Arc<AtomicU64>,
        stream: Stream,
        peer: SocketAddr,
        mut extensions: Vec<Extension>,
        tls_context: TlsContext,
        banner: String,
    ) -> Self {
        if tls_context.is_available() {
            extensions.push(Extension::Starttls);
        }

        tracing::debug!("Extensions ({peer}): {extensions:#?}");

        Self {
            queue,
            peer,
            connection: Connection::Plain { stream },
            context: Context::default(),
            extensions: extensions.into(),
            tls_context,
            banner: if banner.is_empty() {
                "localhost".to_string()
            } else {
                banner
            },
        }
    }

    pub(crate) async fn run(mut self) -> anyhow::Result<()> {
        async fn run_inner<Stream: AsyncRead + AsyncWrite + Unpin + Send + Sync>(
            mut session: Session<Stream>,
            validate_context: &mut context::Context,
        ) -> anyhow::Result<()> {
            loop {
                let (response, ev) = session.response(validate_context);
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
                } else if session.tls_context.is_available()
                    && session.context.state == State::StartTLS
                {
                    let (conn, info) = session.connection.upgrade(&session.tls_context).await?;

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
                        tracing::debug!("Connection successfully upgraded with {info:#?}");
                    } else {
                        session.context.sent = false;
                        session.context.state = State::Reject;
                        validate_context.response =
                            Some((Status::Error, String::from("STARTTLS failed")));
                    }
                } else if session.receive(validate_context).await.unwrap_or(true) {
                    return Ok(());
                }
            }
        }

        let mut validate_context = context::Context::default();

        internal!("Connected to {}", self.peer);
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

        let result = run_inner(self, &mut validate_context).await;

        internal!("Connection closed");
        modules::dispatch(
            modules::Event::Event(modules::Ev::ConnectionClosed),
            &mut validate_context,
        );

        result
    }

    /// Generate the response(s) that should be sent back to the client
    /// depending on the servers state
    #[expect(clippy::too_many_lines)]
    fn response(&mut self, validate_context: &mut context::Context) -> Response {
        if self.context.sent {
            return (None, Event::ConnectionKeepAlive);
        }

        if State::PostDot == self.context.state {
            modules::dispatch(
                modules::Event::Validate(validate::Event::Data),
                validate_context,
            );
        }

        if let Some((status, ref response)) = validate_context.response {
            return (
                Some(vec![format!("{} {}", status, response)]),
                if status.is_permanent() {
                    Event::ConnectionClose
                } else {
                    Event::ConnectionKeepAlive
                },
            );
        }

        let response = match self.context.state {
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
            State::Ehlo => {
                let response = vec![format!(
                    "{}{}Hello {}",
                    Status::Ok,
                    if self.extensions.is_empty() { ' ' } else { '-' },
                    std::str::from_utf8(&self.context.message).unwrap()
                )];

                (
                    Some(self.extensions.iter().enumerate().fold(
                        response,
                        |mut response, (idx, extension)| {
                            response.push(format!(
                                "{}{}{}",
                                Status::Ok,
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
            State::StartTLS if self.tls_context.is_available() => (
                Some(vec![format!("{} Ready to begin TLS", Status::ServiceReady)]),
                Event::ConnectionKeepAlive,
            ),
            State::MailFrom | State::RcptTo => (
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
            State::PostDot => {
                let queue = self
                    .queue
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let default = (Status::Ok, format!("Ok: queued as {queue}"));
                let response = validate_context.response.as_ref().unwrap_or(&default);

                (
                    Some(vec![format!("{} {}", response.0, response.1)]),
                    Event::ConnectionKeepAlive,
                )
            }
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
        };

        response
    }

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
                    let command = Command::try_from(received).map_or_else(|e| e, |c| c);
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

    use crate::{
        ffi::modules::{self, test::test_module, Module, Test, MODULE_STORE},
        smtp::{context::Context, session::Session, status::Status, State},
    };

    use super::TlsContext;

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
            Vec::default(),
            TlsContext::default(),
            banner.to_string(),
        );

        let response = session.response(&mut context);
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
            Vec::default(),
            TlsContext::default(),
            banner.to_string(),
        );

        let _ = session.response(&mut context);
        let response = session.receive(&mut context).await;
        assert!(response.is_ok());
        assert!(!response.unwrap());

        let response = session.response(&mut context);
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
    async fn modules() {
        let banner = "testing";
        let mut context = Context::default();
        let mut cursor = Cursor::<Vec<u8>>::default();
        cursor
            .get_mut()
            .extend_from_slice("MAIL FROM: test@gmail.com".as_bytes());

        let module = test_module();
        let inited = modules::init(vec![module]);
        assert!(inited.is_ok());

        let mut session = Session::create(
            Arc::default(),
            cursor,
            "[::]:25".parse().unwrap(),
            Vec::default(),
            TlsContext::default(),
            banner.to_string(),
        );

        session.context.state = State::Helo;

        let _ = session.response(&mut context);
        let response = session.receive(&mut context).await;
        assert!(response.is_ok());
        assert!(!response.unwrap());

        let response = session.response(&mut context);
        assert!(response.0.is_some());
        assert_eq!(
            response.0.unwrap().first().unwrap(),
            &format!("{} Ok", Status::Ok)
        );

        let response = session.receive(&mut context).await;
        assert!(response.is_ok_and(|v| v));

        let store = MODULE_STORE.read().unwrap();
        let module = store.first().unwrap();
        if let Module::TestModule(mute) = module {
            let test = mute.lock().unwrap();
            assert_eq!(
                *test,
                Test {
                    validate_mail_from_called: true,
                    ..Test::default()
                }
            );
        } else {
            panic!("Expected TestModule to exist");
        }
    }
}
