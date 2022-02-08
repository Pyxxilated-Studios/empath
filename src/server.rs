use std::collections::HashMap;
use std::fmt::{Debug, Display, Formatter};
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::str::FromStr;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, RwLock};

use mailparse::{MailAddr, SingleInfo};
use smol::Async;
use trust_dns_resolver::config::{ResolverConfig, ResolverOpts};
use trust_dns_resolver::Resolver;

use crate::log::Logger;

#[derive(Clone, Copy, PartialEq, PartialOrd)]
pub enum Status {
    ServiceReady = 220,
    GoodBye = 221,
    Ok = 250,
    StartMailInput = 354,
    Unavailable = 421,
    InvalidCommandSequence = 503,
    Error = 550,
}

impl Display for Status {
    fn fmt(&self, fmt: &mut Formatter<'_>) -> Result<(), std::fmt::Error> {
        fmt.write_fmt(format_args!("{}", *self as i32))
    }
}

#[derive(PartialEq, PartialOrd, Eq, Hash, Debug)]
pub enum State {
    Connect,
    Ehlo,
    MailFrom,
    RcptTo,
    Data,
    Reading,
    DataReceived,
    Quit,
    Invalid,
    Close,
}

impl Display for State {
    fn fmt(&self, fmt: &mut Formatter<'_>) -> Result<(), std::fmt::Error> {
        fmt.write_str(match self {
            State::Reading | State::DataReceived => "",
            State::Connect => "Connect",
            State::Close => "Close",
            State::Ehlo => "EHLO",
            State::MailFrom => "MAIL",
            State::RcptTo => "RCPT",
            State::Data => "DATA",
            State::Quit => "QUIT",
            State::Invalid => "INVALID",
        })
    }
}

impl FromStr for State {
    type Err = Self;

    fn from_str(command: &str) -> Result<Self, <Self as FromStr>::Err> {
        match command.to_ascii_uppercase().trim() {
            "EHLO" | "HELO" => Ok(State::Ehlo),
            "MAIL" => Ok(State::MailFrom),
            "RCPT" => Ok(State::RcptTo),
            "DATA" => Ok(State::Data),
            "QUIT" => Ok(State::Quit),
            _ => Err(State::Invalid),
        }
    }
}

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

#[derive(Default, Debug)]
pub struct ValidationContext {
    mail_from: Option<String>,
    rcpt_to: Option<Vec<String>>,
    data: Option<String>,
}

pub(crate) type Handle = fn(&Context) -> Result<(), (Status, String)>;
pub(crate) type Handles = HashMap<State, Handle>;

pub struct Server {
    address: IpAddr,
    port: u16,
    handlers: Arc<RwLock<Handles>>,
}

impl Default for Server {
    fn default() -> Self {
        Server {
            address: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port: 1025,
            handlers: Arc::default(),
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

    /// Returns `true` if the connection is done.
    async fn connect(
        handlers: Arc<RwLock<Handles>>,
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

            match send(&queue, &mut context, &handlers, &mut vctx) {
                Ok((response, close)) => {
                    context.sent = true;

                    if let Some(response) = response {
                        logger.outgoing(&response);
                        stream
                            .write_with(|mut stream| write!(stream, "{}\r\n", response))
                            .await?;
                    }

                    if close {
                        return Ok(true);
                    }

                    if context.state == State::DataReceived {
                        forward(&vctx).await?;
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

            let connection_closed =
                receive(&mut stream, &mut context, &logger)
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
                return Ok(true);
            }
        }
    }

    pub fn listen(mut self, port: u16) -> Server {
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

                smol::spawn(Server::connect(
                    Arc::clone(&self.handlers),
                    Arc::clone(&queue),
                    stream,
                    Context::default(),
                    address,
                ))
                .detach();
            }
        })
    }
}

fn send(
    queue: &Arc<AtomicU64>,
    context: &mut Context,
    handlers: &Arc<RwLock<Handles>>,
    vctx: &mut ValidationContext,
) -> Result<(Option<String>, bool), (Status, String)> {
    if context.sent {
        return Ok((None, false));
    }

    handlers
        .read()
        .unwrap()
        .get(&context.state)
        .map_or_else(|| Ok(()), |handler| handler(context))?;

    let status = match context.state {
        State::Connect => (Some(format!("{} localhost", Status::ServiceReady)), false),
        State::Ehlo => (
            Some(format!("{} Hello {}", Status::Ok, context.message)),
            false,
        ),
        State::MailFrom => {
            vctx.mail_from = Some(context.message.clone());

            (Some(format!("{} Ok", Status::Ok)), false)
        }
        State::RcptTo => {
            if vctx.rcpt_to.is_some() {
                vctx.rcpt_to.as_mut().unwrap().push(context.message.clone());
            } else {
                vctx.rcpt_to = Some(vec![context.message.clone()]);
            }

            (Some(format!("{} Ok", Status::Ok)), false)
        }
        State::Data => {
            context.state = State::Reading;
            (
                Some(format!(
                    "{} End data with <CR><LF>.<CR><LF>",
                    Status::StartMailInput
                )),
                false,
            )
        }
        State::DataReceived => {
            vctx.data = Some(context.message.clone());

            let queue = queue.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            (
                Some(format!("{} Ok: queued as {}", Status::Ok, queue)),
                false,
            )
        }
        State::Quit => (Some(format!("{} Bye", Status::GoodBye)), true),
        State::Invalid => (
            Some(format!(
                "{} Invalid command '{}'",
                Status::InvalidCommandSequence,
                context.message
            )),
            false,
        ),
        State::Reading | State::Close => (None, false),
    };

    Ok(status)
}

struct ReceivedMessage {
    connection_closed: bool,
}

async fn receive<'a>(
    stream: &mut Async<TcpStream>,
    context: &mut Context,
    logger: &Logger<'a>,
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

                    ReceivedMessage {
                        connection_closed: false,
                    }
                })
                .unwrap_or(ReceivedMessage {
                    connection_closed: false,
                }));
        }

        let mut mess = received.split(' ');
        let command = mess.next().unwrap_or("");
        let command = command.trim();
        let data = mess.collect::<Vec<_>>().join(" ");

        let mut message = String::from(data.trim());
        let command = command.parse::<State>().unwrap_or_else(|comm| {
            message = format!("{} {}", command, message);
            comm
        });

        *context = Context {
            state: command,
            message,
            sent: false,
        };
    } else {
        println!("Received (non UTF-8) data: {:?}", received_data);
    }

    logger.incoming(format!("{} {}", context.state, context.message).trim());

    Ok(ReceivedMessage {
        connection_closed: false,
    })
}

async fn forward(vctx: &ValidationContext) -> std::io::Result<()> {
    println!("{vctx:#?}");

    let from = vctx.mail_from.as_ref().unwrap().split(':').nth(1).unwrap();
    let to = vctx
        .rcpt_to
        .as_ref()
        .unwrap()
        .iter()
        .map(|to| to.split(':').nth(1).unwrap())
        .collect::<Vec<_>>();

    let from = if let MailAddr::Single(SingleInfo { addr, .. }) =
        mailparse::addrparse(from).unwrap().first().unwrap()
    {
        addr.clone()
    } else {
        String::default()
    };
    let to = mailparse::addrparse(to.join(",").as_str()).unwrap();

    println!("{from:#?} --> {to:#?}");

    let resolver = Resolver::new(ResolverConfig::default(), ResolverOpts::default()).unwrap();

    if let MailAddr::Single(SingleInfo { addr, .. }) = to.first().unwrap() {
        let response = resolver.mx_lookup(addr.split('@').nth(1).unwrap()).unwrap();
        let response = response.iter().next().unwrap();

        println!("{}", response.exchange());

        let response = resolver.lookup_ip(response.exchange().to_string()).unwrap();

        let address = response.iter().next().expect("no addresses returned!");

        println!("{address}");

        let conn = Async::<TcpStream>::connect((address, 25)).await?;

        println!("{conn:#?}");

        let mut buffer = [0; 4096];

        conn.read_with(|mut conn| conn.read(&mut buffer)).await?;
        println!("RESPONSE: {}", std::str::from_utf8(&buffer).unwrap());

        let mut buffer = [0; 4096];
        conn.write_with(|mut conn| write!(conn, "EHLO test-local\r\n"))
            .await?;
        conn.read_with(|mut conn| conn.read(&mut buffer)).await?;
        println!("RESPONSE: {}", std::str::from_utf8(&buffer).unwrap());

        let mut buffer = [0; 4096];
        conn.write_with(|mut conn| write!(conn, "MAIL FROM:<{from}>\r\n"))
            .await?;
        conn.read_with(|mut conn| conn.read(&mut buffer)).await?;
        println!("RESPONSE: {}", std::str::from_utf8(&buffer).unwrap());

        let mut buffer = [0; 4096];
        conn.write_with(|mut conn| write!(conn, "RCPT TO:<{to}>\r\n"))
            .await?;
        conn.read_with(|mut conn| conn.read(&mut buffer)).await?;
        println!("RESPONSE: {}", std::str::from_utf8(&buffer).unwrap());

        let mut buffer = [0; 4096];
        conn.write_with(|mut conn| write!(conn, "DATA\r\n")).await?;
        conn.read_with(|mut conn| conn.read(&mut buffer)).await?;
        println!("RESPONSE: {}", std::str::from_utf8(&buffer).unwrap());

        let mut buffer = [0; 4096];
        conn.write_with(|mut conn| write!(conn, "{}\r\n", vctx.data.as_ref().unwrap()))
            .await?;
        conn.read_with(|mut conn| conn.read(&mut buffer)).await?;
        println!("RESPONSE: {}", std::str::from_utf8(&buffer).unwrap());

        conn.write_with(|mut conn| write!(conn, "QUIT\r\n")).await?;
    }

    Ok(())
}
