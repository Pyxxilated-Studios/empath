use core::fmt::{self, Display, Formatter};

use empath_common::address::{Address, AddressList};
use mailparse::MailAddr;

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
    /// If this contains `None`, then it should be assumed this is the `null sender`, or `null reverse-path`,
    /// from [RFC-5321](https://www.ietf.org/rfc/rfc5321.txt).
    /// The second field contains the optional SIZE parameter value from the MAIL FROM command.
    Help,
    MailFrom(Option<Address>, Option<usize>),
    RcptTo(AddressList),
    Rset,
    Auth,
    Data,
    Quit,
    StartTLS,
    Invalid(String),
}

impl Command {
    #[must_use]
    pub fn inner(&self) -> String {
        match self {
            Self::MailFrom(from, _) => from.as_ref().map_or_else(String::new, |f| match &**f {
                MailAddr::Group(_) => String::new(),
                MailAddr::Single(s) => s.to_string(),
            }),
            Self::RcptTo(to) => to.to_string(),
            Self::Invalid(command) => command.clone(),
            Self::Helo(HeloVariant::Ehlo(id) | HeloVariant::Helo(id)) => id.clone(),
            _ => String::new(),
        }
    }

    /// Extract the SIZE parameter from a MAIL FROM command, if present.
    ///
    /// Per RFC 1870, the SIZE parameter indicates the size (in bytes) of the
    /// message the client intends to transmit. Returns `Some(size)` if the
    /// SIZE parameter was present in the MAIL FROM command, or `None` otherwise.
    ///
    /// # Example
    ///
    /// ```ignore
    /// // MAIL FROM:<user@example.com> SIZE=12345
    /// assert_eq!(command.size(), Some(12345));
    ///
    /// // MAIL FROM:<user@example.com>
    /// assert_eq!(command.size(), None);
    /// ```
    #[must_use]
    pub const fn size(&self) -> Option<usize> {
        match self {
            Self::MailFrom(_, size) => *size,
            _ => None,
        }
    }
}

impl Display for Command {
    fn fmt(&self, fmt: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Helo(v) => fmt.write_fmt(format_args!("{} {}", v, self.inner())),
            Self::MailFrom(s, size) => {
                let addr = s.as_ref().map_or_else(String::new, |f| match &**f {
                    MailAddr::Group(_) => String::new(),
                    MailAddr::Single(s) => s.to_string(),
                });
                if let Some(size_val) = size {
                    fmt.write_fmt(format_args!("MAIL FROM:{addr} SIZE={size_val}"))
                } else {
                    fmt.write_fmt(format_args!("MAIL FROM:{addr}"))
                }
            }
            Self::RcptTo(rcpt) => fmt.write_fmt(format_args!("RCPT TO:{rcpt}")),
            Self::Data => fmt.write_str("DATA"),
            Self::Quit => fmt.write_str("QUIT"),
            Self::StartTLS => fmt.write_str("STARTTLS"),
            Self::Invalid(s) => fmt.write_str(s),
            Self::Help => fmt.write_str("HELP"),
            Self::Rset => fmt.write_str("Rset"),
            Self::Auth => fmt.write_str("AUTH"),
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

            // Parse the address and optional SIZE parameter
            // Format: MAIL FROM:<addr> [SIZE=<size>]
            let rest = command[10..].trim();

            // Split on whitespace to separate address from parameters
            let parts: Vec<&str> = rest.splitn(2, char::is_whitespace).collect();
            let addr = parts[0];

            // Parse SIZE parameter if present (RFC 1870)
            // Format: MAIL FROM:<addr> [SIZE=<size>] [other ESMTP params...]
            let size = if parts.len() > 1 {
                let params: Vec<&str> = parts[1].split_whitespace().collect();

                // Check for duplicate SIZE parameters
                let size_params: Vec<&str> = params
                    .iter()
                    .filter(|p| p.len() >= 5 && p[..5].eq_ignore_ascii_case("SIZE="))
                    .copied()
                    .collect();

                if size_params.len() > 1 {
                    // Duplicate SIZE parameters - should reject per RFC
                    return Err(Self::Invalid(String::from(
                        "Duplicate SIZE parameter not allowed",
                    )));
                }

                size_params.first().and_then(|size_param| {
                    size_param.split('=').nth(1).and_then(|s| {
                        s.parse::<usize>().ok().and_then(|val| {
                            // Reject SIZE=0 as it's semantically unclear
                            // RFC 1870 Section 4: "value zero indicates no fixed maximum"
                            // but clients shouldn't declare 0-byte messages
                            if val == 0 { None } else { Some(val) }
                        })
                    })
                })
            } else {
                None
            };

            // Handle NULL sender explicitly, as mailparse doesn't tend to like this
            if addr == "<>" {
                return Ok(Self::MailFrom(None, size));
            }

            mailparse::addrparse(addr).map_or_else(
                |err| Err(Self::Invalid(err.to_string())),
                |from| {
                    Ok(Self::MailFrom(
                        if from.is_empty() {
                            None
                        } else {
                            Some(from[0].clone().into())
                        },
                        size,
                    ))
                },
            )
        } else if comm.starts_with("RCPT TO:") {
            if comm.len() < 9 {
                return Err(Self::Invalid(command.to_owned()));
            }

            mailparse::addrparse(command[8..].trim()).map_or_else(
                |e| Err(Self::Invalid(e.to_string())),
                |to| Ok(Self::RcptTo(to.into())),
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
                "HELP" => Ok(Self::Help),
                "AUTH" => Ok(Self::Auth),
                "RSET" => Ok(Self::Rset),
                _ => Err(Self::Invalid(command.to_owned())),
            }
        }
    }
}

impl TryFrom<&[u8]> for Command {
    type Error = Self;

    fn try_from(command: &[u8]) -> Result<Self, Self::Error> {
        std::str::from_utf8(command).map_or_else(
            |_| Err(Self::Invalid("Unable to interpret command".to_string())),
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
    use crate::command::{Command, HeloVariant};

    // Idea copied from https://gitlab.com/erichdongubler-experiments/rust_case_permutations/blob/master/src/lib.rs#L97
    fn string_casing(string: &str) -> impl Iterator<Item = String> {
        let len = string.len();
        let num_cases = usize::pow(2, u32::try_from(len).unwrap_or(0));

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
                    s.push(lower[idx]);
                } else {
                    s.push(upper[idx]);
                }
                s
            })
        })
    }

    #[test]
    fn mail_from_command() {
        assert_eq!(
            Command::try_from("Mail From: test@gmail.com"),
            Ok(Command::MailFrom(
                Some(
                    mailparse::addrparse("test@gmail.com").unwrap()[0]
                        .clone()
                        .into()
                ),
                None
            ))
        );

        assert!(Command::try_from("Mail From:").is_err());
        assert!(Command::try_from("Mail FROM:dasdas").is_err());
        assert!(Command::try_from("Mail FROM dasdas").is_err());

        assert_eq!(
            Command::try_from("MAIL FROM: <>"),
            Ok(Command::MailFrom(None, None))
        );

        // Test SIZE parameter parsing
        assert_eq!(
            Command::try_from("MAIL FROM: <test@gmail.com> SIZE=12345"),
            Ok(Command::MailFrom(
                Some(
                    mailparse::addrparse("test@gmail.com").unwrap()[0]
                        .clone()
                        .into()
                ),
                Some(12345)
            ))
        );

        assert_eq!(
            Command::try_from("MAIL FROM: <> SIZE=1000"),
            Ok(Command::MailFrom(None, Some(1000)))
        );

        for comm in string_casing("mail from") {
            assert!(matches!(
                Command::try_from(format!("{comm}: test@gmail.com")),
                Ok(Command::MailFrom(_, None))
            ));
        }
    }

    #[test]
    fn mail_from_size_edge_cases() {
        // SIZE=0 should be rejected (semantically invalid)
        assert_eq!(
            Command::try_from("MAIL FROM: <test@example.com> SIZE=0"),
            Ok(Command::MailFrom(
                Some(
                    mailparse::addrparse("test@example.com").unwrap()[0]
                        .clone()
                        .into()
                ),
                None
            ))
        );

        // Malformed SIZE values should be silently ignored
        assert!(matches!(
            Command::try_from("MAIL FROM: <test@example.com> SIZE="),
            Ok(Command::MailFrom(_, None))
        ));

        assert!(matches!(
            Command::try_from("MAIL FROM: <test@example.com> SIZE=abc"),
            Ok(Command::MailFrom(_, None))
        ));

        // Duplicate SIZE parameters should be rejected
        assert!(matches!(
            Command::try_from("MAIL FROM: <test@example.com> SIZE=1000 SIZE=2000"),
            Err(Command::Invalid(_))
        ));

        // Case insensitive SIZE parameter
        assert_eq!(
            Command::try_from("MAIL FROM: <test@example.com> size=5000"),
            Ok(Command::MailFrom(
                Some(
                    mailparse::addrparse("test@example.com").unwrap()[0]
                        .clone()
                        .into()
                ),
                Some(5000)
            ))
        );

        assert_eq!(
            Command::try_from("MAIL FROM: <test@example.com> SiZe=3000"),
            Ok(Command::MailFrom(
                Some(
                    mailparse::addrparse("test@example.com").unwrap()[0]
                        .clone()
                        .into()
                ),
                Some(3000)
            ))
        );

        // SIZE with other ESMTP parameters (future-proofing)
        assert_eq!(
            Command::try_from("MAIL FROM: <test@example.com> SIZE=1000 BODY=8BITMIME"),
            Ok(Command::MailFrom(
                Some(
                    mailparse::addrparse("test@example.com").unwrap()[0]
                        .clone()
                        .into()
                ),
                Some(1000)
            ))
        );

        // NULL sender with SIZE
        assert_eq!(
            Command::try_from("MAIL FROM: <> SIZE=500"),
            Ok(Command::MailFrom(None, Some(500)))
        );
    }

    #[test]
    fn rcpt_to_command() {
        assert_eq!(
            Command::try_from("Rcpt To: test@gmail.com"),
            Ok(Command::RcptTo(
                mailparse::addrparse("test@gmail.com").unwrap().into()
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
            Ok(Command::Helo(crate::command::HeloVariant::Ehlo(
                String::from("Testing things")
            )))
        );

        assert_eq!(
            Command::try_from("HELO Testing things"),
            Ok(Command::Helo(crate::command::HeloVariant::Helo(
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

        assert_eq!(Command::try_from("RSET"), Ok(Command::Rset));
        for comm in string_casing("rset") {
            assert_eq!(Command::try_from(comm), Ok(Command::Rset));
        }

        assert_eq!(Command::try_from("AUTH"), Ok(Command::Auth));
        for comm in string_casing("auth") {
            assert_eq!(Command::try_from(comm), Ok(Command::Auth));
        }

        assert_eq!(Command::try_from("HELP"), Ok(Command::Help));
        for comm in string_casing("help") {
            assert_eq!(Command::try_from(comm), Ok(Command::Help));
        }
    }
}
