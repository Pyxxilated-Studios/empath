use core::panic;
use std::collections::HashMap;
use std::fmt::{Debug, Display};
use std::future::Future;
use std::io::{BufReader, Read, Write};
use std::net::{IpAddr, Ipv6Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::{atomic::AtomicU64, Arc, RwLock};
use std::time::Duration;
use std::{fs, io};

use mailparse::MailParseError;
use rustls::server::AllowAnyAnonymousOrAuthenticatedClient;
use rustls::{RootCertStore, ServerConfig, ServerConnection};
use smol::{io::AsyncWriteExt, Async};
use smol_timeout::TimeoutExt;
// use trust_dns_resolver::{
//     config::{ResolverConfig, ResolverOpts},
//     Resolver,
// };

use crate::common::{command::Command, extensions::Extension, status::Status};
use crate::log::Logger;

pub mod phase;
use phase::Phase;

pub mod context;
use context::ValidationContext;

#[repr(C)]
#[derive(PartialEq, Eq)]
pub enum Event {
    ConnectionClose,
    ConnectionKeepAlive,
}

#[derive(Debug, Clone)]
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

#[derive(Clone)]
pub struct Server {
    address: IpAddr,
    port: u16,
    handlers: Arc<RwLock<Handles>>,
    extensions: Arc<RwLock<Vec<Extension>>>,
    context: Context,
}

pub struct Connection {
    stream: Async<TcpStream>,
    tls: Option<ServerConnection>,
    peer: SocketAddr,
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

impl Default for Server {
    fn default() -> Self {
        Server {
            address: IpAddr::V6(Ipv6Addr::UNSPECIFIED),
            port: 1025,
            handlers: Arc::default(),
            extensions: Arc::default(),
            context: Context::default(),
        }
    }
}

impl Server {
    /// Add a handler for a specific `State`. This is useful for doing specific
    /// checks at certain points, e.g. on `Connect` you can check the IP against
    /// a block list and abort the connection due to suspected spam.
    ///
    /// # Examples
    ///
    /// ```
    /// use empath::smtp::server::Server;
    /// use empath::smtp::server::state::State;
    ///
    /// let server = Server::default()
    ///     .handle(State::Connect, |vctx| {
    ///         println!("Connected!");
    ///         Ok(())
    ///     });
    ///
    /// server.run(); // Whenever the server receives a connection it'll print `Connected!`
    ///
    /// ```
    ///
    /// # Panics
    ///
    /// Panics if the server is unable to obtain a write lock on the internal handles
    /// it has
    #[must_use]
    pub fn handle(self, command: Phase, handler: Handle) -> Server {
        self.handlers
            .write()
            .expect("Unable to add handler")
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
    /// use empath::smtp::server::Server;
    ///
    /// let server = Server::default().extension(STARTTLS);
    /// assert_eq!(server.extensions.read().unwrap(), vec![STARTTLS]);
    /// server.run(); // The server will now advertise that it supports the
    ///               // STARTTLS extension, and will also accept it as a
    ///               // command
    /// ```
    ///
    /// # Panics
    ///
    /// Panics if the server is unable to obtain a write lock on the internal
    /// extensions it has
    #[must_use]
    pub fn extension(self, extension: Extension) -> Server {
        self.extensions
            .write()
            .expect("Unable to add extension")
            .push(extension);

        self
    }

    async fn connect(
        mut self,
        queue: Arc<AtomicU64>,
        stream: Async<TcpStream>,
        peer: SocketAddr,
    ) -> std::io::Result<()> {
        let mut connection = Connection {
            stream,
            tls: None,
            peer,
        };
        let id = connection.peer.to_string();
        let logger = Logger::with_id(&id);
        let mut vctx = ValidationContext::default();

        logger.internal(&format!("Connected to {}", peer));

        loop {
            match self.response(&queue, &mut vctx) {
                Ok((response, ev)) => {
                    self.context.sent = true;

                    for response in response.unwrap_or_default() {
                        logger.outgoing(&response);

                        connection.send(&response).await.map_err(|err| {
                            logger.internal(&format!("Error: {err}"));
                            io::Error::new(io::ErrorKind::ConnectionAborted, err.to_string())
                        })?;
                    }

                    if Event::ConnectionClose == ev {
                        return Ok(());
                    }
                }
                Err(SMTPError { status, message }) => {
                    let response = format!("{status} {message}");
                    logger.outgoing(&response);
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
                    self.receive(&mut connection, &logger, &mut vctx).await,
                    Ok(true) | Err(_)
                );

                if connection_closed {
                    logger.internal("Connection closed");
                    connection.stream.flush().await?;
                    return Ok(());
                }
            }
        }
    }

    /// Tell the server to listen on a specific port
    ///
    /// # Examples
    ///
    /// ```
    /// use empath::smtp::server::Server;
    ///
    /// let server = Server::default().on_port(1026);
    /// assert_eq!(server.port, 1026);
    /// ```
    #[must_use]
    pub fn on_port(mut self, port: u16) -> Server {
        self.port = port;

        self
    }

    /// Run the server, which will accept connections on the
    /// port it is asked to (or the default if not chosen).
    ///
    /// # Examples
    ///
    /// ```
    /// use empath::smtp::server::Server;
    ///
    /// let server = Server::default();
    /// server.run();
    /// ```
    ///
    /// # Errors
    ///
    /// This function will return an error if there is an issue accepting a connection,
    /// or if there is an issue binding to the specific address and port combination.
    pub async fn run(self) -> std::io::Result<()> {
        Logger::init();

        let listener = Async::<TcpListener>::bind(SocketAddr::new(self.address, self.port))?;
        let queue = Arc::new(AtomicU64::default());

        loop {
            let (stream, address) = listener.accept().await?;

            smol::spawn(self.clone().connect(Arc::clone(&queue), stream, address)).detach();
        }
    }

    /// Generate the response(s) that should be sent back to the client
    /// depending on the servers state
    fn response(&mut self, queue: &Arc<AtomicU64>, vctx: &mut ValidationContext) -> Response {
        if self.context.sent {
            return Ok((None, Event::ConnectionKeepAlive));
        }

        if let Some(handlers) = self.handlers.read().unwrap().get(&self.context.state) {
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
                    if self.extensions.read().is_ok() {
                        '-'
                    } else {
                        ' '
                    },
                    std::str::from_utf8(&self.context.message).unwrap()
                )];

                if let Ok(extensions) = self.extensions.read() {
                    for (idx, extension) in extensions.iter().enumerate() {
                        response.push(format!(
                            "{}{}{}",
                            Status::Ok,
                            if idx == extensions.len() - 1 {
                                ' '
                            } else {
                                '-'
                            },
                            extension
                        ));
                    }
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
        logger: &Logger<'_>,
        vctx: &mut ValidationContext,
    ) -> std::io::Result<bool> {
        let mut received_data = vec![0; 4096];

        match connection.receive(&mut received_data).await {
            // Consider any errors received here to be fatal
            Err(err) => {
                logger.internal(&format!("Error: {err}"));
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

                    logger.incoming(&command.to_string());

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

// async fn forward(vctx: &ValidationContext) -> std::io::Result<()> {
//     println!("{vctx:#?}");

//     let from = vctx.mail_from.as_ref().unwrap().split(':').nth(1).unwrap();
//     let to = vctx
//         .rcpt_to
//         .as_ref()
//         .unwrap()
//         .iter()
//         .map(|to| to.split(':').nth(1).unwrap())
//         .collect::<Vec<_>>();

//     let from = if let MailAddr::Single(SingleInfo { addr, .. }) =
//         mailparse::addrparse(from).unwrap().first().unwrap()
//     {
//         addr.clone()
//     } else {
//         String::default()
//     };
//     let to = mailparse::addrparse(to.join(",").as_str()).unwrap();

//     println!("{from:#?} --> {to:#?}");

//     let resolver = Resolver::new(ResolverConfig::default(), ResolverOpts::default()).unwrap();

//     if let MailAddr::Single(SingleInfo { addr, .. }) = to.first().unwrap() {
//         let response = resolver.mx_lookup(addr.split('@').nth(1).unwrap()).unwrap();
//         let response = response.iter().next().unwrap();

//         println!("{}", response.exchange());

//         let response = resolver.lookup_ip(response.exchange().to_string()).unwrap();

//         let address = response.iter().next().expect("no addresses returned!");

//         println!("{address}");

//         let conn = Async::<TcpStream>::connect((address, 25)).await?;

//         println!("{conn:#?}");

//         let mut buffer = [0; 4096];

//         conn.read_with(|mut conn| conn.read(&mut buffer)).await?;
//         println!("RESPONSE: {}", std::str::from_utf8(&buffer).unwrap());

//         let mut buffer = [0; 4096];
//         conn.write_with(|mut conn| write!(conn, "EHLO test-local\r\n"))
//             .await?;
//         conn.read_with(|mut conn| conn.read(&mut buffer)).await?;
//         println!("RESPONSE: {}", std::str::from_utf8(&buffer).unwrap());

//         let mut buffer = [0; 4096];
//         conn.write_with(|mut conn| write!(conn, "MAIL FROM:<{from}>\r\n"))
//             .await?;
//         conn.read_with(|mut conn| conn.read(&mut buffer)).await?;
//         println!("RESPONSE: {}", std::str::from_utf8(&buffer).unwrap());

//         let mut buffer = [0; 4096];
//         conn.write_with(|mut conn| write!(conn, "RCPT TO:<{to}>\r\n"))
//             .await?;
//         conn.read_with(|mut conn| conn.read(&mut buffer)).await?;
//         println!("RESPONSE: {}", std::str::from_utf8(&buffer).unwrap());

//         let mut buffer = [0; 4096];
//         conn.write_with(|mut conn| write!(conn, "DATA\r\n")).await?;
//         conn.read_with(|mut conn| conn.read(&mut buffer)).await?;
//         println!("RESPONSE: {}", std::str::from_utf8(&buffer).unwrap());

//         let mut buffer = [0; 4096];
//         conn.write_with(|mut conn| write!(conn, "{}\r\n", vctx.data.as_ref().unwrap()))
//             .await?;
//         conn.read_with(|mut conn| conn.read(&mut buffer)).await?;
//         println!("RESPONSE: {}", std::str::from_utf8(&buffer).unwrap());

//         conn.write_with(|mut conn| write!(conn, "QUIT\r\n")).await?;
//     }

//     Ok(())
// }
