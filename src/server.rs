use std::collections::HashMap;
use std::fmt::{Debug, Display, Formatter};
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::str::FromStr;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, RwLock};

use smol::Async;

use crate::log::Logger;

#[derive(Clone, Copy, PartialEq, PartialOrd)]
pub enum Status {
    ServiceReady = 220,
    GoodBye = 221,
    Ok = 250,
    StartMailInput = 354,
    Unavailable = 421,
    InvalidCommandSequence = 503,
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

pub struct ValidationContext {
    envelope: (),
}

pub(crate) type Handle = fn(&Context) -> std::io::Result<()>;
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
    async fn handle_connection_event(
        handlers: Arc<RwLock<Handles>>,
        queue: Arc<AtomicU64>,
        mut stream: Async<TcpStream>,
        mut context: Context,
        peer: SocketAddr,
    ) -> std::io::Result<bool> {
        let mut connection_closed = false;

        let id = peer.to_string();
        let logger = Logger::with_id(&id);

        logger.internal("Connected");

        loop {
            if let Ok((response, close)) = response(&queue, &mut context, &handlers) {
                context.sent = true;

                if let Some(response) = response {
                    logger.outgoing(&response);
                    stream
                        .write_with(|mut stream| write!(stream, "{}\r\n", response))
                        .await?;
                }

                if close {
                    // connection_closed = true;

                    return Ok(true);
                }
            }

            stream.readable().await?;

            if let Ok(message) = receive(&mut stream, &mut context).await {
                match message {
                    ReceivedMessage {
                        reading_done: true,
                        connection_closed: closed,
                    } => {
                        if context.state != State::DataReceived {
                            logger
                                .incoming(format!("{} {}", context.state, context.message).trim());
                        }
                        if closed {
                            connection_closed = true;
                        }
                    }
                    ReceivedMessage {
                        connection_closed: true,
                        ..
                    } => connection_closed = true,
                    _ => {}
                }
            } else {
                connection_closed = true;
            }

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

                smol::spawn(Server::handle_connection_event(
                    Arc::clone(&self.handlers),
                    Arc::clone(&queue),
                    stream,
                    Context {
                        state: State::Connect,
                        message: String::new(),
                        sent: false,
                    },
                    address,
                ))
                .detach();
            }
        })
    }
}

fn response(
    queue: &Arc<AtomicU64>,
    context: &mut Context,
    handlers: &Arc<RwLock<Handles>>,
) -> std::io::Result<(Option<String>, bool)> {
    if context.sent {
        return Ok((None, false));
    }

    if let Some(handler) = handlers.read().unwrap().get(&context.state) {
        handler(context)?;
    }

    Ok(match context.state {
        State::Connect => (Some(format!("{} localhost", Status::ServiceReady)), false),
        State::Ehlo => (
            Some(format!("{} Hello {}", Status::Ok, context.message)),
            false,
        ),
        State::MailFrom | State::RcptTo => (Some(format!("{} Ok", Status::Ok)), false),
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
    })
}

struct ReceivedMessage {
    connection_closed: bool,
    reading_done: bool,
}

async fn receive(
    stream: &mut Async<TcpStream>,
    context: &mut Context,
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
                reading_done: true,
            });
        }
        Ok(n) => n,
        // Other errors we'll consider fatal.
        Err(err) => return Err(err),
    };

    if let Ok(receieved) = std::str::from_utf8(&received_data[..bytes_read]) {
        if context.state == State::Reading {
            if receieved.ends_with("\r\n.\r\n") {
                *context = Context {
                    state: State::DataReceived,
                    message: format!("{}{}", context.message, receieved),
                    sent: false,
                };

                return Ok(ReceivedMessage {
                    connection_closed: false,
                    reading_done: true,
                });
            }

            context.message += receieved;
            return Ok(ReceivedMessage {
                connection_closed: false,
                reading_done: false,
            });
        }

        let mut mess = receieved.split(' ');
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

    Ok(ReceivedMessage {
        connection_closed: false,
        reading_done: true,
    })
}
