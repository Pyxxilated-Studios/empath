use std::collections::HashMap;
use std::fmt::{Debug, Display, Formatter};
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::str::FromStr;

use mio::event::Event;
use mio::net::{TcpListener, TcpStream};
use mio::{Events, Interest, Poll, Token};

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

#[derive(PartialEq, PartialOrd)]
pub enum State {
    CONNECTED,
    HELLO,
    FROM,
    RCPTTO,
    QUIT,
}

#[derive(PartialEq, PartialOrd, Eq, Hash, Debug)]
pub enum Command {
    Connect,
    Ehlo,
    MailFrom,
    RcptTo,
    Data,
    Reading,
    DataReceived,
    Quit,
    Invalid,
}

impl Display for Command {
    fn fmt(&self, fmt: &mut Formatter<'_>) -> Result<(), std::fmt::Error> {
        fmt.write_str(match self {
            Command::Reading | Command::DataReceived => "",
            Command::Connect => "Connect",
            Command::Ehlo => "EHLO",
            Command::MailFrom => "MAIL",
            Command::RcptTo => "RCPT",
            Command::Data => "DATA",
            Command::Quit => "QUIT",
            Command::Invalid => "INVALID",
        })
    }
}

impl FromStr for Command {
    type Err = Self;

    fn from_str(command: &str) -> Result<Self, <Self as FromStr>::Err> {
        match command.to_ascii_uppercase().trim() {
            "EHLO" | "HELO" => Ok(Command::Ehlo),
            "MAIL" => Ok(Command::MailFrom),
            "RCPT" => Ok(Command::RcptTo),
            "DATA" => Ok(Command::Data),
            "QUIT" => Ok(Command::Quit),
            _ => Err(Command::Invalid),
        }
    }
}

#[derive(Debug)]
pub struct Context {
    pub command: Command,
    pub message: String,
    sent: bool,
}

pub(crate) type Handle = fn(&Context) -> std::io::Result<()>;
pub(crate) type Handles = HashMap<Command, Handle>;

pub struct Server {
    connections: HashMap<Token, (TcpStream, Context)>,
    address: IpAddr,
    port: u16,
    handlers: Handles,
    logger: Logger,
}

impl Default for Server {
    fn default() -> Self {
        Server {
            connections: HashMap::new(),
            address: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port: 1025,
            handlers: HashMap::new(),
            logger: Logger::default(),
        }
    }
}

struct Connection<'a> {
    token: &'a mut Token,
    event: &'a Event,
    poll: &'a mut Poll,
}

const SERVER: Token = Token(0);

impl Server {
    pub fn handle(mut self, command: Command, handler: Handle) -> Server {
        self.handlers
            .entry(command)
            .and_modify(|hdlr| *hdlr = handler)
            .or_insert(handler);
        self
    }

    pub fn listen(mut self, port: u16) -> Server {
        self.port = port;

        self
    }

    pub fn run(mut self) -> std::io::Result<()> {
        // Create storage for events.
        let mut events = Events::with_capacity(128);
        let mut poll = Poll::new()?;

        let mut listener = TcpListener::bind(SocketAddr::new(self.address, self.port))?;

        poll.registry().register(
            &mut listener,
            SERVER,
            Interest::READABLE | Interest::WRITABLE,
        )?;

        let mut unique_token = Token(SERVER.0 + 1);

        // Start an event loop.
        loop {
            // Poll Mio for events, blocking until we get an event.
            poll.poll(&mut events, None)?;

            // Process each event.
            for event in events.iter() {
                // We can use the token we previously provided to `register` to
                // determine for which socket the event is.
                match event.token() {
                    SERVER => {
                        let (mut connection, address) = match listener.accept() {
                            Ok((connection, address)) => (connection, address),
                            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                                // If we get a `WouldBlock` error we know our
                                // listener has no more incoming connections queued,
                                // so we can return to polling and wait for some
                                // more.
                                break;
                            }
                            Err(e) => {
                                // If it was any other kind of error, something went
                                // wrong and we terminate with an error.
                                return Err(e);
                            }
                        };

                        self.logger
                            .incoming(&format!("Connection from {}", address));

                        let token = next(&mut unique_token);
                        poll.registry().register(
                            &mut connection,
                            token,
                            Interest::READABLE | Interest::WRITABLE,
                        )?;

                        write!(connection, "{} localhost\r\n", Status::ServiceReady)?;

                        self.connections.insert(
                            token,
                            (
                                connection,
                                Context {
                                    command: Command::Connect,
                                    message: String::new(),
                                    sent: true,
                                },
                            ),
                        );
                    }
                    mut token => {
                        let done = self
                            .handle_connection_event(&Connection {
                                token: &mut token,
                                event,
                                poll: &mut poll,
                            })
                            .unwrap_or(false);

                        if done {
                            if let Some(mut connection) = self.connections.remove(&token) {
                                poll.registry().deregister(&mut connection.0)?;
                            }

                            self.logger.outgoing("=== Connection closed");
                        }
                    }
                }
            }
        }
    }

    /// Returns `true` if the connection is done.
    fn handle_connection_event(&mut self, connection: &Connection) -> std::io::Result<bool> {
        if let Some((stream, context)) = self.connections.get_mut(connection.token) {
            let mut connection_closed = false;

            if connection.event.is_readable() {
                if receive(stream, context)? {
                    connection_closed = true;
                }

                self.logger
                    .incoming(&format!("{} {}", context.command, context.message));
            }

            if connection.event.is_writable() && !context.sent {
                let (response, close) = response(context, &self.handlers)?;

                if let Some(response) = response {
                    self.logger.outgoing(&response);
                    write!(stream, "{}\r\n", response)?;
                }

                if close {
                    connection_closed = close;
                }

                context.sent = true;
            }

            if connection_closed {
                return Ok(true);
            }

            connection.poll.registry().reregister(
                stream,
                connection.event.token(),
                Interest::WRITABLE | Interest::READABLE,
            )?;
        }

        Ok(false)
    }
}

fn next(current: &mut Token) -> Token {
    let next = current.0;
    current.0 += 1;
    Token(next)
}

fn would_block(err: &std::io::Error) -> bool {
    err.kind() == std::io::ErrorKind::WouldBlock
}

fn interrupted(err: &std::io::Error) -> bool {
    err.kind() == std::io::ErrorKind::Interrupted
}

fn response(context: &mut Context, handlers: &Handles) -> std::io::Result<(Option<String>, bool)> {
    if let Some(handler) = handlers.get(&context.command) {
        handler(context)?;
    }

    Ok(match context.command {
        Command::Connect | Command::Reading => (None, false),
        Command::Ehlo => (
            Some(format!("{} Hello {}", Status::Ok, context.message)),
            false,
        ),
        Command::MailFrom | Command::RcptTo => (Some(format!("{} Ok", Status::Ok)), false),
        Command::Data => {
            context.command = Command::Reading;
            (
                Some(format!(
                    "{} End data with <CR><LF>.<CR><LF>",
                    Status::StartMailInput
                )),
                false,
            )
        }
        Command::DataReceived => (Some(format!("{} Ok: queued as 123", Status::Ok)), false),
        Command::Quit => (Some(format!("{} Bye", Status::GoodBye)), true),
        Command::Invalid => (
            Some(format!(
                "{} Invalid command '{}'",
                Status::InvalidCommandSequence,
                context.message
            )),
            true,
        ),
    })
}

fn receive(stream: &mut TcpStream, context: &mut Context) -> std::io::Result<bool> {
    let mut received_data = vec![0; 4096];
    let mut bytes_read = 0;

    loop {
        match stream.read(&mut received_data[bytes_read..]) {
            Ok(0) => {
                // Reading 0 bytes means the other side has closed the
                // connection or is done writing, then so are we.
                return Ok(true);
            }
            Ok(n) => {
                bytes_read += n;
                if bytes_read == received_data.len() {
                    received_data.resize(received_data.len() + 1024, 0);
                }
            }
            // Would block "errors" are the OS's way of saying that the
            // connection is not actually ready to perform this I/O operation.
            Err(ref err) if would_block(err) => break,
            Err(ref err) if interrupted(err) => continue,
            // Other errors we'll consider fatal.
            Err(err) => return Err(err),
        }
    }

    if bytes_read != 0 {
        let received_data = &received_data[..bytes_read];
        if let Ok(receieved) = std::str::from_utf8(received_data) {
            let mut mess = receieved.split(' ');
            let command = mess.next().unwrap_or("");
            let command = command.trim();
            let data = mess.collect::<Vec<_>>().join(" ");

            if context.command == Command::Reading {
                let message = format!("{}{}", context.message, receieved);

                if message.contains("\r\n.\r\n") {
                    *context = Context {
                        command: Command::DataReceived,
                        message,
                        sent: false,
                    };
                } else {
                    context.message = message;
                }
            } else {
                let mut message = String::from(data.trim());
                let command = command.parse::<Command>().unwrap_or_else(|comm| {
                    message = format!("{} {}", command, message);
                    comm
                });

                *context = Context {
                    command,
                    message,
                    sent: false,
                };
            }
        } else {
            println!("Received (non UTF-8) data: {:?}", received_data);
        }
    }

    Ok(false)
}
