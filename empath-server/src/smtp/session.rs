use std::{
    net::SocketAddr,
    sync::{atomic::AtomicU64, Arc},
};

use mailparse::MailParseError;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncWrite};

use empath_common::{context, ffi::module, incoming, internal, outgoing};
use empath_smtp_proto::{command::Command, extensions::Extension, phase::Phase, status::Status};

use super::connection::Connection;

#[repr(C)]
#[derive(PartialEq, Eq)]
pub enum Event {
    ConnectionClose,
    ConnectionKeepAlive,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Context {
    pub state: Phase,
    pub message: Vec<u8>,
    pub sent: bool,
}

impl Default for Context {
    fn default() -> Self {
        Self {
            state: Phase::Connect,
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

impl From<module::Error> for SMTPError {
    fn from(value: module::Error) -> Self {
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
            extensions.push(Extension::STARTTLS);
        }

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

    pub(crate) async fn run(mut self) -> std::io::Result<()> {
        async fn run_inner<Stream: AsyncRead + AsyncWrite + Unpin + Send + Sync>(
            mut session: Session<Stream>,
            validate_context: &mut context::Context,
        ) -> std::io::Result<()> {
            loop {
                let (response, ev) = session.response(validate_context);
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
                    && session.context.state == Phase::StartTLS
                {
                    session.connection = session.connection.upgrade(&session.tls_context).await?;
                    session.context = Context {
                        sent: true,
                        ..Default::default()
                    };
                } else if session.receive(validate_context).await.unwrap_or(true) {
                    return Ok(());
                }
            }
        }

        let mut validate_context = context::Context::default();

        internal!("Connected to {}", self.peer);
        module::dispatch(
            module::Event::Event(module::Ev::ConnectionOpened),
            &mut validate_context,
        );

        if !module::dispatch(
            module::Event::Validate(module::ValidateEvent::Connect),
            &mut validate_context,
        ) {
            self.context.state = Phase::Reject;
        }

        let result = run_inner(self, &mut validate_context).await;

        module::dispatch(
            module::Event::Event(module::Ev::ConnectionClosed),
            &mut validate_context,
        );
        internal!("Connection closed");

        result
    }

    /// Generate the response(s) that should be sent back to the client
    /// depending on the servers state
    #[allow(clippy::too_many_lines)]
    fn response(&mut self, validate_context: &mut context::Context) -> Response {
        if self.context.sent {
            return (None, Event::ConnectionKeepAlive);
        }

        if Phase::DataReceived == self.context.state {
            module::dispatch(
                module::Event::Validate(module::ValidateEvent::Data),
                validate_context,
            );
        }

        let status = match self.context.state {
            Phase::Connect => (
                Some(vec![format!("{} {}", Status::ServiceReady, self.banner)]),
                Event::ConnectionKeepAlive,
            ),
            Phase::Ehlo | Phase::Helo => {
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
            Phase::StartTLS if self.tls_context.is_available() => (
                Some(vec![format!("{} Ready to begin TLS", Status::ServiceReady)]),
                Event::ConnectionKeepAlive,
            ),
            Phase::MailFrom | Phase::RcptTo => (
                Some(vec![format!("{} Ok", Status::Ok)]),
                Event::ConnectionKeepAlive,
            ),
            Phase::Data => {
                self.context.state = Phase::Reading;
                (
                    Some(vec![format!(
                        "{} End data with <CR><LF>.<CR><LF>",
                        Status::StartMailInput
                    )]),
                    Event::ConnectionKeepAlive,
                )
            }
            Phase::DataReceived => {
                let queue = self
                    .queue
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let default = format!("Ok: queued as {queue}");
                let response = validate_context.data_response.as_ref().unwrap_or(&default);

                (
                    Some(vec![format!("{} {}", Status::Ok, response)]),
                    Event::ConnectionKeepAlive,
                )
            }
            Phase::Quit => (
                Some(vec![format!("{} Bye", Status::GoodBye)]),
                Event::ConnectionClose,
            ),
            Phase::Reading | Phase::Close => (None, Event::ConnectionKeepAlive),
            Phase::InvalidCommandSequence => (
                Some(vec![format!(
                    "{} {}",
                    Status::InvalidCommandSequence,
                    self.context.state
                )]),
                Event::ConnectionClose,
            ),
            Phase::Reject => (
                Some(vec![format!(
                    "{} {}",
                    Status::Unavailable,
                    validate_context.data_response.clone().unwrap_or_default()
                )]),
                Event::ConnectionClose,
            ),
            _ => (
                Some(vec![format!(
                    "{} Invalid command '{}'",
                    Status::InvalidCommandSequence,
                    std::str::from_utf8(&self.context.message).unwrap()
                )]),
                Event::ConnectionClose,
            ),
        };

        status
    }

    async fn receive(&mut self, validate_context: &mut context::Context) -> std::io::Result<bool> {
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
                Ok(false)
            }
            Ok(bytes_read) => {
                let received = &received_data[..bytes_read];

                if self.context.state == Phase::Reading {
                    self.context.message.extend(received);

                    if self.context.message.ends_with(b"\r\n.\r\n") {
                        self.context = Context {
                            state: Phase::DataReceived,
                            message: self.context.message.clone(),
                            sent: false,
                        };

                        validate_context.data = Some(self.context.message.clone().into());
                    }
                } else {
                    let command = Command::from(received);
                    let message = command.inner().into_bytes();

                    incoming!("{command}");

                    self.context = Context {
                        state: self.context.state.transition(command, validate_context),
                        message,
                        sent: false,
                    };
                }

                Ok(false)
            }
        }
    }
}
