//! SMTP response parsing and representation.

use super::error::{ClientError, Result};

/// Represents a single line in an SMTP response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResponseLine {
    /// The SMTP status code (e.g., 220, 250, 550).
    pub code: u16,
    /// Whether this is the last line in a multi-line response.
    pub is_last: bool,
    /// The message text following the status code.
    pub message: String,
}

/// Represents a complete SMTP response, which may be multi-line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    /// The SMTP status code.
    pub code: u16,
    /// All message lines in the response.
    pub lines: Vec<String>,
}

impl Response {
    /// Creates a new `Response`.
    #[must_use]
    pub const fn new(code: u16, lines: Vec<String>) -> Self {
        Self { code, lines }
    }

    /// Returns the complete message as a single string with lines joined by newlines.
    #[must_use]
    pub fn message(&self) -> String {
        self.lines.join("\n")
    }

    /// Returns `true` if this response indicates success (2xx code).
    #[must_use]
    pub const fn is_success(&self) -> bool {
        self.code >= 200 && self.code < 300
    }

    /// Returns `true` if this response indicates a temporary error (4xx code).
    #[must_use]
    pub const fn is_temporary_error(&self) -> bool {
        self.code >= 400 && self.code < 500
    }

    /// Returns `true` if this response indicates a permanent error (5xx code).
    #[must_use]
    pub const fn is_permanent_error(&self) -> bool {
        self.code >= 500 && self.code < 600
    }

    /// Returns `true` if this response indicates any error (4xx or 5xx code).
    #[must_use]
    pub const fn is_error(&self) -> bool {
        self.is_temporary_error() || self.is_permanent_error()
    }

    /// Parses a single response line.
    ///
    /// # Errors
    ///
    /// Returns `ClientError::ParseError` if the line doesn't match SMTP format.
    pub fn parse_line(line: &str) -> Result<ResponseLine> {
        if line.len() < 3 {
            return Err(ClientError::ParseError(format!(
                "Response line too short: '{line}'"
            )));
        }

        let code_str = &line[..3];
        let code = code_str.parse::<u16>().map_err(|_| {
            ClientError::ParseError(format!("Invalid status code: '{code_str}'"))
        })?;

        // Check if this is the last line (indicated by a space) or continuation (dash)
        let is_last = if line.len() > 3 {
            match line.chars().nth(3) {
                Some(' ') => true,
                Some('-') => false,
                Some(c) => {
                    return Err(ClientError::ParseError(format!(
                        "Invalid separator character: '{c}'"
                    )))
                }
                None => true, // Exactly 3 characters
            }
        } else {
            true
        };

        let message = if line.len() > 4 {
            line[4..].to_string()
        } else {
            String::new()
        };

        Ok(ResponseLine {
            code,
            is_last,
            message,
        })
    }

    /// Parses a complete multi-line SMTP response from a buffer.
    ///
    /// Returns the parsed `Response` and the number of bytes consumed.
    ///
    /// # Errors
    ///
    /// Returns `ClientError::ParseError` if the response is malformed.
    pub fn parse_response(buffer: &[u8]) -> Result<Option<(Self, usize)>> {
        let text = std::str::from_utf8(buffer)?;
        let mut lines = Vec::new();
        let mut bytes_consumed = 0;
        let mut first_code = None;
        let mut complete = false;

        for line in text.lines() {
            let line_with_crlf = if text[bytes_consumed..].starts_with(line) {
                bytes_consumed += line.len();
                // Check for CRLF or LF
                if text[bytes_consumed..].starts_with("\r\n") {
                    bytes_consumed += 2;
                } else if text[bytes_consumed..].starts_with('\n') {
                    bytes_consumed += 1;
                } else {
                    // Incomplete line
                    break;
                }
                line
            } else {
                break;
            };

            if line_with_crlf.is_empty() {
                continue;
            }

            let parsed_line = Self::parse_line(line_with_crlf)?;

            if let Some(code) = first_code {
                if parsed_line.code != code {
                    return Err(ClientError::ParseError(format!(
                        "Status code mismatch in multi-line response: expected {code}, got {}",
                        parsed_line.code
                    )));
                }
            } else {
                first_code = Some(parsed_line.code);
            }

            lines.push(parsed_line.message);

            if parsed_line.is_last {
                complete = true;
                break;
            }
        }

        if complete {
            if let Some(code) = first_code {
                Ok(Some((Self::new(code, lines), bytes_consumed)))
            } else {
                Ok(None)
            }
        } else {
            Ok(None) // Need more data
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_single_line() {
        let line = ResponseLine {
            code: 220,
            is_last: true,
            message: "mail.example.com ESMTP".to_string(),
        };
        assert_eq!(
            Response::parse_line("220 mail.example.com ESMTP").unwrap(),
            line
        );
    }

    #[test]
    fn test_parse_multi_line_indicator() {
        let line = ResponseLine {
            code: 250,
            is_last: false,
            message: "mail.example.com".to_string(),
        };
        assert_eq!(Response::parse_line("250-mail.example.com").unwrap(), line);
    }

    #[test]
    fn test_parse_complete_response() {
        let data = b"250 OK\r\n";
        let (response, consumed) = Response::parse_response(data).unwrap().unwrap();
        assert_eq!(response.code, 250);
        assert_eq!(response.lines, vec!["OK"]);
        assert_eq!(consumed, 8);
    }

    #[test]
    fn test_parse_multi_line_response() {
        let data = b"250-mail.example.com\r\n250-SIZE 10000000\r\n250 HELP\r\n";
        let (response, consumed) = Response::parse_response(data).unwrap().unwrap();
        assert_eq!(response.code, 250);
        assert_eq!(
            response.lines,
            vec!["mail.example.com", "SIZE 10000000", "HELP"]
        );
        assert_eq!(consumed, 51); // 22 + 19 + 10 = 51
    }

    #[test]
    fn test_parse_incomplete_response() {
        let data = b"250-mail.example.com\r\n250-SIZE";
        let result = Response::parse_response(data).unwrap();
        assert!(result.is_none()); // Need more data
    }

    #[test]
    fn test_is_success() {
        let response = Response::new(250, vec!["OK".to_string()]);
        assert!(response.is_success());
        assert!(!response.is_error());
    }

    #[test]
    fn test_is_error() {
        let response = Response::new(550, vec!["Error".to_string()]);
        assert!(response.is_permanent_error());
        assert!(response.is_error());
        assert!(!response.is_success());
    }
}
