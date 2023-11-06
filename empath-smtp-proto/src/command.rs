use std::{
    fmt::{Display, Formatter},
    str::FromStr,
};

use mailparse::MailAddrList;

use empath_common::tracing::error;

#[derive(PartialEq, PartialOrd, Eq, Hash, Debug)]
pub enum HeloVariant {
    Ehlo(String),
    Helo(String),
}

impl Display for HeloVariant {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Ehlo(_) => "EHLO",
            Self::Helo(_) => "HELO",
        })
    }
}

#[derive(Eq, PartialEq, Debug)]
pub enum Command {
    Helo(HeloVariant),
    /// If this is `None`, then it should be assumed this is the `null sender`, or `null reverse-path`,
    /// from [RFC-5321](https://www.ietf.org/rfc/rfc5321.txt).
    MailFrom(Option<MailAddrList>),
    RcptTo(MailAddrList),
    Data,
    Quit,
    StartTLS,
    Invalid(String),
}

impl Command {
    #[must_use]
    pub fn inner(&self) -> String {
        match self {
            Self::MailFrom(from) => from.clone().map(|f| f.to_string()).unwrap_or_default(),
            Self::RcptTo(to) => to.to_string(),
            Self::Invalid(command) => command.clone(),
            Self::Helo(HeloVariant::Ehlo(id) | HeloVariant::Helo(id)) => id.clone(),
            _ => String::default(),
        }
    }
}

impl Display for Command {
    fn fmt(&self, fmt: &mut Formatter<'_>) -> Result<(), std::fmt::Error> {
        match self {
            Self::Helo(v) => fmt.write_fmt(format_args!("{} {}", v, self.inner())),
            Self::MailFrom(s) => fmt.write_fmt(format_args!(
                "MAIL FROM:{}",
                s.clone().map(|f| f.to_string()).unwrap_or_default()
            )),
            Self::RcptTo(rcpt) => fmt.write_fmt(format_args!("RCPT TO:{rcpt}")),
            Self::Data => fmt.write_str("DATA"),
            Self::Quit => fmt.write_str("QUIT"),
            Self::StartTLS => fmt.write_str("STARTTLS"),
            Self::Invalid(s) => fmt.write_str(s),
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
                    error!("{e}");
                    e.to_string()
                })?;

            Ok(Self::MailFrom(if from.is_empty() {
                None
            } else {
                Some(from)
            }))
        } else if comm.starts_with("RCPT TO:") {
            let to = mailparse::addrparse(command[command.find(':').unwrap() + 1..].trim())
                .map_err(|e| e.to_string())?;
            Ok(Self::RcptTo(to))
        } else if comm.starts_with("EHLO") {
            Ok(Self::Helo(HeloVariant::Ehlo(
                command
                    .split(' ')
                    .nth(1)
                    .unwrap_or_default()
                    .trim()
                    .to_string(),
            )))
        } else if comm.starts_with("HELO") {
            Ok(Self::Helo(HeloVariant::Helo(
                command
                    .split(' ')
                    .nth(1)
                    .unwrap_or_default()
                    .trim()
                    .to_string(),
            )))
        } else {
            match comm {
                "DATA" => Ok(Self::Data),
                "QUIT" => Ok(Self::Quit),
                "STARTTLS" => Ok(Self::StartTLS),
                _ => Err(Self::Invalid(command.to_owned())),
            }
        }
    }
}

impl From<&str> for Command {
    fn from(val: &str) -> Self {
        Self::from_str(val).unwrap_or_else(|e| e)
    }
}

impl From<String> for Command {
    fn from(val: String) -> Self {
        Self::from(val.as_str())
    }
}

impl From<&[u8]> for Command {
    fn from(val: &[u8]) -> Self {
        std::str::from_utf8(val).map_or(
            Self::Invalid("Unable to interpret command".to_string()),
            |s| Self::from_str(s).unwrap_or_else(|e| e),
        )
    }
}
