use std::{
    collections::HashMap,
    fmt::Display,
    fs,
    future::Future,
    io::{self, BufReader, Read, Write},
    net::{IpAddr, Ipv6Addr, SocketAddr, TcpListener, TcpStream},
    sync::{atomic::AtomicU64, Arc},
    time::Duration,
};

use empath_common::{context::ValidationContext, listener::Listener, log::Logger};
use empath_smtp_proto::{command::Command, extensions::Extension, phase::Phase, status::Status};
use mailparse::MailParseError;
use rustls::{
    server::AllowAnyAnonymousOrAuthenticatedClient, RootCertStore, ServerConfig, ServerConnection,
};
use serde::{Deserialize, Serialize};
use smol::{io::AsyncWriteExt, Async};
use smol_timeout::TimeoutExt;

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
        Context {
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

pub(crate) type Handle = fn(&mut ValidationContext) -> Result<(), SMTPError>;
pub(crate) type Handles = HashMap<Phase, Vec<Handle>>;
pub(crate) type Response = Result<(Option<Vec<String>>, Event), SMTPError>;

impl From<MailParseError> for SMTPError {
    fn from(err: MailParseError) -> Self {
        SMTPError {
            status: Status::Error,
            message: err.to_string(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub struct SmtpListener {
    address: IpAddr,
    port: u16,
    #[serde(skip)]
    context: Context,
    #[serde(skip)]
    handlers: Handles,
    extensions: Vec<Extension>,
}

#[typetag::serde]
#[async_trait::async_trait]
impl Listener for SmtpListener {
    async fn spawn(&self) {
        Logger::internal(&format!(
            "Starting SMTP Listener on: {}:{}",
            self.address, self.port
        ));

        let smtplistener = self.clone();
        let listener =
            Async::<TcpListener>::bind(SocketAddr::new(smtplistener.address, smtplistener.port))
                .expect("Unable to start smtp session");
        let queue = Arc::new(AtomicU64::default());

        loop {
            let (stream, address) = listener
                .accept()
                .await
                .expect("Unable to accept connection");

            smol::spawn(
                smtplistener
                    .clone()
                    .connect(Arc::clone(&queue), stream, address),
            )
            .detach();
        }
    }
}

pub struct Connection {
    stream: Async<TcpStream>,
    tls: Option<ServerConnection>,
}

async fn with_timeout<F, T>(timeout: Duration, af: F) -> io::Result<T>
where
    F: Future<Output = io::Result<T>>,
{
    af.timeout(timeout).await.unwrap_or_else(|| {
        Err(io::Error::new(
            io::ErrorKind::ConnectionAborted,
            "Connection rejected due to timeout",
        ))
    })
}

impl Connection {
    async fn send<S: Display>(&mut self, response: &S) -> io::Result<()> {
        with_timeout(Duration::from_secs(30), self.stream.writable()).await?;

        with_timeout(
            Duration::from_secs(30),
            self.stream.write_with(|mut stream| {
                if let Some(ref mut tls) = self.tls {
                    tls.write_tls(&mut stream)?;
                    write!(tls.writer(), "{response}\r\n")?;
                    tls.write_tls(&mut stream)?;
                    tls.writer().flush()
                } else {
                    write!(stream, "{response}\r\n")
                }
            }),
        )
        .await
    }

    async fn upgrade(&mut self) -> io::Result<()> {
        let certfile = fs::File::open("certificate.crt").expect("cannot open certificate file");
        let mut reader = BufReader::new(certfile);
        let certs = rustls_pemfile::certs(&mut reader)
            .unwrap()
            .iter()
            .map(|v| rustls::Certificate(v.clone()))
            .collect::<Vec<_>>();

        let keyfile = fs::File::open("private.key").expect("cannot open private key file");
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

        let mut tls_connection = ServerConnection::new(Arc::new(config)).unwrap();

        self.stream.readable().await?;

        while tls_connection.read_tls(self.stream.get_mut()).is_ok() {
            match tls_connection.process_new_packets() {
                Ok(a) => {
                    if a.tls_bytes_to_write() > 0 {
                        tls_connection.write_tls(self.stream.get_mut())?;
                    }
                }
                Err(err) => {
                    eprintln!("ERROR WHILE READING PACKETS: {}", err);
                    return Err(io::Error::new(
                        io::ErrorKind::ConnectionAborted,
                        err.to_string(),
                    ));
                }
            }
        }

        self.tls = Some(tls_connection);

        Ok(())
    }

    async fn receive(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        with_timeout(Duration::from_secs(30), self.stream.readable()).await?;

        with_timeout(
            Duration::from_secs(30),
            self.stream.read_with(|mut stream| {
                if let Some(ref mut tls) = self.tls {
                    if tls.wants_read() {
                        tls.read_tls(&mut stream)?;
                    }
                    tls.process_new_packets().map_err(|e| {
                        io::Error::new(io::ErrorKind::ConnectionAborted, e.to_string())
                    })?;
                    tls.reader().read(buf)
                } else {
                    stream.read(buf)
                }
            }),
        )
        .await
    }
}

impl Default for SmtpListener {
    fn default() -> Self {
        SmtpListener {
            address: IpAddr::V6(Ipv6Addr::UNSPECIFIED),
            port: 1025,
            context: Context::default(),
            handlers: Default::default(),
            extensions: Default::default(),
        }
    }
}

impl SmtpListener {
    async fn connect(
        mut self,
        queue: Arc<AtomicU64>,
        stream: Async<TcpStream>,
        peer: SocketAddr,
    ) -> std::io::Result<()> {
        let mut connection = Connection { stream, tls: None };
        let mut vctx = ValidationContext::default();

        Logger::internal(&format!("Connected to {}", peer));

        loop {
            match self.response(&queue, &mut vctx) {
                Ok((response, ev)) => {
                    self.context.sent = true;

                    for response in response.unwrap_or_default() {
                        Logger::outgoing(&response);

                        connection.send(&response).await.map_err(|err| {
                            Logger::internal(&format!("Error: {err}"));
                            io::Error::new(io::ErrorKind::ConnectionAborted, err.to_string())
                        })?;
                    }

                    if Event::ConnectionClose == ev {
                        return Ok(());
                    }
                }
                Err(SMTPError { status, message }) => {
                    let response = format!("{status} {message}");
                    Logger::outgoing(&response);
                    connection.send(&response).await?;
                }
            }

            if self.context.state == Phase::StartTLS {
                connection.upgrade().await?;
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
                    Logger::internal("Connection closed");
                    connection.stream.flush().await?;
                    return Ok(());
                }
            }
        }
    }

    /// Generate the response(s) that should be sent back to the client
    /// depending on the servers state
    fn response(&mut self, queue: &Arc<AtomicU64>, vctx: &mut ValidationContext) -> Response {
        if self.context.sent {
            return Ok((None, Event::ConnectionKeepAlive));
        }

        if let Some(handlers) = self.handlers.get(&self.context.state) {
            for handler in handlers {
                handler(vctx)?;
            }
        }

        let status = match self.context.state {
            Phase::Connect => (
                Some(vec![format!("{} localhost", Status::ServiceReady)]),
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
        vctx: &mut ValidationContext,
    ) -> std::io::Result<bool> {
        let mut received_data = vec![0; 4096];

        match connection.receive(&mut received_data).await {
            // Consider any errors received here to be fatal
            Err(err) => {
                Logger::internal(&format!("Error: {err}"));
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

                    Logger::incoming(&command.to_string());

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

    #[must_use]
    pub fn on_port(mut self, port: u16) -> SmtpListener {
        self.port = port;

        self
    }

    /// Add a handler for a specific `State`. This is useful for doing specific
    /// checks at certain points, e.g. on `Connect` you can check the IP against
    /// a block list and abort the connection due to suspected spam.
    ///
    /// # Examples
    ///
    /// ```
    /// use empath_server::smtp::SmtpListener;
    /// use empath_smtp_proto::phase::Phase;
    ///
    /// let server = SmtpListener::default()
    ///     .handle(Phase::Connect, |vctx| {
    ///         println!("Connected!");
    ///         Ok(())
    ///     });
    /// ```
    ///
    /// # Panics
    ///
    /// Panics if the server is unable to obtain a write lock on the internal handles
    /// it has
    #[must_use]
    pub fn handle(mut self, command: Phase, handler: Handle) -> SmtpListener {
        self.handlers
            .entry(command)
            .and_modify(|hdlr| hdlr.push(handler))
            .or_insert(vec![handler]);

        self
    }

    /// Add an `Extension` to advertise that the server supports, as well
    /// as request the server to actually handle the command the extension
    /// pertains to.
    ///
    /// # Examples
    ///
    /// ```
    /// use empath_server::smtp::SmtpListener;
    /// use empath_common::listener::Listener;
    /// use empath_smtp_proto::extensions::Extension::STARTTLS;
    ///
    /// let server = SmtpListener::default().extension(STARTTLS);
    /// server.spawn(); // The server will now advertise that it supports the
    ///                 // STARTTLS extension, and will also accept it as a
    ///                 // command
    /// ```
    ///
    /// # Panics
    ///
    /// Panics if the server is unable to obtain a write lock on the internal
    /// extensions it has
    #[must_use]
    pub fn extension(mut self, extension: Extension) -> SmtpListener {
        self.extensions.push(extension);

        self
    }
}
