pub mod command;
pub mod connection;
pub mod context;
pub mod envelope;
pub mod extensions;
pub mod session;
pub mod status;

use core::fmt::{self, Display, Formatter};
use std::{collections::HashMap, net::SocketAddr, sync::Arc};

use serde::{Deserialize, Serialize};
use tokio::net::TcpStream;

use crate::{
    ffi::modules::{self, validate},
    traits::{
        fsm::FiniteStateMachine,
        protocol::{Protocol, SessionHandler},
    },
};

use self::{
    command::{Command, HeloVariant},
    context::Context,
    extensions::Extension,
    session::{Session, TlsContext},
};

#[derive(Default, Deserialize, Serialize)]
pub struct Smtp {
    state: State,
}

impl Protocol for Smtp {
    type Session = Session<TcpStream>;

    fn handle(
        &self,
        stream: TcpStream,
        peer: SocketAddr,
        extensions: &[Extension],
        tls_context: Option<TlsContext>,
        init_context: HashMap<String, String>,
    ) -> Self::Session {
        Session::create(
            Arc::default(),
            stream,
            peer,
            extensions.to_vec(),
            tls_context.unwrap_or_default(),
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
            (Self::Ehlo | Self::Helo | Self::StartTLS | Self::PostDot, Command::MailFrom(from)) => {
                modules::dispatch(
                    modules::Event::Validate(validate::Event::MailFrom),
                    validate_context,
                );
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
