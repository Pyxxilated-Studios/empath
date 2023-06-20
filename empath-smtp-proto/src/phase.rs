use std::{
    borrow::BorrowMut,
    fmt::{Display, Formatter},
    str::FromStr,
};

use crate::command::{Command, HeloVariant};
use empath_common::context::Context;
use serde::{Deserialize, Serialize};

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
    pub fn transition(self, command: Command, vctx: &mut Context) -> Self {
        match (self, command) {
            (Self::Connect, Command::Helo(HeloVariant::Ehlo(id))) => {
                vctx.id = id;
                Self::Ehlo
            }
            (Self::Connect, Command::Helo(HeloVariant::Helo(id))) => {
                vctx.id = id;
                Self::Helo
            }
            (Self::Ehlo | Self::Helo, Command::StartTLS) => Self::StartTLS,
            (Self::Ehlo | Self::Helo | Self::StartTLS, Command::MailFrom(from)) => {
                vctx.mail_from = from;
                Self::MailFrom
            }
            (Self::RcptTo | Self::MailFrom, Command::RcptTo(to)) => {
                if let Some(rcpts) = vctx.rcpt_to.borrow_mut() {
                    rcpts.extend_from_slice(&to[..]);
                } else {
                    vctx.rcpt_to = Some(to);
                }
                Self::RcptTo
            }
            (Self::RcptTo, Command::Data) => Self::Data,
            (Self::Data, comm) if comm != Command::Quit => Self::Connect,
            (_, Command::Quit) => Self::Quit,
            _ => Self::InvalidCommandSequence,
        }
    }
}
