use core::fmt::{self, Display, Formatter};
use std::collections::HashMap;

use empath_common::address::{Address, AddressList};
use mailparse::MailAddr;

/// ESMTP parameters for MAIL FROM command (RFC 5321 Section 3.3).
///
/// Stores generic key-value parameters from the MAIL FROM command.
/// Common parameters include:
/// - SIZE: Message size in bytes (RFC 1870)
/// - BODY: 7BIT or 8BITMIME (RFC 6152)
/// - AUTH: Authorization identity (RFC 4954)
/// - RET: FULL or HDRS for DSN (RFC 3461)
/// - ENVID: Envelope identifier for DSN (RFC 3461)
/// - SMTPUTF8: UTF-8 support (RFC 6531)
#[derive(PartialEq, Eq, Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct MailParameters {
    params: HashMap<String, Option<String>>,
}

impl MailParameters {
    /// Creates an empty parameter set.
    #[must_use]
    pub fn new() -> Self {
        Self {
            params: HashMap::new(),
        }
    }

    /// Parses ESMTP parameters from a string.
    ///
    /// Parses parameter tokens in the form `KEY=VALUE` or `FLAG`.
    /// All keys are normalized to uppercase for case-insensitive matching.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - A parameter appears multiple times
    /// - SIZE parameter has an invalid value (non-numeric or zero)
    pub fn from_params_str(params_str: &str) -> Result<Self, String> {
        let mut params = Self::new();
        let param_tokens: Vec<&str> = params_str.split_whitespace().collect();

        for token in param_tokens {
            if let Some((key, value)) = token.split_once('=') {
                // Parameter with value: KEY=VALUE
                let key_upper = key.to_uppercase();

                // Check for duplicates using has()
                if params.has(&key_upper) {
                    return Err(format!("Duplicate parameter '{key_upper}' not allowed"));
                }

                // Special validation for SIZE parameter
                if key_upper == "SIZE" {
                    if let Ok(size_val) = value.parse::<usize>() {
                        if size_val == 0 {
                            return Err(String::from("SIZE=0 is not allowed"));
                        }
                        params.insert(key_upper, value);
                    } else {
                        return Err(format!("Invalid SIZE value: {value}"));
                    }
                } else {
                    params.insert(key_upper, value);
                }
            } else {
                // Parameter without value: FLAG (e.g., SMTPUTF8)
                let key_upper = token.to_uppercase();

                if params.has(&key_upper) {
                    return Err(format!("Duplicate parameter '{key_upper}' not allowed"));
                }

                params.insert_flag(key_upper);
            }
        }

        Ok(params)
    }

    /// Adds a parameter with a value.
    pub fn insert(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.params
            .insert(key.into().to_uppercase(), Some(value.into()));
    }

    /// Adds a parameter without a value (flag).
    pub fn insert_flag(&mut self, key: impl Into<String>) {
        self.params.insert(key.into().to_uppercase(), None);
    }

    /// Gets a parameter value by key (case-insensitive).
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&str> {
        self.params.get(&key.to_uppercase())?.as_deref()
    }

    /// Checks if a parameter exists (case-insensitive).
    #[must_use]
    pub fn has(&self, key: &str) -> bool {
        self.params.contains_key(&key.to_uppercase())
    }

    /// Gets the SIZE parameter value, if present.
    #[must_use]
    pub fn size(&self) -> Option<usize> {
        self.get("SIZE")?.parse().ok()
    }

    /// Checks if the parameter set is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.params.is_empty()
    }

    /// Returns an iterator over all parameters.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &Option<String>)> {
        self.params.iter()
    }
}

impl<S: std::hash::BuildHasher + Default> From<MailParameters>
    for HashMap<String, Option<String>, S>
{
    fn from(params: MailParameters) -> Self {
        params.params.into_iter().collect()
    }
}

impl Display for MailParameters {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let params: Vec<String> = self
            .params
            .iter()
            .map(|(k, v)| {
                v.as_ref()
                    .map_or_else(|| k.clone(), |val| format!("{k}={val}"))
            })
            .collect();
        f.write_str(&params.join(" "))
    }
}

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
    /// The second field contains ESMTP parameters from the MAIL FROM command (RFC 5321 Section 3.3).
    Help,
    MailFrom(Option<Address>, MailParameters),
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
    pub fn size(&self) -> Option<usize> {
        match self {
            Self::MailFrom(_, params) => params.size(),
            _ => None,
        }
    }

    /// Get the MAIL FROM parameters, if this is a MAIL FROM command.
    #[must_use]
    pub const fn mail_parameters(&self) -> Option<&MailParameters> {
        match self {
            Self::MailFrom(_, params) => Some(params),
            _ => None,
        }
    }
}

impl Display for Command {
    fn fmt(&self, fmt: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Helo(v) => fmt.write_fmt(format_args!("{} {}", v, self.inner())),
            Self::MailFrom(s, params) => {
                let addr = s.as_ref().map_or_else(String::new, |f| match &**f {
                    MailAddr::Group(_) => String::new(),
                    MailAddr::Single(s) => s.to_string(),
                });
                if params.is_empty() {
                    fmt.write_fmt(format_args!("MAIL FROM:{addr}"))
                } else {
                    fmt.write_fmt(format_args!("MAIL FROM:{addr} {params}"))
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

            // Parse the address and optional ESMTP parameters
            // Format: MAIL FROM:<addr> [param1=value1] [param2=value2] ...
            let rest = command[10..].trim();

            // Split on whitespace to separate address from parameters
            let parts: Vec<&str> = rest.splitn(2, char::is_whitespace).collect();
            let addr = parts[0];

            // Parse ESMTP parameters if present (RFC 5321 Section 3.3)
            let mail_params = if parts.len() > 1 {
                MailParameters::from_params_str(parts[1]).map_err(Self::Invalid)?
            } else {
                MailParameters::new()
            };

            // Handle NULL sender explicitly, as mailparse doesn't tend to like this
            if addr == "<>" {
                return Ok(Self::MailFrom(None, mail_params));
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
                        mail_params,
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
    use crate::command::{Command, HeloVariant, MailParameters};

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
                MailParameters::new()
            ))
        );

        assert!(Command::try_from("Mail From:").is_err());
        assert!(Command::try_from("Mail FROM:dasdas").is_err());
        assert!(Command::try_from("Mail FROM dasdas").is_err());

        assert_eq!(
            Command::try_from("MAIL FROM: <>"),
            Ok(Command::MailFrom(None, MailParameters::new()))
        );

        // Test SIZE parameter parsing
        let mut params_with_size = MailParameters::new();
        params_with_size.insert("SIZE", "12345");
        assert_eq!(
            Command::try_from("MAIL FROM: <test@gmail.com> SIZE=12345"),
            Ok(Command::MailFrom(
                Some(
                    mailparse::addrparse("test@gmail.com").unwrap()[0]
                        .clone()
                        .into()
                ),
                params_with_size
            ))
        );

        let mut params_null_sender = MailParameters::new();
        params_null_sender.insert("SIZE", "1000");
        assert_eq!(
            Command::try_from("MAIL FROM: <> SIZE=1000"),
            Ok(Command::MailFrom(None, params_null_sender))
        );

        for comm in string_casing("mail from") {
            assert!(matches!(
                Command::try_from(format!("{comm}: test@gmail.com")),
                Ok(Command::MailFrom(_, params)) if params.is_empty()
            ));
        }
    }

    #[test]
    fn mail_from_size_edge_cases() {
        // SIZE=0 should be rejected (semantically invalid)
        assert!(matches!(
            Command::try_from("MAIL FROM: <test@example.com> SIZE=0"),
            Err(Command::Invalid(_))
        ));

        // Malformed SIZE values should be rejected
        assert!(matches!(
            Command::try_from("MAIL FROM: <test@example.com> SIZE="),
            Err(Command::Invalid(_))
        ));

        assert!(matches!(
            Command::try_from("MAIL FROM: <test@example.com> SIZE=abc"),
            Err(Command::Invalid(_))
        ));

        // Duplicate SIZE parameters should be rejected
        assert!(matches!(
            Command::try_from("MAIL FROM: <test@example.com> SIZE=1000 SIZE=2000"),
            Err(Command::Invalid(_))
        ));

        // Case insensitive SIZE parameter
        let mut params_lower = MailParameters::new();
        params_lower.insert("SIZE", "5000");
        assert_eq!(
            Command::try_from("MAIL FROM: <test@example.com> size=5000"),
            Ok(Command::MailFrom(
                Some(
                    mailparse::addrparse("test@example.com").unwrap()[0]
                        .clone()
                        .into()
                ),
                params_lower
            ))
        );

        let mut params_mixed = MailParameters::new();
        params_mixed.insert("SIZE", "3000");
        assert_eq!(
            Command::try_from("MAIL FROM: <test@example.com> SiZe=3000"),
            Ok(Command::MailFrom(
                Some(
                    mailparse::addrparse("test@example.com").unwrap()[0]
                        .clone()
                        .into()
                ),
                params_mixed
            ))
        );

        // SIZE with other ESMTP parameters
        let mut params_multi = MailParameters::new();
        params_multi.insert("SIZE", "1000");
        params_multi.insert("BODY", "8BITMIME");
        assert_eq!(
            Command::try_from("MAIL FROM: <test@example.com> SIZE=1000 BODY=8BITMIME"),
            Ok(Command::MailFrom(
                Some(
                    mailparse::addrparse("test@example.com").unwrap()[0]
                        .clone()
                        .into()
                ),
                params_multi
            ))
        );

        // NULL sender with SIZE
        let mut params_null = MailParameters::new();
        params_null.insert("SIZE", "500");
        assert_eq!(
            Command::try_from("MAIL FROM: <> SIZE=500"),
            Ok(Command::MailFrom(None, params_null))
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
