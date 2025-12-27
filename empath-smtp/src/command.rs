use core::fmt::{self, Display, Formatter};
use std::borrow::Cow;

use ahash::AHashMap;
use empath_common::{
    address::{Address, AddressList},
    address_parser,
};
use phf::phf_map;

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
    params: AHashMap<Cow<'static, str>, Option<String>>,
}

/// XCLIENT parameters for trusted upstream proxies.
///
/// Stores key-value parameters from the XCLIENT command.
#[derive(PartialEq, Eq, Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct XclientParameters {
    params: AHashMap<String, String>,
}

/// Perfect hash map of known ESMTP parameters for O(1) lookup
static KNOWN_PARAMS: phf::Map<&'static str, &'static str> = phf_map! {
    "SIZE" => "SIZE",
    "BODY" => "BODY",
    "AUTH" => "AUTH",
    "RET" => "RET",
    "ENVID" => "ENVID",
    "SMTPUTF8" => "SMTPUTF8",
};

/// Normalize a parameter key, using static str for known parameters with O(1) lookup
fn normalize_key(key: &str) -> Cow<'static, str> {
    // Stack-allocated buffer for uppercasing (max param name length is typically < 16)
    let mut upper_buf = [0u8; 16];
    let key_bytes = key.as_bytes();
    let len = key_bytes.len().min(16);

    // Convert to uppercase in-place
    for i in 0..len {
        upper_buf[i] = key_bytes[i].to_ascii_uppercase();
    }

    // SAFETY: We only uppercased ASCII bytes, so still valid UTF-8
    let upper = unsafe { std::str::from_utf8_unchecked(&upper_buf[..len]) };

    // O(1) lookup in perfect hash map
    KNOWN_PARAMS
        .get(upper)
        .map_or_else(|| Cow::Owned(upper.to_string()), |&s| Cow::Borrowed(s))
}

impl MailParameters {
    /// Creates an empty parameter set.
    #[must_use]
    pub fn new() -> Self {
        Self {
            params: AHashMap::new(),
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
                let key_normalized = normalize_key(key);

                // Check for duplicates using has()
                if params.has(key) {
                    return Err(format!(
                        "Duplicate parameter '{key_normalized}' not allowed"
                    ));
                }

                // Special validation for SIZE parameter
                if key_normalized == "SIZE" {
                    if let Ok(size_val) = value.parse::<usize>() {
                        if size_val == 0 {
                            return Err(String::from("SIZE=0 is not allowed"));
                        }
                        params
                            .params
                            .insert(key_normalized, Some(value.to_string()));
                    } else {
                        return Err(format!("Invalid SIZE value: {value}"));
                    }
                } else {
                    params
                        .params
                        .insert(key_normalized, Some(value.to_string()));
                }
            } else {
                // Parameter without value: FLAG (e.g., SMTPUTF8)
                let key_normalized = normalize_key(token);

                if params.has(token) {
                    return Err(format!(
                        "Duplicate parameter '{key_normalized}' not allowed"
                    ));
                }

                params.params.insert(key_normalized, None);
            }
        }

        Ok(params)
    }

    /// Adds a parameter with a value.
    pub fn insert(&mut self, key: impl Into<String>, value: impl Into<String>) {
        let key_str = key.into();
        self.params
            .insert(normalize_key(&key_str), Some(value.into()));
    }

    /// Adds a parameter without a value (flag).
    pub fn insert_flag(&mut self, key: impl Into<String>) {
        let key_str = key.into();
        self.params.insert(normalize_key(&key_str), None);
    }

    /// Gets a parameter value by key (case-insensitive).
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&str> {
        self.params.get(normalize_key(key).as_ref())?.as_deref()
    }

    /// Checks if a parameter exists (case-insensitive).
    #[must_use]
    pub fn has(&self, key: &str) -> bool {
        self.params.contains_key(normalize_key(key).as_ref())
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
    pub fn iter(&self) -> impl Iterator<Item = (&Cow<'static, str>, &Option<String>)> {
        self.params.iter()
    }
}

impl XclientParameters {
    /// Creates an empty parameter set.
    #[must_use]
    pub fn new() -> Self {
        Self {
            params: AHashMap::new(),
        }
    }

    /// Parses XCLIENT parameters from a string.
    ///
    /// Parameters are `KEY=VALUE` tokens separated by whitespace.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - A parameter is missing `=`
    /// - A parameter key is empty
    /// - A parameter appears multiple times
    pub fn from_params_str(params_str: &str) -> Result<Self, String> {
        let mut params = Self::new();
        for token in params_str.split_whitespace() {
            let (key, value) = token
                .split_once('=')
                .ok_or_else(|| format!("Invalid XCLIENT token: {token}"))?;
            if key.is_empty() {
                return Err(String::from("XCLIENT parameter key cannot be empty"));
            }
            let key_normalized = key.to_ascii_uppercase();
            if params.params.contains_key(&key_normalized) {
                return Err(format!(
                    "Duplicate XCLIENT parameter '{key_normalized}' not allowed"
                ));
            }
            params.params.insert(key_normalized, value.to_string());
        }

        Ok(params)
    }

    /// Gets a parameter value by key (case-insensitive).
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&str> {
        self.params
            .get(&key.to_ascii_uppercase())
            .map(String::as_str)
    }

    /// Returns an iterator over all parameters.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &String)> {
        self.params.iter()
    }
}

impl From<MailParameters> for AHashMap<Cow<'static, str>, Option<String>> {
    fn from(params: MailParameters) -> Self {
        params.params
    }
}

impl Display for MailParameters {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let mut first = true;
        for (k, v) in &self.params {
            if !first {
                f.write_str(" ")?;
            }
            first = false;

            match v {
                None => f.write_str(k)?,
                Some(val) => write!(f, "{k}={val}")?,
            }
        }
        Ok(())
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
    Auth {
        mechanism: String,
        initial_response: Option<String>,
    },
    Data,
    Quit,
    Noop(Option<String>),
    Vrfy(String),
    Expn(String),
    Etrn(String),
    Turn,
    Xclient(XclientParameters),
    StartTLS,
    Invalid(String),
}

impl Command {
    #[must_use]
    pub fn inner(&self) -> Cow<'_, str> {
        match self {
            Self::MailFrom(from, _) => from
                .as_ref()
                .map_or_else(|| Cow::Borrowed(""), |f| Cow::Owned(f.to_string())),
            Self::RcptTo(to) => Cow::Owned(to.to_string()),
            Self::Invalid(command) => Cow::Borrowed(command.as_str()),
            Self::Helo(HeloVariant::Ehlo(id) | HeloVariant::Helo(id)) => Cow::Borrowed(id.as_str()),
            _ => Cow::Borrowed(""),
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

    fn parse_mail_from(trimmed: &str, command: &str) -> Option<Result<Self, Self>> {
        if !(trimmed.len() >= 10 && trimmed[..10].eq_ignore_ascii_case("MAIL FROM:")) {
            return None;
        }

        if trimmed.len() < 11 {
            return Some(Err(Self::Invalid(command.to_owned())));
        }

        // Parse the address and optional ESMTP parameters
        // Format: MAIL FROM:<addr> [param1=value1] [param2=value2] ...
        let rest = trimmed[10..].trim();

        // Split on whitespace to separate address from parameters
        let parts: Vec<&str> = rest.splitn(2, char::is_whitespace).collect();
        let addr = parts[0];

        // Parse ESMTP parameters if present (RFC 5321 Section 3.3)
        let mail_params = if parts.len() > 1 {
            match MailParameters::from_params_str(parts[1]) {
                Ok(params) => params,
                Err(err) => return Some(Err(Self::Invalid(err))),
            }
        } else {
            MailParameters::new()
        };

        // Parse MAIL FROM address using RFC 5321-compliant parser
        // Supports null sender (<>) and standard mailbox syntax
        // Auto-wrap addresses without angle brackets for compatibility
        let addr_with_brackets = if addr.starts_with('<') {
            addr.to_string()
        } else {
            format!("<{addr}>")
        };

        Some(
            address_parser::parse_reverse_path(&addr_with_brackets).map_or_else(
                |err| Err(Self::Invalid(format!("Invalid MAIL FROM address: {err}"))),
                |mailbox_opt| Ok(Self::MailFrom(mailbox_opt.map(Address::from), mail_params)),
            ),
        )
    }

    fn parse_rcpt_to(trimmed: &str, command: &str) -> Option<Result<Self, Self>> {
        if !(trimmed.len() >= 8 && trimmed[..8].eq_ignore_ascii_case("RCPT TO:")) {
            return None;
        }

        if trimmed.len() < 9 {
            return Some(Err(Self::Invalid(command.to_owned())));
        }

        // Parse RCPT TO address using RFC 5321-compliant parser
        // Auto-wrap addresses without angle brackets for compatibility
        let addr = trimmed[8..].trim();
        let addr_with_brackets = if addr.starts_with('<') {
            addr.to_string()
        } else {
            format!("<{addr}>")
        };

        Some(
            address_parser::parse_forward_path(&addr_with_brackets).map_or_else(
                |err| Err(Self::Invalid(format!("Invalid RCPT TO address: {err}"))),
                |mailbox| {
                    // Convert single Mailbox to AddressList
                    Ok(Self::RcptTo(AddressList::from(vec![Address::from(
                        mailbox,
                    )])))
                },
            ),
        )
    }

    fn parse_xclient(trimmed: &str) -> Option<Result<Self, Self>> {
        if !(trimmed.len() >= 7 && trimmed[..7].eq_ignore_ascii_case("XCLIENT")) {
            return None;
        }

        let params = if trimmed.len() == 7 {
            XclientParameters::new()
        } else {
            let rest = trimmed[7..].trim();
            if rest.is_empty() {
                XclientParameters::new()
            } else {
                match XclientParameters::from_params_str(rest) {
                    Ok(params) => params,
                    Err(err) => return Some(Err(Self::Invalid(err))),
                }
            }
        };

        Some(Ok(Self::Xclient(params)))
    }

    fn parse_simple_command(trimmed: &str, command: &str) -> Option<Result<Self, Self>> {
        if trimmed.len() < 4 {
            return None;
        }

        let prefix = &trimmed[..4];
        let command = if prefix.eq_ignore_ascii_case("EHLO") || prefix.eq_ignore_ascii_case("HELO")
        {
            match trimmed.split_once(' ') {
                None => Err(Self::Invalid(format!("Expected hostname in {trimmed}"))),
                Some((cmd, host)) if cmd.eq_ignore_ascii_case("HELO") => {
                    Ok(Self::Helo(HeloVariant::Helo(host.trim().to_string())))
                }
                Some((_, host)) => Ok(Self::Helo(HeloVariant::Ehlo(host.trim().to_string()))),
            }
        } else if trimmed.eq_ignore_ascii_case("DATA") {
            Ok(Self::Data)
        } else if trimmed.eq_ignore_ascii_case("QUIT") {
            Ok(Self::Quit)
        } else if prefix.eq_ignore_ascii_case("NOOP") {
            if trimmed.len() == 4 {
                Ok(Self::Noop(None))
            } else {
                let arg = trimmed[4..].trim();
                if arg.is_empty() {
                    Ok(Self::Noop(None))
                } else {
                    Ok(Self::Noop(Some(arg.to_string())))
                }
            }
        } else if prefix.eq_ignore_ascii_case("VRFY") {
            match trimmed.split_once(' ') {
                Some((_, arg)) if !arg.trim().is_empty() => Ok(Self::Vrfy(arg.trim().to_string())),
                _ => Err(Self::Invalid(format!("Expected argument in {trimmed}"))),
            }
        } else if prefix.eq_ignore_ascii_case("EXPN") {
            match trimmed.split_once(' ') {
                Some((_, arg)) if !arg.trim().is_empty() => Ok(Self::Expn(arg.trim().to_string())),
                _ => Err(Self::Invalid(format!("Expected argument in {trimmed}"))),
            }
        } else if prefix.eq_ignore_ascii_case("ETRN") {
            match trimmed.split_once(' ') {
                Some((_, arg)) if !arg.trim().is_empty() => Ok(Self::Etrn(arg.trim().to_string())),
                _ => Err(Self::Invalid(format!("Expected argument in {trimmed}"))),
            }
        } else if trimmed.eq_ignore_ascii_case("TURN") {
            Ok(Self::Turn)
        } else if trimmed.len() >= 8 && trimmed[..8].eq_ignore_ascii_case("STARTTLS") {
            Ok(Self::StartTLS)
        } else if trimmed.eq_ignore_ascii_case("HELP") {
            Ok(Self::Help)
        } else if prefix.eq_ignore_ascii_case("AUTH") {
            let mut parts = trimmed.split_whitespace();
            let _ = parts.next();
            let Some(mechanism) = parts.next() else {
                return Some(Err(Self::Invalid(String::from(
                    "Expected mechanism in AUTH command",
                ))));
            };
            let initial_response = parts.next().map(str::to_string);
            if parts.next().is_some() {
                return Some(Err(Self::Invalid(String::from(
                    "Too many arguments in AUTH command",
                ))));
            }

            Ok(Self::Auth {
                mechanism: mechanism.to_ascii_uppercase(),
                initial_response,
            })
        } else if trimmed.eq_ignore_ascii_case("RSET") {
            Ok(Self::Rset)
        } else {
            Err(Self::Invalid(command.to_owned()))
        };

        Some(command)
    }
}

impl Display for Command {
    fn fmt(&self, fmt: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Helo(v) => fmt.write_fmt(format_args!("{} {}", v, self.inner())),
            Self::MailFrom(s, params) => {
                let addr = s.as_ref().map_or_else(String::new, ToString::to_string);
                if params.is_empty() {
                    fmt.write_fmt(format_args!("MAIL FROM:{addr}"))
                } else {
                    fmt.write_fmt(format_args!("MAIL FROM:{addr} {params}"))
                }
            }
            Self::RcptTo(rcpt) => fmt.write_fmt(format_args!("RCPT TO:{rcpt}")),
            Self::Data => fmt.write_str("DATA"),
            Self::Quit => fmt.write_str("QUIT"),
            Self::Noop(arg) => {
                if let Some(arg) = arg {
                    fmt.write_fmt(format_args!("NOOP {arg}"))
                } else {
                    fmt.write_str("NOOP")
                }
            }
            Self::Vrfy(arg) => fmt.write_fmt(format_args!("VRFY {arg}")),
            Self::Expn(arg) => fmt.write_fmt(format_args!("EXPN {arg}")),
            Self::Etrn(arg) => fmt.write_fmt(format_args!("ETRN {arg}")),
            Self::Turn => fmt.write_str("TURN"),
            Self::Xclient(params) => {
                if params.params.is_empty() {
                    fmt.write_str("XCLIENT")
                } else {
                    fmt.write_str("XCLIENT")?;
                    for (k, v) in params.iter() {
                        fmt.write_str(" ")?;
                        fmt.write_fmt(format_args!("{k}={v}"))?;
                    }
                    Ok(())
                }
            }
            Self::StartTLS => fmt.write_str("STARTTLS"),
            Self::Invalid(s) => fmt.write_str(s),
            Self::Help => fmt.write_str("HELP"),
            Self::Rset => fmt.write_str("Rset"),
            Self::Auth {
                mechanism,
                initial_response,
            } => {
                if let Some(initial_response) = initial_response {
                    fmt.write_fmt(format_args!("AUTH {mechanism} {initial_response}"))
                } else {
                    fmt.write_fmt(format_args!("AUTH {mechanism}"))
                }
            }
        }
    }
}

impl TryFrom<&str> for Command {
    type Error = Self;

    fn try_from(command: &str) -> Result<Self, Self::Error> {
        let trimmed = command.trim();

        if let Some(result) = Self::parse_mail_from(trimmed, command) {
            return result;
        }
        if let Some(result) = Self::parse_rcpt_to(trimmed, command) {
            return result;
        }
        if let Some(result) = Self::parse_xclient(trimmed) {
            return result;
        }
        if let Some(result) = Self::parse_simple_command(trimmed, command) {
            return result;
        }

        Err(Self::Invalid(command.to_owned()))
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
    use empath_common::{
        address::{Address, AddressList},
        address_parser,
    };

    use crate::command::{Command, HeloVariant, MailParameters, XclientParameters};

    type TestResult = std::result::Result<(), String>;

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
    fn mail_from_command() -> TestResult {
        let expected_mailbox = address_parser::parse_forward_path("<test@gmail.com>")
            .map_err(|err| format!("{err:?}"))?;
        assert_eq!(
            Command::try_from("Mail From: <test@gmail.com>"),
            Ok(Command::MailFrom(
                Some(Address::from(expected_mailbox)),
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
        let expected_mailbox2 = address_parser::parse_forward_path("<test@gmail.com>")
            .map_err(|err| format!("{err:?}"))?;
        assert_eq!(
            Command::try_from("MAIL FROM: <test@gmail.com> SIZE=12345"),
            Ok(Command::MailFrom(
                Some(Address::from(expected_mailbox2)),
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
        Ok(())
    }

    #[test]
    fn mail_from_size_edge_cases() -> TestResult {
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
        let mailbox1 = address_parser::parse_forward_path("<test@example.com>")
            .map_err(|err| format!("{err:?}"))?;
        assert_eq!(
            Command::try_from("MAIL FROM: <test@example.com> size=5000"),
            Ok(Command::MailFrom(
                Some(Address::from(mailbox1)),
                params_lower
            ))
        );

        let mut params_mixed = MailParameters::new();
        params_mixed.insert("SIZE", "3000");
        let mailbox2 = address_parser::parse_forward_path("<test@example.com>")
            .map_err(|err| format!("{err:?}"))?;
        assert_eq!(
            Command::try_from("MAIL FROM: <test@example.com> SiZe=3000"),
            Ok(Command::MailFrom(
                Some(Address::from(mailbox2)),
                params_mixed
            ))
        );

        // SIZE with other ESMTP parameters
        let mut params_multi = MailParameters::new();
        params_multi.insert("SIZE", "1000");
        params_multi.insert("BODY", "8BITMIME");
        let mailbox3 = address_parser::parse_forward_path("<test@example.com>")
            .map_err(|err| format!("{err:?}"))?;
        assert_eq!(
            Command::try_from("MAIL FROM: <test@example.com> SIZE=1000 BODY=8BITMIME"),
            Ok(Command::MailFrom(
                Some(Address::from(mailbox3)),
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
        Ok(())
    }

    #[test]
    fn rcpt_to_command() -> TestResult {
        let mailbox = address_parser::parse_forward_path("<test@gmail.com>")
            .map_err(|err| format!("{err:?}"))?;
        assert_eq!(
            Command::try_from("Rcpt To: <test@gmail.com>"),
            Ok(Command::RcptTo(AddressList::from(vec![Address::from(
                mailbox
            )])))
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
        Ok(())
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

        assert!(Command::try_from("AUTH").is_err());
        for comm in string_casing("auth") {
            assert!(Command::try_from(comm).is_err());
        }
        assert_eq!(
            Command::try_from("AUTH PLAIN"),
            Ok(Command::Auth {
                mechanism: "PLAIN".to_string(),
                initial_response: None,
            })
        );
        assert_eq!(
            Command::try_from("AUTH PLAIN dGVzdA=="),
            Ok(Command::Auth {
                mechanism: "PLAIN".to_string(),
                initial_response: Some("dGVzdA==".to_string()),
            })
        );

        assert_eq!(Command::try_from("HELP"), Ok(Command::Help));
        for comm in string_casing("help") {
            assert_eq!(Command::try_from(comm), Ok(Command::Help));
        }

        assert_eq!(Command::try_from("NOOP"), Ok(Command::Noop(None)));
        assert_eq!(
            Command::try_from("NOOP keepalive"),
            Ok(Command::Noop(Some("keepalive".to_string())))
        );

        assert_eq!(
            Command::try_from("VRFY user@example.com"),
            Ok(Command::Vrfy("user@example.com".to_string()))
        );
        assert_eq!(
            Command::try_from("EXPN staff"),
            Ok(Command::Expn("staff".to_string()))
        );
        assert_eq!(
            Command::try_from("ETRN example.com"),
            Ok(Command::Etrn("example.com".to_string()))
        );
        assert_eq!(Command::try_from("TURN"), Ok(Command::Turn));

        let mut xclient = XclientParameters::new();
        xclient
            .params
            .insert("ADDR".to_string(), "192.0.2.1".to_string());
        xclient
            .params
            .insert("NAME".to_string(), "proxy.example.com".to_string());
        assert_eq!(
            Command::try_from("XCLIENT ADDR=192.0.2.1 NAME=proxy.example.com"),
            Ok(Command::Xclient(xclient))
        );
    }
}
