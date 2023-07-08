use std::{
    fmt::Display,
    fs::File,
    io::BufReader,
    net::{IpAddr, Ipv6Addr, SocketAddr},
    sync::{atomic::AtomicU64, Arc},
};

use empath_common::{
    context,
    ffi::module::{self, dispatch, Error},
    incoming, internal,
    listener::Listener,
    outgoing,
};
use empath_smtp_proto::{command::Command, extensions::Extension, phase::Phase, status::Status};
use mailparse::MailParseError;
use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::TcpListener,
};
use tokio_rustls::{
    rustls::{server::AllowAnyAnonymousOrAuthenticatedClient, RootCertStore, ServerConfig},
    server::TlsStream,
    TlsAcceptor,
};

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

impl TlsContext {
    fn is_enabled(&self) -> bool {
        !self.certificate.is_empty() && !self.key.is_empty()
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Smtp {
    address: IpAddr,
    port: u16,
    #[serde(skip)]
    context: Context,
    #[serde(skip)]
    extensions: Vec<Extension>,
    #[serde(default)]
    banner: String,
    #[serde(default)]
    tls_context: TlsContext,
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

pub enum Connection<Stream: AsyncRead + AsyncWrite + Unpin + Send + Sync> {
    Plain { stream: Stream },
    Tls { stream: Box<TlsStream<Stream>> },
}

impl<Stream: AsyncRead + AsyncWrite + Unpin + Send + Sync> Connection<Stream> {
    async fn send<S: Display + Send + Sync>(&mut self, response: &S) -> std::io::Result<usize> {
        match self {
            Self::Plain { stream } => stream.write(format!("{response}\r\n").as_bytes()).await,
            Self::Tls { stream } => stream.write(format!("{response}\r\n").as_bytes()).await,
        }
    }

    async fn upgrade(self, tls_context: &TlsContext) -> std::io::Result<Self> {
        if !tls_context.is_enabled() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                "No tls certificate or key provided",
            ));
        }

        let certfile = File::open(&tls_context.certificate).expect("cannot open certificate file");
        let mut reader = BufReader::new(certfile);
        let certs = rustls_pemfile::certs(&mut reader)
            .unwrap()
            .iter()
            .map(|v| tokio_rustls::rustls::Certificate(v.clone()))
            .collect::<Vec<_>>();

        let keyfile = File::open(&tls_context.key).expect("cannot open private key file");
        let mut reader = BufReader::new(keyfile);

        let key =
            match rustls_pemfile::read_one(&mut reader).expect("cannot parse private key file") {
                Some(
                    rustls_pemfile::Item::RSAKey(key)
                    | rustls_pemfile::Item::PKCS8Key(key)
                    | rustls_pemfile::Item::ECKey(key),
                ) => tokio_rustls::rustls::PrivateKey(key),
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
                Self::Plain { stream } => Box::new(acceptor.accept(stream).await?),
                Self::Tls { stream } => Box::new(acceptor.accept(stream.into_inner().0).await?),
            },
        })
    }

    async fn receive(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
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
            tls_context: TlsContext::default(),
        }
    }
}

impl Smtp {
    async fn connect<Stream: AsyncRead + AsyncWrite + Unpin + Send + Sync>(
        mut self,
        queue: Arc<AtomicU64>,
        stream: Stream,
        peer: SocketAddr,
    ) -> std::io::Result<()> {
        let mut connection = Connection::Plain { stream };
        let mut vctx = context::Context::default();

        if self.tls_context.is_enabled() {
            self.extensions.push(Extension::STARTTLS);
        }

        internal!("Connected to {peer}");

        loop {
            let (response, ev) = self.response(&queue, &mut vctx);
            self.context.sent = true;

            for response in response.unwrap_or_default() {
                outgoing!("{response}");

                connection.send(&response).await.map_err(|err| {
                    internal!("Error: {err}");
                    std::io::Error::new(std::io::ErrorKind::ConnectionAborted, err.to_string())
                })?;
            }

            if Event::ConnectionClose == ev {
                return Ok(());
            }

            if self.tls_context.is_enabled() && self.context.state == Phase::StartTLS {
                connection = connection.upgrade(&self.tls_context).await?;
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
            return (None, Event::ConnectionKeepAlive);
        }

        if Phase::DataReceived == self.context.state {
            dispatch(module::Event::ValidateData, vctx);
        }

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
            Phase::StartTLS if self.tls_context.is_enabled() => (
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
                let default = format!("Ok: queued as {queue}");
                let response = vctx.data_response.as_ref().unwrap_or(&default);

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

    async fn receive<Stream: AsyncRead + AsyncWrite + Unpin + Send + Sync>(
        &mut self,
        connection: &mut Connection<Stream>,
        vctx: &mut context::Context,
    ) -> std::io::Result<bool> {
        let mut received_data = [0; 4096];

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
