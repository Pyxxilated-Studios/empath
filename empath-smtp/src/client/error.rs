//! Error types for the SMTP client.

use std::io;

use thiserror::Error;

/// Errors that can occur when using the SMTP client.
#[derive(Error, Debug)]
pub enum ClientError {
    /// IO error occurred during network operations.
    #[error("IO error: {0}")]
    Io(#[from] io::Error),

    /// Failed to parse an SMTP response from the server.
    #[error("Failed to parse SMTP response: {0}")]
    ParseError(String),

    /// The server returned an unexpected SMTP status code.
    #[error("Unexpected SMTP status code: {code} - {message}")]
    UnexpectedResponse { code: u16, message: String },

    /// The server returned an error status code (4xx or 5xx).
    #[error("SMTP error: {code} - {message}")]
    SmtpError { code: u16, message: String },

    /// TLS/SSL error occurred.
    #[error("TLS error: {0}")]
    TlsError(String),

    /// Invalid builder configuration.
    #[error("Invalid builder configuration: {0}")]
    BuilderError(String),

    /// Connection was closed unexpectedly.
    #[error("Connection closed unexpectedly")]
    ConnectionClosed,

    /// UTF-8 decoding error.
    #[error("UTF-8 error: {0}")]
    Utf8Error(#[from] std::str::Utf8Error),
}

/// Specialized `Result` type for SMTP client operations.
pub type Result<T> = anyhow::Result<T, ClientError>;
