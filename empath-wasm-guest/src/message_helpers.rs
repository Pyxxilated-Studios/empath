use alloc::string::String;

/// Mutation failure surfaced from host helpers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageMutationError {
    /// Inputs were rejected by the host (invalid header/body data).
    InvalidInput,
    /// Host context was unavailable (e.g., poisoned lock or missing message).
    HostUnavailable,
}

pub type MutationResult = Result<(), MessageMutationError>;

/// Read failure surfaced from host helpers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageReadError {
    /// No message data was available in the host context.
    MissingData,
    /// Host failed to parse the message body into parts.
    ParseFailed,
    /// Requested offset/length was outside the message bounds.
    OutOfRange,
    /// Host context was unavailable.
    HostUnavailable,
    /// Host returned an unexpected empty chunk before EOF.
    Truncated,
}

pub type ReadResult<T> = Result<T, MessageReadError>;

/// Line ending style of the current message body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NewlineStyle {
    Crlf,
    Lf,
    Mixed,
    Unknown(String),
}

/// Convert host status codes (0 on success, negative on failure) into a `Result`.
///
/// # Errors
/// Returns `MessageMutationError::InvalidInput` when the host reports a non-zero status.
pub const fn map_status(status: i32) -> MutationResult {
    if status == 0 {
        Ok(())
    } else {
        Err(MessageMutationError::InvalidInput)
    }
}

/// Convert host `Result<T, ReadError>` into a typed read result.
///
/// # Errors
/// Returns a `MessageReadError` matching the host error.
pub fn map_read_result<T, E>(result: Result<T, E>) -> ReadResult<T>
where
    E: Into<MessageReadError>,
{
    result.map_err(Into::into)
}

/// Convert host `Result<bool, i32>` into a typed result.
///
/// # Errors
/// Returns `MessageMutationError::InvalidInput` when the host reports an error status.
pub fn map_status_bool(result: Result<bool, i32>) -> Result<bool, MessageMutationError> {
    result.map_err(|_| MessageMutationError::InvalidInput)
}

/// Normalize newline style strings emitted by the host.
#[must_use]
pub fn parse_newline_style(raw: &str) -> NewlineStyle {
    match raw {
        "crlf" => NewlineStyle::Crlf,
        "lf" => NewlineStyle::Lf,
        "mixed" => NewlineStyle::Mixed,
        other => NewlineStyle::Unknown(other.to_string()),
    }
}

impl AsRef<str> for NewlineStyle {
    fn as_ref(&self) -> &str {
        match self {
            Self::Crlf => "crlf",
            Self::Lf => "lf",
            Self::Mixed => "mixed",
            Self::Unknown(raw) => raw.as_str(),
        }
    }
}

/// Convert a newline style back into its raw string value for host interop.
#[must_use]
pub fn newline_style_to_str(style: &NewlineStyle) -> &str {
    style.as_ref()
}
