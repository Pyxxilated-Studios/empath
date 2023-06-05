use std::{
    borrow::BorrowMut,
    fmt::{Display, Formatter},
    str::FromStr,
};

use crate::command::{Command, HeloVariant};
use empath_common::context::ValidationContext;
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
            Phase::Reading | Phase::DataReceived => "",
            Phase::Connect => "Connect",
            Phase::Close => "Close",
            Phase::Ehlo => "EHLO",
            Phase::Helo => "HELO",
            Phase::StartTLS => "STARTTLS",
            Phase::MailFrom => "MAIL",
            Phase::RcptTo => "RCPT",
            Phase::Data => "DATA",
            Phase::Quit => "QUIT",
            Phase::Invalid => "INVALID",
            Phase::InvalidCommandSequence => "Invalid Command Sequence",
        })
    }
}

impl FromStr for Phase {
    type Err = Self;

    fn from_str(command: &str) -> Result<Self, <Self as FromStr>::Err> {
        match command.to_ascii_uppercase().trim() {
            "EHLO" => Ok(Phase::Ehlo),
            "HELO" => Ok(Phase::Helo),
            "STARTTLS" => Ok(Phase::StartTLS),
            "MAIL" => Ok(Phase::MailFrom),
            "RCPT" => Ok(Phase::RcptTo),
            "DATA" => Ok(Phase::Data),
            "QUIT" => Ok(Phase::Quit),
            _ => Err(Phase::Invalid),
        }
    }
}

impl Phase {
    pub fn transition(self, command: Command, vctx: &mut ValidationContext) -> Phase {
        match (self, command) {
            (Phase::Connect, Command::Helo(HeloVariant::Ehlo(id))) => {
                vctx.id = id;
                Phase::Ehlo
            }
            (Phase::Connect, Command::Helo(HeloVariant::Helo(id))) => {
                vctx.id = id;
                Phase::Helo
            }
            (Phase::Ehlo | Phase::Helo, Command::StartTLS) => Phase::StartTLS,
            (Phase::Ehlo | Phase::Helo | Phase::StartTLS, Command::MailFrom(from)) => {
                vctx.mail_from = from;
                Phase::MailFrom
            }
            (Phase::RcptTo | Phase::MailFrom, Command::RcptTo(to)) => {
                if let Some(rcpts) = vctx.rcpt_to.borrow_mut() {
                    rcpts.extend_from_slice(&to[..]);
                } else {
                    vctx.rcpt_to = Some(to);
                }
                Phase::RcptTo
            }
            (Phase::RcptTo, Command::Data) => Phase::Data,
            (Phase::Data, comm) if comm != Command::Quit => Phase::Connect,
            (_, Command::Quit) => Phase::Quit,
            _ => Phase::InvalidCommandSequence,
        }
    }
}
