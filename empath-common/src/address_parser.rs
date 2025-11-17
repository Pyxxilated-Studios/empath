//! RFC 5321 compliant SMTP address parser
//!
//! This module implements a parser for SMTP addresses according to RFC 5321 Section 4.1.2.
//! It replaces the use of mailparse for parsing MAIL FROM and RCPT TO addresses with
//! a strict RFC-compliant implementation.
//!
//! # ABNF Grammar (RFC 5321 Section 4.1.2)
//!
//! ```text
//! Reverse-path   = Path / "<>"
//! Forward-path   = Path
//! Path           = "<" [ A-d-l ":" ] Mailbox ">"
//! Mailbox        = Local-part "@" ( Domain / address-literal )
//! Local-part     = Dot-string / Quoted-string
//! Dot-string     = Atom *("." Atom)
//! Atom           = 1*atext
//! Quoted-string  = DQUOTE *QcontentSMTP DQUOTE
//! Domain         = sub-domain *("." sub-domain)
//! sub-domain     = Let-dig [Ldh-str]
//! address-literal = "[" ( IPv4-address-literal / IPv6-address-literal / General-address-literal ) "]"
//!
//! atext          = ALPHA / DIGIT / "!" / "#" / "$" / "%" / "&" / "'" /
//!                  "*" / "+" / "-" / "/" / "=" / "?" / "^" / "_" / "`" /
//!                  "{" / "|" / "}" / "~"
//! Let-dig        = ALPHA / DIGIT
//! Ldh-str        = *( ALPHA / DIGIT / "-" ) Let-dig
//! QcontentSMTP   = qtextSMTP / quoted-pairSMTP
//! qtextSMTP      = %d32-33 / %d35-91 / %d93-126
//! quoted-pairSMTP = %d92 %d32-126  ; backslash followed by any ASCII graphic
//! ```
//!
//! # Size Constraints
//!
//! - Maximum path length: 256 octets (including angle brackets and punctuation)
//! - Maximum local-part: 64 octets
//! - Maximum domain: 255 octets

use std::net::{Ipv4Addr, Ipv6Addr};

/// Result type for address parsing
pub type Result<T> = std::result::Result<T, AddressError>;

/// Errors that can occur during address parsing
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddressError {
    /// Empty input
    Empty,
    /// Path exceeds 256 octets
    PathTooLong,
    /// Local-part exceeds 64 octets
    LocalPartTooLong,
    /// Domain exceeds 255 octets
    DomainTooLong,
    /// Missing opening angle bracket
    MissingOpenBracket,
    /// Missing closing angle bracket
    MissingCloseBracket,
    /// Missing '@' separator in mailbox
    MissingAtSign,
    /// Invalid character in local-part
    InvalidLocalPart(String),
    /// Invalid character in domain
    InvalidDomain(String),
    /// Invalid address literal format
    InvalidAddressLiteral(String),
    /// Unclosed quoted string
    UnclosedQuotedString,
    /// Invalid character in quoted string
    InvalidQuotedString(String),
}

impl std::fmt::Display for AddressError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => write!(f, "Empty address"),
            Self::PathTooLong => write!(f, "Path exceeds 256 octets"),
            Self::LocalPartTooLong => write!(f, "Local-part exceeds 64 octets"),
            Self::DomainTooLong => write!(f, "Domain exceeds 255 octets"),
            Self::MissingOpenBracket => write!(f, "Missing opening angle bracket '<'"),
            Self::MissingCloseBracket => write!(f, "Missing closing angle bracket '>'"),
            Self::MissingAtSign => write!(f, "Missing '@' separator in mailbox"),
            Self::InvalidLocalPart(s) => write!(f, "Invalid local-part: {s}"),
            Self::InvalidDomain(s) => write!(f, "Invalid domain: {s}"),
            Self::InvalidAddressLiteral(s) => write!(f, "Invalid address literal: {s}"),
            Self::UnclosedQuotedString => write!(f, "Unclosed quoted string in local-part"),
            Self::InvalidQuotedString(s) => write!(f, "Invalid quoted string: {s}"),
        }
    }
}

impl std::error::Error for AddressError {}

/// A parsed SMTP mailbox (local-part@domain)
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Mailbox {
    /// The local part (before @)
    pub local_part: String,
    /// The domain or address literal (after @)
    pub domain: String,
}

impl std::fmt::Display for Mailbox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{}", self.local_part, self.domain)
    }
}

/// Parse a reverse-path (MAIL FROM argument)
///
/// Accepts either `<mailbox>` or `<>` (null sender).
///
/// # Errors
///
/// Returns `AddressError` if the input is not a valid reverse-path according to RFC 5321.
pub fn parse_reverse_path(input: &str) -> Result<Option<Mailbox>> {
    let trimmed = input.trim();

    if trimmed.is_empty() {
        return Err(AddressError::Empty);
    }

    // Check size constraint
    if trimmed.len() > 256 {
        return Err(AddressError::PathTooLong);
    }

    // Handle null sender
    if trimmed == "<>" {
        return Ok(None);
    }

    // Parse as Path
    parse_path(trimmed).map(Some)
}

/// Parse a forward-path (RCPT TO argument)
///
/// Must be `<mailbox>`.
///
/// # Errors
///
/// Returns `AddressError` if the input is not a valid forward-path according to RFC 5321.
pub fn parse_forward_path(input: &str) -> Result<Mailbox> {
    let trimmed = input.trim();

    if trimmed.is_empty() {
        return Err(AddressError::Empty);
    }

    // Check size constraint
    if trimmed.len() > 256 {
        return Err(AddressError::PathTooLong);
    }

    parse_path(trimmed)
}

/// Parse a Path: `<mailbox>` or `<source-route:mailbox>`
///
/// Note: Source routing is deprecated but we accept the syntax for backwards compatibility.
fn parse_path(input: &str) -> Result<Mailbox> {
    // Must start with '<'
    if !input.starts_with('<') {
        return Err(AddressError::MissingOpenBracket);
    }

    // Must end with '>'
    if !input.ends_with('>') {
        return Err(AddressError::MissingCloseBracket);
    }

    // Extract content between angle brackets
    let content = &input[1..input.len() - 1];

    // Check for source routing (deprecated but accepted): @domain1,@domain2:mailbox
    // We need to find a colon that's NOT inside square brackets (address literal)
    let mailbox_str =
        find_source_route_colon(content).map_or(content, |colon_pos| &content[colon_pos + 1..]);

    parse_mailbox(mailbox_str)
}

/// Find the position of ':' for source routing (not inside brackets)
fn find_source_route_colon(input: &str) -> Option<usize> {
    let mut in_brackets = false;
    let mut last_colon: Option<usize> = None;

    for (i, ch) in input.chars().enumerate() {
        if ch == '[' {
            in_brackets = true;
        } else if ch == ']' {
            in_brackets = false;
        } else if ch == ':' && !in_brackets {
            last_colon = Some(i);
        }
    }

    last_colon
}

/// Parse a Mailbox: `local-part@domain` or `local-part@[address-literal]`
fn parse_mailbox(input: &str) -> Result<Mailbox> {
    // Find the '@' separator
    // We need to be careful about quoted strings which may contain '@'
    let at_pos = find_unquoted_at(input)?;

    let local_part = &input[..at_pos];
    let domain = &input[at_pos + 1..];

    // Validate size constraints
    if local_part.len() > 64 {
        return Err(AddressError::LocalPartTooLong);
    }
    if domain.len() > 255 {
        return Err(AddressError::DomainTooLong);
    }

    // Parse and validate local-part
    let local = parse_local_part(local_part)?;

    // Parse and validate domain or address-literal
    let dom = parse_domain_or_address_literal(domain)?;

    Ok(Mailbox {
        local_part: local,
        domain: dom,
    })
}

/// Find the position of '@' that is not inside a quoted string or address literal
fn find_unquoted_at(input: &str) -> Result<usize> {
    let mut in_quotes = false;
    let mut in_brackets = false;
    let mut prev_was_backslash = false;

    for (i, ch) in input.chars().enumerate() {
        if ch == '"' && !prev_was_backslash && !in_brackets {
            in_quotes = !in_quotes;
        } else if ch == '[' && !in_quotes {
            in_brackets = true;
        } else if ch == ']' && !in_quotes {
            in_brackets = false;
        } else if ch == '@' && !in_quotes && !in_brackets {
            return Ok(i);
        }

        prev_was_backslash = ch == '\\' && !prev_was_backslash;
    }

    Err(AddressError::MissingAtSign)
}

/// Parse a local-part: Dot-string or Quoted-string
fn parse_local_part(input: &str) -> Result<String> {
    if input.is_empty() {
        return Err(AddressError::InvalidLocalPart(
            "Empty local-part".to_string(),
        ));
    }

    // Check if it's a quoted string
    if input.starts_with('"') {
        parse_quoted_string(input)
    } else {
        parse_dot_string(input)
    }
}

/// Parse a Dot-string: Atom *("." Atom)
fn parse_dot_string(input: &str) -> Result<String> {
    if input.is_empty() {
        return Err(AddressError::InvalidLocalPart(
            "Empty dot-string".to_string(),
        ));
    }

    // Cannot start or end with dot
    if input.starts_with('.') || input.ends_with('.') {
        return Err(AddressError::InvalidLocalPart(
            "Dot-string cannot start or end with '.'".to_string(),
        ));
    }

    // Cannot have consecutive dots
    if input.contains("..") {
        return Err(AddressError::InvalidLocalPart(
            "Dot-string cannot contain consecutive dots".to_string(),
        ));
    }

    // Split on dots and validate each atom
    for atom in input.split('.') {
        if atom.is_empty() {
            return Err(AddressError::InvalidLocalPart(
                "Empty atom in dot-string".to_string(),
            ));
        }

        // Validate atom characters (atext)
        for ch in atom.chars() {
            if !is_atext(ch) {
                return Err(AddressError::InvalidLocalPart(format!(
                    "Invalid character '{ch}' in atom"
                )));
            }
        }
    }

    Ok(input.to_string())
}

/// Parse a Quoted-string: DQUOTE *`QcontentSMTP` DQUOTE
fn parse_quoted_string(input: &str) -> Result<String> {
    if !input.starts_with('"') {
        return Err(AddressError::InvalidQuotedString(
            "Quoted string must start with '\"'".to_string(),
        ));
    }

    if !input.ends_with('"') || input.len() < 2 {
        return Err(AddressError::UnclosedQuotedString);
    }

    // Extract content between quotes
    let content = &input[1..input.len() - 1];

    // Validate quoted content (qtextSMTP / quoted-pairSMTP)
    let mut chars = content.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            // quoted-pair: backslash followed by any ASCII graphic
            if let Some(next_ch) = chars.next() {
                if !next_ch.is_ascii_graphic() && next_ch != ' ' {
                    return Err(AddressError::InvalidQuotedString(format!(
                        "Invalid quoted-pair: \\{next_ch}"
                    )));
                }
            } else {
                return Err(AddressError::InvalidQuotedString(
                    "Backslash at end of quoted string".to_string(),
                ));
            }
        } else if !is_qtext_smtp(ch) {
            return Err(AddressError::InvalidQuotedString(format!(
                "Invalid character '{ch}' in quoted string"
            )));
        }
    }

    Ok(input.to_string())
}

/// Parse domain or address-literal
fn parse_domain_or_address_literal(input: &str) -> Result<String> {
    if input.starts_with('[') {
        parse_address_literal(input)
    } else {
        parse_domain(input)
    }
}

/// Parse a Domain: sub-domain *("." sub-domain)
fn parse_domain(input: &str) -> Result<String> {
    if input.is_empty() {
        return Err(AddressError::InvalidDomain("Empty domain".to_string()));
    }

    // Cannot start or end with dot
    if input.starts_with('.') || input.ends_with('.') {
        return Err(AddressError::InvalidDomain(
            "Domain cannot start or end with '.'".to_string(),
        ));
    }

    // Cannot have consecutive dots
    if input.contains("..") {
        return Err(AddressError::InvalidDomain(
            "Domain cannot contain consecutive dots".to_string(),
        ));
    }

    // Split on dots and validate each subdomain
    for subdomain in input.split('.') {
        parse_subdomain(subdomain)?;
    }

    Ok(input.to_string())
}

/// Parse a sub-domain: Let-dig [Ldh-str]
fn parse_subdomain(input: &str) -> Result<()> {
    if input.is_empty() {
        return Err(AddressError::InvalidDomain("Empty subdomain".to_string()));
    }

    // Must start with letter or digit
    if input
        .chars()
        .next()
        .is_none_or(|first| !first.is_ascii_alphanumeric())
    {
        return Err(AddressError::InvalidDomain(format!(
            "Subdomain must start with letter or digit{}",
            input
                .chars()
                .next()
                .map_or_else(String::default, |first| format!(", got '{first}'"))
        )));
    }

    // Must end with letter or digit
    if input
        .chars()
        .last()
        .is_none_or(|last| !last.is_ascii_alphanumeric())
    {
        return Err(AddressError::InvalidDomain(format!(
            "Subdomain must end with letter or digit{}",
            input
                .chars()
                .last()
                .map_or_else(String::default, |last| format!(", got '{last}'"))
        )));
    }

    // Middle characters can be letter, digit, or hyphen
    for ch in input.chars() {
        if !ch.is_ascii_alphanumeric() && ch != '-' {
            return Err(AddressError::InvalidDomain(format!(
                "Invalid character '{ch}' in subdomain"
            )));
        }
    }

    Ok(())
}

/// Parse an address-literal: `[IPv4]` or `[IPv6:...]` or `[tag:...]`
fn parse_address_literal(input: &str) -> Result<String> {
    if !input.starts_with('[') || !input.ends_with(']') {
        return Err(AddressError::InvalidAddressLiteral(
            "Address literal must be enclosed in brackets".to_string(),
        ));
    }

    let content = &input[1..input.len() - 1];

    // Try to parse as IPv4
    if let Ok(_addr) = content.parse::<Ipv4Addr>() {
        return Ok(input.to_string());
    }

    // Try to parse as IPv6 (with IPv6: prefix)
    if let Some(ipv6_str) = content.strip_prefix("IPv6:")
        && ipv6_str.parse::<Ipv6Addr>().is_ok()
    {
        return Ok(input.to_string());
    }

    // General address literal: tag:value
    // We accept any ASCII printable characters after the tag
    if content.contains(':') {
        let parts: Vec<&str> = content.splitn(2, ':').collect();
        if parts.len() == 2 && !parts[0].is_empty() && !parts[1].is_empty() {
            return Ok(input.to_string());
        }
    }

    Err(AddressError::InvalidAddressLiteral(format!(
        "Invalid address literal format: {content}"
    )))
}

/// Check if character is valid atext (atom text)
///
/// atext = ALPHA / DIGIT / "!" / "#" / "$" / "%" / "&" / "'" /
///         "*" / "+" / "-" / "/" / "=" / "?" / "^" / "_" / "\`" /
///         "{" / "|" / "}" / "~"
#[inline]
const fn is_atext(ch: char) -> bool {
    ch.is_ascii_alphanumeric()
        || matches!(
            ch,
            '!' | '#'
                | '$'
                | '%'
                | '&'
                | '\''
                | '*'
                | '+'
                | '-'
                | '/'
                | '='
                | '?'
                | '^'
                | '_'
                | '`'
                | '{'
                | '|'
                | '}'
                | '~'
        )
}

/// Check if character is valid qtextSMTP (quoted text for SMTP)
///
/// qtextSMTP = %d32-33 / %d35-91 / %d93-126
/// (printable ASCII except backslash and quote)
#[inline]
const fn is_qtext_smtp(ch: char) -> bool {
    matches!(ch as u8, 32..=33 | 35..=91 | 93..=126)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_null_sender() {
        assert_eq!(parse_reverse_path("<>").unwrap(), None);
        assert_eq!(parse_reverse_path(" <> ").unwrap(), None);
    }

    #[test]
    fn test_parse_simple_mailbox() {
        let result = parse_forward_path("<user@example.com>").unwrap();
        assert_eq!(result.local_part, "user");
        assert_eq!(result.domain, "example.com");
    }

    #[test]
    fn test_parse_dotted_local_part() {
        let result = parse_forward_path("<first.last@example.com>").unwrap();
        assert_eq!(result.local_part, "first.last");
    }

    #[test]
    fn test_parse_quoted_local_part() {
        let result = parse_forward_path(r#"<"user name"@example.com>"#).unwrap();
        assert_eq!(result.local_part, r#""user name""#);
    }

    #[test]
    fn test_parse_address_literal_ipv4() {
        let result = parse_forward_path("<user@[192.168.1.1]>").unwrap();
        assert_eq!(result.domain, "[192.168.1.1]");
    }

    #[test]
    fn test_parse_address_literal_ipv6() {
        let result = parse_forward_path("<user@[IPv6:2001:db8::1]>").unwrap();
        assert_eq!(result.domain, "[IPv6:2001:db8::1]");
    }

    #[test]
    fn test_invalid_missing_brackets() {
        assert!(parse_forward_path("user@example.com").is_err());
    }

    #[test]
    fn test_invalid_missing_at() {
        assert!(parse_forward_path("<userexample.com>").is_err());
    }

    #[test]
    fn test_invalid_consecutive_dots() {
        assert!(parse_forward_path("<user..name@example.com>").is_err());
    }

    #[test]
    fn test_invalid_domain_start_with_dot() {
        assert!(parse_forward_path("<user@.example.com>").is_err());
    }

    #[test]
    fn test_invalid_domain_end_with_hyphen() {
        assert!(parse_forward_path("<user@example-.com>").is_err());
    }

    #[test]
    fn test_path_too_long() {
        let long_path = format!("<{}@example.com>", "a".repeat(300));
        assert_eq!(
            parse_forward_path(&long_path).unwrap_err(),
            AddressError::PathTooLong
        );
    }

    #[test]
    fn test_local_part_too_long() {
        let long_local = format!("<{}@example.com>", "a".repeat(70));
        assert_eq!(
            parse_forward_path(&long_local).unwrap_err(),
            AddressError::LocalPartTooLong
        );
    }

    #[test]
    fn test_source_routing_ignored() {
        // Source routing is deprecated but we should accept it
        let result = parse_forward_path("<@relay1.com,@relay2.com:user@example.com>").unwrap();
        assert_eq!(result.local_part, "user");
        assert_eq!(result.domain, "example.com");
    }

    #[test]
    fn test_special_chars_in_local_part() {
        let result = parse_forward_path("<user+tag@example.com>").unwrap();
        assert_eq!(result.local_part, "user+tag");
    }

    #[test]
    fn test_quoted_pair_in_local_part() {
        let result = parse_forward_path(r#"<"user\"quote"@example.com>"#).unwrap();
        assert_eq!(result.local_part, r#""user\"quote""#);
    }

    #[test]
    fn test_local_part_single_dot() {
        // Test the exact input from issue: "MAIL FROM:<.@aaa.aa>"
        // Local part is just a dot, which should be invalid
        let result = parse_reverse_path("<.@aaa.aa>");
        println!("Result for <.@aaa.aa>: {result:?}");

        // This should return an error, not panic
        assert!(result.is_err());
        assert!(matches!(result, Err(AddressError::InvalidLocalPart(_))));
    }
}
