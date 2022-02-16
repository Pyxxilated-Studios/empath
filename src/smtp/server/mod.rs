use std::collections::HashMap;
use std::fmt::Debug;
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::{atomic::AtomicU64, Arc, RwLock};

use smol::{io::AsyncWriteExt, Async};
// use trust_dns_resolver::{
//     config::{ResolverConfig, ResolverOpts},
//     Resolver,
// };

use crate::common::{command::Command, extensions::Extension, status::Status};
use crate::log::Logger;

pub mod state;
use state::State;

pub mod validation_context;
use validation_context::ValidationContext;

#[derive(Debug)]
pub struct Context {
    pub state: State,
    pub message: String,
    sent: bool,
}

impl Default for Context {
    fn default() -> Self {
        Context {
            state: State::Connect,
            message: String::default(),
            sent: false,
        }
    }
}

struct ReceivedMessage {
    connection_closed: bool,
}

pub(crate) type Handle = fn(&ValidationContext) -> Result<(), (Status, String)>;
pub(crate) type Handles = HashMap<State, Handle>;
type Response = Result<(Option<Vec<String>>, bool), (Status, String)>;

#[derive(Clone)]
pub struct Server {
    address: IpAddr,
    port: u16,
    handlers: Arc<RwLock<Handles>>,
    extensions: Arc<RwLock<Vec<Extension>>>,
}

impl Default for Server {
    fn default() -> Self {
        Server {
            address: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port: 1025,
            handlers: Arc::default(),
            extensions: Arc::default(),
        }
    }
}

impl Server {
    pub fn handle(self, command: State, handler: Handle) -> Server {
        self.handlers
            .write()
            .expect("Unable to add handler")
            .entry(command)
            .and_modify(|hdlr| *hdlr = handler)
            .or_insert(handler);
        self
    }

    pub fn extension(self, extension: Extension) -> Server {
        self.extensions
            .write()
            .expect("Unable to add extension")
            .push(extension);

        self
    }

    /// Returns `true` if the connection is done.
    async fn connect(
        self,
        queue: Arc<AtomicU64>,
        mut stream: Async<TcpStream>,
        mut context: Context,
        peer: SocketAddr,
    ) -> std::io::Result<bool> {
        let id = peer.to_string();
        let logger = Logger::with_id(&id);
        let mut vctx = ValidationContext::default();

        logger.internal("Connected");

        loop {
            stream.writable().await?;

            match self.send(&queue, &mut context, &mut vctx) {
                Ok((response, close)) => {
                    context.sent = true;

                    if let Some(response) = response {
                        stream
                            .write_with(|mut stream| {
                                for response in &response {
                                    logger.outgoing(response);
                                    if let Err(err) = write!(stream, "{}\r\n", response) {
                                        return Err(err);
                                    }
                                }

                                Ok(())
                            })
                            .await?;
                    }

                    if close {
                        return Ok(true);
                    }
                }
                Err((status, message)) => {
                    let response = format!("{status} {message}");
                    logger.outgoing(&response);
                    stream
                        .write_with(|mut stream| write!(stream, "{response}\r\n"))
                        .await?;
                }
            }

            stream.readable().await?;

            let connection_closed = self
                .receive(&mut stream, &mut context, &logger, &mut vctx)
                .await
                .map_or(true, |message| {
                    matches!(
                        message,
                        ReceivedMessage {
                            connection_closed: true,
                            ..
                        }
                    )
                });

            if connection_closed {
                logger.internal("Connection closed");
                stream.flush().await?;
                return Ok(true);
            }
        }
    }

    pub fn on_port(mut self, port: u16) -> Server {
        self.port = port;

        self
    }

    pub fn run(self) -> std::io::Result<()> {
        smol::block_on(async {
            Logger::init();

            let listener = Async::<TcpListener>::bind(SocketAddr::new(self.address, self.port))?;
            let queue = Arc::new(AtomicU64::default());

            loop {
                let (stream, address) = listener.accept().await?;

                smol::spawn(self.clone().connect(
                    Arc::clone(&queue),
                    stream,
                    Context::default(),
                    address,
                ))
                .detach();
            }
        })
    }

    fn send(
        &self,
        queue: &Arc<AtomicU64>,
        context: &mut Context,
        vctx: &mut ValidationContext,
    ) -> Response {
        if context.sent {
            return Ok((None, false));
        }

        if let Some(handler) = self.handlers.read().unwrap().get(&context.state) {
            handler(vctx)?;
        }

        let status = match context.state {
            State::Connect => (
                Some(vec![format!("{} localhost", Status::ServiceReady)]),
                false,
            ),
            State::Ehlo => {
                let mut response = vec![];

                if let Ok(extensions) = self.extensions.read() {
                    response.push(format!("{}-Hello {}", Status::Ok, context.message));
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
                } else {
                    response.push(format!("{} Hello {}", Status::Ok, context.message));
                }
                (Some(response), false)
            }
            State::StartTLS => (
                Some(vec![format!("{} Ready to begin TLS", Status::Ok)]),
                false,
            ),
            State::MailFrom | State::RcptTo => (Some(vec![format!("{} Ok", Status::Ok)]), false),
            State::Data => {
                context.state = State::Reading;
                (
                    Some(vec![format!(
                        "{} End data with <CR><LF>.<CR><LF>",
                        Status::StartMailInput
                    )]),
                    false,
                )
            }
            State::DataReceived => {
                let queue = queue.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                (
                    Some(vec![format!("{} Ok: queued as {}", Status::Ok, queue)]),
                    false,
                )
            }
            State::Quit => (Some(vec![format!("{} Bye", Status::GoodBye)]), true),
            State::Invalid => (
                Some(vec![format!(
                    "{} Invalid command '{}'",
                    Status::InvalidCommandSequence,
                    context.message
                )]),
                true,
            ),
            State::Reading | State::Close => (None, false),
            State::InvalidCommandSequence => (
                Some(vec![format!(
                    "{} {}",
                    Status::InvalidCommandSequence,
                    context.state
                )]),
                true,
            ),
        };

        Ok(status)
    }

    async fn receive(
        &self,
        stream: &mut Async<TcpStream>,
        context: &mut Context,
        logger: &Logger<'_>,
        vctx: &mut ValidationContext,
    ) -> std::io::Result<ReceivedMessage> {
        let mut received_data = vec![0; 4096];

        let bytes_read = match stream
            .read_with(|mut stream| stream.read(&mut received_data))
            .await
        {
            Ok(0) => {
                // Reading 0 bytes means the other side has closed the
                // connection or is done writing, then so are we.
                return Ok(ReceivedMessage {
                    connection_closed: true,
                });
            }
            Ok(n) => n,
            // Other errors we'll consider fatal.
            Err(err) => return Err(err),
        };

        if let Ok(received) = std::str::from_utf8(&received_data[..bytes_read]) {
            if context.state == State::Reading {
                context.message += received;

                return Ok(context
                    .message
                    .ends_with("\r\n.\r\n")
                    .then(|| {
                        *context = Context {
                            state: State::DataReceived,
                            message: context.message.clone(),
                            sent: false,
                        };

                        vctx.data = Some(context.message.clone());

                        ReceivedMessage {
                            connection_closed: false,
                        }
                    })
                    .unwrap_or(ReceivedMessage {
                        connection_closed: false,
                    }));
            }

            let command = received
                .trim()
                .parse::<Command>()
                .unwrap_or_else(|comm| comm);

            let message = command.inner();

            logger.incoming(&format!("{command}"));

            *context = Context {
                state: context.state.transition(command, vctx),
                message,
                sent: false,
            };
        } else {
            logger.internal(&format!("Received (non UTF-8) data: {:?}", received_data));
        }

        Ok(ReceivedMessage {
            connection_closed: false,
        })
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
