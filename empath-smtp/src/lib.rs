#![feature(let_chains)]

pub mod command;
pub mod connection;
pub mod extensions;
pub mod listener;
pub mod server;
pub mod session;

use core::fmt::{self, Display, Formatter};
use std::{collections::HashMap, net::SocketAddr, sync::Arc};

use empath_tracing::traced;
use extensions::Extension;
use serde::{Deserialize, Serialize};
use tokio::net::TcpStream;

use empath_common::{
    context::Context,
    traits::{
        fsm::FiniteStateMachine,
        protocol::{Protocol, SessionHandler},
    },
};

use self::{
    command::{Command, HeloVariant},
    session::{Session, TlsContext},
};

#[derive(Default, Deserialize, Serialize)]
pub struct Smtp {
    state: State,
}

impl Protocol for Smtp {
    type Session = Session<TcpStream>;
    type ExtraArgs = (Vec<Extension>, Option<TlsContext>);

    #[traced(instrument(level = tracing::Level::TRACE, skip(self, stream, init_context, args)), timing(precision = "ns"))]
    fn handle(
        &self,
        stream: TcpStream,
        peer: SocketAddr,
        init_context: HashMap<String, String>,
        args: Self::ExtraArgs,
    ) -> Self::Session {
        Session::create(
            Arc::default(),
            stream,
            peer,
            args.0,
            args.1,
            String::default(),
            init_context,
        )
    }
}

impl SessionHandler for Session<TcpStream> {
    async fn run(self) -> anyhow::Result<()> {
        Self::run(self).await
    }
}

#[repr(C)]
#[derive(PartialEq, PartialOrd, Eq, Hash, Debug, Clone, Copy, Serialize, Deserialize, Default)]
pub enum State {
    #[default]
    Connect,
    Ehlo,
    Helo,
    Help,
    StartTLS,
    MailFrom,
    RcptTo,
    Data,
    Reading,
    PostDot,
    Quit,
    InvalidCommandSequence,
    Invalid,
    Reject,
    Close,
}

impl Display for State {
    fn fmt(&self, fmt: &mut Formatter<'_>) -> Result<(), fmt::Error> {
        fmt.write_str(match self {
            Self::Reading | Self::PostDot => "",
            Self::Connect => "Connect",
            Self::Close => "Close",
            Self::Ehlo => "EHLO",
            Self::Helo => "HELO",
            Self::Help => "HELP",
            Self::StartTLS => "STARTTLS",
            Self::MailFrom => "MAIL",
            Self::RcptTo => "RCPT",
            Self::Data => "DATA",
            Self::Quit => "QUIT",
            Self::Invalid => "INVALID",
            Self::InvalidCommandSequence => "Invalid Command Sequence",
            Self::Reject => "Rejected",
        })
    }
}

impl TryFrom<&str> for State {
    type Error = Self;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value.to_ascii_uppercase().trim() {
            "EHLO" => Ok(Self::Ehlo),
            "HELO" => Ok(Self::Helo),
            "HELP" => Ok(Self::Help),
            "STARTTLS" => Ok(Self::StartTLS),
            "MAIL" => Ok(Self::MailFrom),
            "RCPT" => Ok(Self::RcptTo),
            "DATA" => Ok(Self::Data),
            "QUIT" => Ok(Self::Quit),
            _ => Err(Self::Invalid),
        }
    }
}

impl FiniteStateMachine for State {
    type Input = Command;
    type Context = Context;

    fn transition(self, input: Self::Input, validate_context: &mut Self::Context) -> Self {
        match (self, input) {
            (Self::Connect, Command::Helo(HeloVariant::Ehlo(id))) => {
                validate_context.id = id;
                validate_context.extended = true;
                Self::Ehlo
            }
            (Self::Connect, Command::Helo(HeloVariant::Helo(id))) => {
                validate_context.id = id;
                Self::Helo
            }
            (Self::Ehlo | Self::Helo, Command::StartTLS) if validate_context.extended => {
                Self::StartTLS
            }
            (Self::Ehlo | Self::Helo, Command::Help) => Self::Help,
            (
                Self::Ehlo | Self::Helo | Self::StartTLS | Self::PostDot | Self::Help,
                Command::MailFrom(from),
            ) => {
                *validate_context.envelope.sender_mut() = from;
                Self::MailFrom
            }
            (Self::RcptTo | Self::MailFrom, Command::RcptTo(to)) => {
                if let Some(rcpts) = validate_context.envelope.recipients_mut() {
                    rcpts.extend_from_slice(&to[..]);
                } else {
                    *validate_context.envelope.recipients_mut() = Some(to);
                }
                Self::RcptTo
            }
            (Self::RcptTo, Command::Data) => Self::Data,
            (Self::Data, comm) if comm != Command::Quit => Self::Connect,
            (_, Command::Quit) => Self::Quit,
            (Self::Invalid, _) => Self::Invalid,
            _ => Self::InvalidCommandSequence,
        }
    }
}
