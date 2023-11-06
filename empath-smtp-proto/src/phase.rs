use std::{
    borrow::BorrowMut,
    fmt::{Display, Formatter},
    str::FromStr,
};

use serde::{Deserialize, Serialize};

use empath_common::context::Context;
use empath_common::ffi::module;

use crate::command::{Command, HeloVariant};

#[repr(C)]
#[derive(PartialEq, PartialOrd, Eq, Hash, Debug, Clone, Copy, Serialize, Deserialize)]
pub enum Phase {
    Connect,
    Ehlo,
    Helo,
    StartTLS,
    MailFrom,
    RcptTo,
    Data,
    Reading,
    DataReceived,
    Quit,
    InvalidCommandSequence,
    Invalid,
    Reject,
    Close,
}

impl Display for Phase {
    fn fmt(&self, fmt: &mut Formatter<'_>) -> Result<(), std::fmt::Error> {
        fmt.write_str(match self {
            Self::Reading | Self::DataReceived => "",
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

impl FromStr for Phase {
    type Err = Self;

    fn from_str(command: &str) -> Result<Self, <Self as FromStr>::Err> {
        match command.to_ascii_uppercase().trim() {
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

impl Phase {
    #[must_use]
    pub fn transition(self, command: Command, validate_context: &mut Context) -> Self {
        match (self, command) {
            (Self::Connect, Command::Helo(HeloVariant::Ehlo(id))) => {
                validate_context.id = id;
                Self::Ehlo
            }
            (Self::Connect, Command::Helo(HeloVariant::Helo(id))) => {
                validate_context.id = id;
                Self::Helo
            }
            (Self::Ehlo | Self::Helo, Command::StartTLS) => Self::StartTLS,
            (Self::Ehlo | Self::Helo | Self::StartTLS, Command::MailFrom(from)) => {
                module::dispatch(
                    module::Event::Validate(module::ValidateEvent::MailFrom),
                    validate_context,
                );
                validate_context.mail_from = from;
                Self::MailFrom
            }
            (Self::RcptTo | Self::MailFrom, Command::RcptTo(to)) => {
                if let Some(rcpts) = validate_context.rcpt_to.borrow_mut() {
                    rcpts.extend_from_slice(&to[..]);
                } else {
                    validate_context.rcpt_to = Some(to);
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
