use std::{
    fmt::{Display, Formatter},
    str::FromStr,
};

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

#[derive(PartialEq, PartialOrd, Eq, Hash, Debug)]
pub enum Command {
    Helo(HeloVariant),
    MailFrom(Option<String>),
    RcptTo(String),
    Data,
    Quit,
    Invalid(String),
}

impl Display for Command {
    fn fmt(&self, fmt: &mut Formatter<'_>) -> Result<(), std::fmt::Error> {
        match self {
            Command::Helo(v) => fmt.write_fmt(format_args!("{}", v)),
            Command::MailFrom(s) => {
                fmt.write_fmt(format_args!("MAIL FROM:<{}>", s.as_deref().unwrap_or("")))
            }
            Command::RcptTo(rcpt) => fmt.write_fmt(format_args!("RCPT TO:{}", rcpt)),
            Command::Data => fmt.write_str("DATA"),
            Command::Quit => fmt.write_str("QUIT"),
            Command::Invalid(_) => fmt.write_str("INVALID"),
        }
    }
}

impl FromStr for Command {
    type Err = Self;

    fn from_str(command: &str) -> Result<Self, <Self as FromStr>::Err> {
        let comm = command.to_ascii_uppercase();
        let comm = comm.trim();

        if comm.starts_with("MAIL FROM:") {
            let from = command[command.find(':').unwrap() + 1..].to_string();
            let from = from.trim();

            Ok(Command::MailFrom(if from.is_empty() {
                None
            } else {
                Some(from.to_string())
            }))
        } else if comm.starts_with("RCPT TO:") {
            Ok(Command::RcptTo(
                command[command.find(':').unwrap() + 1..].to_string(),
            ))
        } else if comm.starts_with("EHLO") {
            Ok(Command::Helo(HeloVariant::Ehlo(
                comm.split(' ').nth(1).unwrap_or_default().to_string(),
            )))
        } else if comm.starts_with("HELO") {
            Ok(Command::Helo(HeloVariant::Helo(
                comm.split(' ').nth(1).unwrap_or_default().to_string(),
            )))
        } else {
            match comm {
                "DATA" => Ok(Command::Data),
                "QUIT" => Ok(Command::Quit),
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
        Command::from(val.as_ref())
    }
}
