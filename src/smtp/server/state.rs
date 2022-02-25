use std::{
    borrow::BorrowMut,
    fmt::{Display, Formatter},
    str::FromStr,
};

use crate::common::command::{Command, HeloVariant};
use crate::validation_context::ValidationContext;

#[derive(PartialEq, PartialOrd, Eq, Hash, Debug, Clone, Copy)]
pub enum State {
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

impl Display for State {
    fn fmt(&self, fmt: &mut Formatter<'_>) -> Result<(), std::fmt::Error> {
        fmt.write_str(match self {
            State::Reading | State::DataReceived => "",
            State::Connect => "Connect",
            State::Close => "Close",
            State::Ehlo => "EHLO",
            State::Helo => "HELO",
            State::StartTLS => "STARTTLS",
            State::MailFrom => "MAIL",
            State::RcptTo => "RCPT",
            State::Data => "DATA",
            State::Quit => "QUIT",
            State::Invalid => "INVALID",
            State::InvalidCommandSequence => "Invalid Command Sequence",
        })
    }
}

impl FromStr for State {
    type Err = Self;

    fn from_str(command: &str) -> Result<Self, <Self as FromStr>::Err> {
        match command.to_ascii_uppercase().trim() {
            "EHLO" => Ok(State::Ehlo),
            "HELO" => Ok(State::Helo),
            "STARTTLS" => Ok(State::StartTLS),
            "MAIL" => Ok(State::MailFrom),
            "RCPT" => Ok(State::RcptTo),
            "DATA" => Ok(State::Data),
            "QUIT" => Ok(State::Quit),
            _ => Err(State::Invalid),
        }
    }
}

impl State {
    pub(crate) fn transition(self, command: Command, vctx: &mut ValidationContext) -> State {
        match (self, command) {
            (State::Connect, Command::Helo(HeloVariant::Ehlo(id))) => {
                vctx.id = id;
                State::Ehlo
            }
            (State::Connect, Command::Helo(HeloVariant::Helo(id))) => {
                vctx.id = id;
                State::Helo
            }
            (State::Ehlo | State::Helo, Command::StartTLS) => State::StartTLS,
            (State::Ehlo | State::Helo | State::StartTLS, Command::MailFrom(from)) => {
                vctx.mail_from = from;
                State::MailFrom
            }
            (State::RcptTo | State::MailFrom, Command::RcptTo(to)) => {
                if let Some(rcpts) = vctx.rcpt_to.borrow_mut() {
                    rcpts.push(to);
                } else {
                    vctx.rcpt_to = Some(vec![to]);
                }
                State::RcptTo
            }
            (State::RcptTo, Command::Data) => State::Data,
            (_, Command::Quit) => State::Quit,
            _ => State::InvalidCommandSequence,
        }
    }
}
