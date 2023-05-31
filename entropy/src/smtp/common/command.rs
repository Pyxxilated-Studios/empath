use std::{
    fmt::{Display, Formatter},
    str::FromStr,
};

use mailparse::MailAddrList;

#[derive(PartialEq, PartialOrd, Eq, Hash, Debug)]
pub enum HeloVariant {
    Ehlo(String),
    Helo(String),
}

impl Display for HeloVariant {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            HeloVariant::Ehlo(_) => "EHLO",
            HeloVariant::Helo(_) => "HELO",
        })
    }
}

#[derive(PartialEq, Debug)]
pub enum Command {
    Helo(HeloVariant),
    MailFrom(Option<MailAddrList>),
    RcptTo(MailAddrList),
    Data,
    Quit,
    StartTLS,
    Invalid(String),
}

impl Command {
    pub(crate) fn inner(&self) -> String {
        match self {
            Command::MailFrom(from) => from.clone().map(|f| f.to_string()).unwrap_or_default(),
            Command::RcptTo(to) => to.to_string(),
            Command::Invalid(command) => command.clone(),
            Command::Helo(HeloVariant::Ehlo(id) | HeloVariant::Helo(id)) => id.clone(),
            _ => String::default(),
        }
    }
}

impl Display for Command {
    fn fmt(&self, fmt: &mut Formatter<'_>) -> Result<(), std::fmt::Error> {
        match self {
            Command::Helo(v) => fmt.write_fmt(format_args!("{} {}", v, self.inner())),
            Command::MailFrom(s) => fmt.write_fmt(format_args!(
                "MAIL FROM:{}",
                s.clone().map(|f| f.to_string()).unwrap_or_default()
            )),
            Command::RcptTo(rcpt) => fmt.write_fmt(format_args!("RCPT TO:{}", rcpt)),
            Command::Data => fmt.write_str("DATA"),
            Command::Quit => fmt.write_str("QUIT"),
            Command::StartTLS => fmt.write_str("STARTTLS"),
            Command::Invalid(s) => fmt.write_str(s),
        }
    }
}

impl FromStr for Command {
    type Err = Self;

    fn from_str(command: &str) -> Result<Self, <Self as FromStr>::Err> {
        let comm = command.to_ascii_uppercase();
        let comm = comm.trim();

        if comm.starts_with("MAIL FROM:") {
            let from = mailparse::addrparse(command[command.find(':').unwrap() + 1..].trim())
                .map_err(|e| {
                    println!("ERROR: {e}");
                    e.to_string()
                })?;

            Ok(Command::MailFrom(if from.is_empty() {
                None
            } else {
                Some(from)
            }))
        } else if comm.starts_with("RCPT TO:") {
            let to = mailparse::addrparse(command[command.find(':').unwrap() + 1..].trim())
                .map_err(|e| e.to_string())?;
            Ok(Command::RcptTo(to))
        } else if comm.starts_with("EHLO") {
            Ok(Command::Helo(HeloVariant::Ehlo(
                command
                    .split(' ')
                    .nth(1)
                    .unwrap_or_default()
                    .trim()
                    .to_string(),
            )))
        } else if comm.starts_with("HELO") {
            Ok(Command::Helo(HeloVariant::Helo(
                command
                    .split(' ')
                    .nth(1)
                    .unwrap_or_default()
                    .trim()
                    .to_string(),
            )))
        } else {
            match comm {
                "DATA" => Ok(Command::Data),
                "QUIT" => Ok(Command::Quit),
                "STARTTLS" => Ok(Command::StartTLS),
                _ => Err(Command::Invalid(command.to_owned())),
            }
        }
    }
}

impl From<&str> for Command {
    fn from(val: &str) -> Self {
        Command::from_str(val).unwrap_or_else(|e| e)
    }
}

impl From<String> for Command {
    fn from(val: String) -> Self {
        Command::from(val.as_str())
    }
}

impl From<&[u8]> for Command {
    fn from(val: &[u8]) -> Self {
        match std::str::from_utf8(val) {
            Ok(s) => Command::from_str(s).unwrap_or_else(|e| e),
            Err(_) => Command::Invalid("Unable to interpret command".to_string()),
        }
    }
}
