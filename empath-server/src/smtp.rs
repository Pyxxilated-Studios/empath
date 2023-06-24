use std::{
    fmt::Display,
    fs,
    io::{self, BufReader},
    net::{IpAddr, Ipv6Addr, SocketAddr},
    sync::{atomic::AtomicU64, Arc},
};

use empath_common::{
    context,
    ffi::module::{self, Error},
    incoming, internal,
    listener::Listener,
    outgoing,
};
use empath_smtp_proto::{command::Command, extensions::Extension, phase::Phase, status::Status};
use mailparse::MailParseError;
use rustls::{server::AllowAnyAnonymousOrAuthenticatedClient, RootCertStore, ServerConfig};
use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};
use tokio_rustls::{server::TlsStream, TlsAcceptor};

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

pub type Response = Result<(Option<Vec<String>>, Event), SMTPError>;

impl From<MailParseError> for SMTPError {
    fn from(err: MailParseError) -> Self {
        Self {
            status: Status::Error,
            message: err.to_string(),
        }
    }
}

impl From<Error> for SMTPError {
    fn from(value: Error) -> Self {
        Self {
            status: Status::Error,
            message: format!("{value}"),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct TlsContext {
    certificate: String,
    key: String,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Smtp {
    address: IpAddr,
    port: u16,
    #[serde(skip)]
    context: Context,
    #[serde(default)]
    extensions: Vec<Extension>,
    #[serde(default)]
    banner: String,
    #[serde(default)]
    tls_certificate: TlsContext,
}

#[typetag::serde]
#[async_trait::async_trait]
impl Listener for Smtp {
    async fn spawn(&self) {
        internal!(
            level = INFO,
            "Starting SMTP Listener on: {}:{}",
            self.address,
            self.port
        );

        let smtplistener = self.clone();
        let listener = TcpListener::bind(SocketAddr::new(smtplistener.address, smtplistener.port))
            .await
            .expect("Unable to start smtp session");
        let queue = Arc::new(AtomicU64::default());

        loop {
            let (stream, address) = listener
                .accept()
                .await
                .expect("Unable to accept connection");

            tokio::spawn(
                smtplistener
                    .clone()
                    .connect(Arc::clone(&queue), stream, address),
            );
        }
    }
}

pub enum Connection {
    Plain { stream: TcpStream },
    Tls { stream: TlsStream<TcpStream> },
}

impl Connection {
    async fn send<S: Display + Send + Sync>(&mut self, response: &S) -> io::Result<usize> {
        match self {
            Self::Plain { stream } => stream.write(format!("{response}\r\n").as_bytes()).await,
            Self::Tls { stream } => stream.write(format!("{response}\r\n").as_bytes()).await,
        }
    }

    async fn upgrade(self, tls_context: &TlsContext) -> io::Result<Self> {
        if tls_context.certificate.is_empty() || tls_context.key.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                "No tls certificate or key provided",
            ));
        }

        let certfile =
            fs::File::open(&tls_context.certificate).expect("cannot open certificate file");
        let mut reader = BufReader::new(certfile);
        let certs = rustls_pemfile::certs(&mut reader)
            .unwrap()
            .iter()
            .map(|v| rustls::Certificate(v.clone()))
            .collect::<Vec<_>>();

        let keyfile = fs::File::open(&tls_context.key).expect("cannot open private key file");
        let mut reader = BufReader::new(keyfile);

        let key =
            match rustls_pemfile::read_one(&mut reader).expect("cannot parse private key file") {
                Some(
                    rustls_pemfile::Item::RSAKey(key)
                    | rustls_pemfile::Item::PKCS8Key(key)
                    | rustls_pemfile::Item::ECKey(key),
                ) => rustls::PrivateKey(key),
                _ => panic!("Unable to determine key file"),
            };

        let config = ServerConfig::builder()
            .with_safe_default_cipher_suites()
            .with_safe_default_kx_groups()
            .with_safe_default_protocol_versions()
            .expect("Invalid TLS Configuration")
            .with_client_cert_verifier(Arc::new(AllowAnyAnonymousOrAuthenticatedClient::new({
                let mut cert_store = RootCertStore::empty();
                cert_store.add(certs.first().unwrap()).unwrap();
                cert_store
            })))
            .with_single_cert_with_ocsp_and_sct(certs, key, Vec::new(), Vec::new())
            .expect("Invalid Cert Configuration");

        let acceptor = TlsAcceptor::from(Arc::new(config));

        Ok(Self::Tls {
            stream: match self {
                Self::Plain { stream } => acceptor.accept(stream).await?,
                Self::Tls { stream } => stream,
            },
        })
    }

    async fn receive(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Plain { stream } => stream.read(buf).await,
            Self::Tls { stream } => stream.read(buf).await,
        }
    }
}

impl Default for Smtp {
    fn default() -> Self {
        Self {
            address: IpAddr::V6(Ipv6Addr::UNSPECIFIED),
            port: 1025,
            context: Context::default(),
            extensions: Vec::default(),
            banner: String::default(),
            tls_certificate: TlsContext::default(),
        }
    }
}

impl Smtp {
    async fn connect(
        mut self,
        queue: Arc<AtomicU64>,
        stream: TcpStream,
        peer: SocketAddr,
    ) -> std::io::Result<()> {
        let mut connection = Connection::Plain { stream };
        let mut vctx = context::Context::default();

        internal!("Connected to {peer}");

        loop {
            match self.response(&queue, &mut vctx) {
                Ok((response, ev)) => {
                    self.context.sent = true;

                    for response in response.unwrap_or_default() {
                        outgoing!("{response}");

                        connection.send(&response).await.map_err(|err| {
                            internal!("Error: {err}");
                            io::Error::new(io::ErrorKind::ConnectionAborted, err.to_string())
                        })?;
                    }

                    if Event::ConnectionClose == ev {
                        return Ok(());
                    }
                }
                Err(SMTPError { status, message }) => {
                    let response = format!("{status} {message}");
                    outgoing!("{response}");
                    connection.send(&response).await?;
                }
            }

            if self.context.state == Phase::StartTLS {
                connection = connection.upgrade(&self.tls_certificate).await?;
                self.context = Context {
                    sent: true,
                    ..Default::default()
                };
            } else {
                let connection_closed = matches!(
                    self.receive(&mut connection, &mut vctx).await,
                    Ok(true) | Err(_)
                );

                if connection_closed {
                    internal!("Connection closed");
                    return Ok(());
                }
            }
        }
    }

    /// Generate the response(s) that should be sent back to the client
    /// depending on the servers state
    fn response(&mut self, queue: &Arc<AtomicU64>, vctx: &mut context::Context) -> Response {
        if self.context.sent {
            return Ok((None, Event::ConnectionKeepAlive));
        }

        match self.context.state {
            Phase::DataReceived => module::dispatch("validate_data", vctx),
            _ => Ok(()),
        }?;

        let status = match self.context.state {
            Phase::Connect => (
                Some(vec![format!(
                    "{} {}",
                    Status::ServiceReady,
                    if self.banner.is_empty() {
                        "localhost"
                    } else {
                        &self.banner
                    }
                )]),
                Event::ConnectionKeepAlive,
            ),
            Phase::Ehlo | Phase::Helo => {
                let mut response = vec![format!(
                    "{}{}Hello {}",
                    Status::Ok,
                    if self.extensions.is_empty() { ' ' } else { '-' },
                    std::str::from_utf8(&self.context.message).unwrap()
                )];

                for (idx, extension) in self.extensions.iter().enumerate() {
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
                }
                (Some(response), Event::ConnectionKeepAlive)
            }
            Phase::StartTLS => (
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
                let queue = queue.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                (
                    Some(vec![format!("{} Ok: queued as {}", Status::Ok, queue)]),
                    Event::ConnectionKeepAlive,
                )
            }
            Phase::Quit => (
                Some(vec![format!("{} Bye", Status::GoodBye)]),
                Event::ConnectionClose,
            ),
            Phase::Invalid => (
                Some(vec![format!(
                    "{} Invalid command '{}'",
                    Status::InvalidCommandSequence,
                    std::str::from_utf8(&self.context.message).unwrap()
                )]),
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
        };

        Ok(status)
    }

    async fn receive(
        &mut self,
        connection: &mut Connection,
        vctx: &mut context::Context,
    ) -> std::io::Result<bool> {
        let mut received_data = vec![0; 4096];

        match connection.receive(&mut received_data).await {
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

                        vctx.data = Some(self.context.message.clone());
                    }
                } else {
                    let command = Command::from(received);
                    let message = command.inner().into_bytes();

                    incoming!("{command}");

                    self.context = Context {
                        state: self.context.state.transition(command, vctx),
                        message,
                        sent: false,
                    };
                }

                Ok(false)
            }
        }
    }
}
