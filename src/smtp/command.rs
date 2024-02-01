use core::fmt::{self, Display, Formatter};

use mailparse::{MailAddr, MailAddrList};

#[derive(PartialEq, PartialOrd, Eq, Hash, Debug)]
pub enum HeloVariant {
    Ehlo(String),
    Helo(String),
}

impl Display for HeloVariant {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
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
    MailFrom(Option<MailAddr>),
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
            Self::MailFrom(from) => from
                .clone()
                .map(|f| match f {
                    MailAddr::Group(_) => String::default(),
                    MailAddr::Single(s) => s.to_string(),
                })
                .unwrap_or_default(),
            Self::RcptTo(to) => to.to_string(),
            Self::Invalid(command) => command.clone(),
            Self::Helo(HeloVariant::Ehlo(id) | HeloVariant::Helo(id)) => id.clone(),
            _ => String::default(),
        }
    }
}

impl Display for Command {
    fn fmt(&self, fmt: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Helo(v) => fmt.write_fmt(format_args!("{} {}", v, self.inner())),
            Self::MailFrom(s) => fmt.write_fmt(format_args!(
                "MAIL FROM:{}",
                s.clone()
                    .map(|f| match f {
                        MailAddr::Group(_) => String::default(),
                        MailAddr::Single(s) => s.to_string(),
                    })
                    .unwrap_or_default()
            )),
            Self::RcptTo(rcpt) => fmt.write_fmt(format_args!("RCPT TO:{rcpt}")),
            Self::Data => fmt.write_str("DATA"),
            Self::Quit => fmt.write_str("QUIT"),
            Self::StartTLS => fmt.write_str("STARTTLS"),
            Self::Invalid(s) => fmt.write_str(s),
        }
    }
}

impl TryFrom<&str> for Command {
    type Error = Self;

    fn try_from(command: &str) -> Result<Self, Self::Error> {
        let comm = command.to_ascii_uppercase();
        let comm = comm.trim();

        if comm.starts_with("MAIL FROM:") {
            if comm.len() < 11 {
                return Err(Self::Invalid(command.to_owned()));
            }

            // Handle NULL sender explicitly, as mailparse doesn't tend to like this
            let addr = command[10..].trim();
            if addr == "<>" {
                return Ok(Self::MailFrom(None));
            }

            mailparse::addrparse(addr).map_or_else(
                |err| Err(Self::Invalid(err.to_string())),
                |from| {
                    Ok(Self::MailFrom(if from.is_empty() {
                        None
                    } else {
                        Some(from[0].clone())
                    }))
                },
            )
        } else if comm.starts_with("RCPT TO:") {
            if comm.len() < 9 {
                return Err(Self::Invalid(command.to_owned()));
            }

            mailparse::addrparse(command[8..].trim()).map_or_else(
                |e| Err(Self::Invalid(e.to_string())),
                |to| Ok(Self::RcptTo(to)),
            )
        } else if comm.starts_with("EHLO") || comm.starts_with("HELO") {
            match command.split_once(' ') {
                None => Err(Self::Invalid(format!("Expected hostname in {comm}"))),
                Some((_, host)) if comm.starts_with('H') => {
                    Ok(Self::Helo(HeloVariant::Helo(host.trim().to_string())))
                }
                Some((_, host)) => Ok(Self::Helo(HeloVariant::Ehlo(host.trim().to_string()))),
            }
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

impl TryFrom<&[u8]> for Command {
    type Error = Self;

    fn try_from(command: &[u8]) -> Result<Self, Self::Error> {
        std::str::from_utf8(command).map_or(
            Err(Self::Invalid("Unable to interpret command".to_string())),
            Self::try_from,
        )
    }
}

impl TryFrom<String> for Command {
    type Error = Self;

    fn try_from(command: String) -> Result<Self, Self::Error> {
        Self::try_from(command.as_str())
    }
}

#[cfg(test)]
mod test {
    use crate::smtp::command::{Command, HeloVariant};

    // Idea copied from https://gitlab.com/erichdongubler-experiments/rust_case_permutations/blob/master/src/lib.rs#L97
    fn string_casing(string: &str) -> impl Iterator<Item = String> {
        let len = string.len();
        let num_cases = usize::pow(2, len as u32);

        let (upper, lower) = string.chars().fold(
            (Vec::with_capacity(len), Vec::with_capacity(len)),
            |(mut upper, mut lower), c| {
                upper.push(c.to_ascii_uppercase());
                lower.push(c.to_ascii_lowercase());
                (upper, lower)
            },
        );

        (0..num_cases).map(move |i| {
            (0..len).fold(String::with_capacity(len), |mut s, idx| {
                if (i & (1 << idx)) == 0 {
                    s.push(lower[idx])
                } else {
                    s.push(upper[idx])
                }
                s
            })
        })
    }

    #[test]
    fn mail_from_command() {
        assert_eq!(
            Command::try_from("Mail From: test@gmail.com"),
            Ok(Command::MailFrom(Some(
                mailparse::addrparse("test@gmail.com").unwrap()[0].clone()
            )))
        );

        assert!(Command::try_from("Mail From:").is_err());
        assert!(Command::try_from("Mail FROM:dasdas").is_err());
        assert!(Command::try_from("Mail FROM dasdas").is_err());

        assert_eq!(
            Command::try_from("MAIL FROM: <>"),
            Ok(Command::MailFrom(None))
        );

        for comm in string_casing("mail from") {
            assert!(matches!(
                Command::try_from(format!("{comm}: test@gmail.com")),
                Ok(Command::MailFrom(_))
            ));
        }
    }

    #[test]
    fn rcpt_to_command() {
        assert_eq!(
            Command::try_from("Rcpt To: test@gmail.com"),
            Ok(Command::RcptTo(
                mailparse::addrparse("test@gmail.com").unwrap()
            ))
        );

        assert!(Command::try_from("Rcpt To:").is_err());
        assert!(Command::try_from("RCPT TO:dasdsa").is_err());
        assert!(Command::try_from("RCPT TO dasdsa").is_err());

        for comm in string_casing("rcpt to") {
            assert!(matches!(
                Command::try_from(format!("{comm}: test@gmail.com")),
                Ok(Command::RcptTo(_))
            ));
        }
    }

    #[test]
    fn helo_ehlo_command() {
        assert!(Command::try_from("EHLO").is_err());
        assert!(Command::try_from("HELO").is_err());

        assert_eq!(
            Command::try_from("EHLO Testing things"),
            Ok(Command::Helo(crate::smtp::command::HeloVariant::Ehlo(
                String::from("Testing things")
            )))
        );

        assert_eq!(
            Command::try_from("HELO Testing things"),
            Ok(Command::Helo(crate::smtp::command::HeloVariant::Helo(
                String::from("Testing things")
            )))
        );

        for comm in string_casing("ehlo") {
            assert!(
                matches!(
                    Command::try_from(format!("{comm} test")),
                    Ok(Command::Helo(HeloVariant::Ehlo(_)))
                ),
                "'{comm}' should map to Ehlo"
            );
        }

        for comm in string_casing("helo") {
            assert!(
                matches!(
                    Command::try_from(format!("{comm} test")),
                    Ok(Command::Helo(HeloVariant::Helo(_))),
                ),
                "'{comm}' should map to Helo"
            );
        }
    }

    #[test]
    fn other_commands() {
        assert_eq!(Command::try_from("DATA"), Ok(Command::Data));
        for comm in string_casing("data") {
            assert_eq!(Command::try_from(comm), Ok(Command::Data));
        }

        assert_eq!(Command::try_from("QUIT"), Ok(Command::Quit));
        for comm in string_casing("quit") {
            assert_eq!(Command::try_from(comm), Ok(Command::Quit));
        }

        assert_eq!(Command::try_from("STARTTLS"), Ok(Command::StartTLS));
        for comm in string_casing("starttls") {
            assert_eq!(Command::try_from(comm), Ok(Command::StartTLS));
        }
    }
}
