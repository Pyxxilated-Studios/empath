use std::num::NonZeroUsize;

use thiserror::Error;

/// Errors that can occur during message parsing.
#[derive(Debug, Error)]
pub enum MessageParseError {
    /// End of body marker not found.
    #[error("Could not find end of data")]
    EndOfBodyNotFound,

    /// Invalid UTF-8 in message headers.
    #[error("Invalid UTF-8 in headers: {0}")]
    InvalidUtf8(#[from] std::str::Utf8Error),

    /// Invalid message structure.
    #[error("Invalid message structure: {0}")]
    InvalidStructure(String),

    /// I/O error during parsing.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Not enough bytes left")]
    UnexpectedEOF(NonZeroUsize),
}
