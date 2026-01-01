//! Error types for the SMTP client.

use std::io;

use thiserror::Error;

/// Errors that can occur when using the SMTP client.
///
/// Errors are categorized to enable proper retry logic:
/// - Temporary errors (4xx codes, timeouts, connection issues) can be retried
/// - Permanent errors (5xx codes, auth failures) should not be retried
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

impl ClientError {
    /// Returns `true` if this error is temporary and should be retried.
    ///
    /// Temporary errors include:
    /// - 4xx SMTP response codes (transient failures)
    /// - Connection failures and timeouts
    /// - Unexpected connection closures
    #[must_use]
    pub const fn is_temporary(&self) -> bool {
        match self {
            Self::SmtpError { code, .. } => *code >= 400 && *code < 500,
            Self::Io(_) | Self::ConnectionClosed => true,
            Self::ParseError(_)
            | Self::UnexpectedResponse { .. }
            | Self::TlsError(_)
            | Self::BuilderError(_)
            | Self::Utf8Error(_) => false,
        }
    }

    /// Returns `true` if this error is permanent and should not be retried.
    ///
    /// Permanent errors include:
    /// - 5xx SMTP response codes
    /// - Configuration errors
    /// - Protocol parsing errors
    #[must_use]
    pub const fn is_permanent(&self) -> bool {
        match self {
            Self::SmtpError { code, .. } => *code >= 500 && *code < 600,
            Self::BuilderError(_) | Self::ParseError(_) | Self::Utf8Error(_) => true,
            Self::Io(_)
            | Self::ConnectionClosed
            | Self::UnexpectedResponse { .. }
            | Self::TlsError(_) => false,
        }
    }

    /// Extract the SMTP response code if this error contains one.
    #[must_use]
    pub const fn response_code(&self) -> Option<u16> {
        match self {
            Self::SmtpError { code, .. } | Self::UnexpectedResponse { code, .. } => Some(*code),
            Self::Io(_)
            | Self::ParseError(_)
            | Self::TlsError(_)
            | Self::BuilderError(_)
            | Self::ConnectionClosed
            | Self::Utf8Error(_) => None,
        }
    }
}

/// Specialized `Result` type for SMTP client operations.
pub type Result<T> = std::result::Result<T, ClientError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_temporary_errors() {
        // 4xx SMTP codes are temporary
        let err = ClientError::SmtpError {
            code: 421,
            message: "Service not available".to_string(),
        };
        assert!(err.is_temporary());
        assert!(!err.is_permanent());
        assert_eq!(err.response_code(), Some(421));

        // I/O errors are temporary
        let err = ClientError::Io(io::Error::new(
            io::ErrorKind::ConnectionRefused,
            "connection refused",
        ));
        assert!(err.is_temporary());
        assert!(!err.is_permanent());

        // Connection closed is temporary
        let err = ClientError::ConnectionClosed;
        assert!(err.is_temporary());
        assert!(!err.is_permanent());
    }

    #[test]
    fn test_permanent_errors() {
        // 5xx SMTP codes are permanent
        let err = ClientError::SmtpError {
            code: 550,
            message: "User not found".to_string(),
        };
        assert!(!err.is_temporary());
        assert!(err.is_permanent());
        assert_eq!(err.response_code(), Some(550));

        // Builder errors are permanent
        let err = ClientError::BuilderError("Invalid config".to_string());
        assert!(!err.is_temporary());
        assert!(err.is_permanent());

        // Parse errors are permanent
        let err = ClientError::ParseError("Invalid response".to_string());
        assert!(!err.is_temporary());
        assert!(err.is_permanent());
    }

    #[test]
    fn test_response_code_extraction() {
        let err = ClientError::SmtpError {
            code: 250,
            message: "OK".to_string(),
        };
        assert_eq!(err.response_code(), Some(250));

        let err = ClientError::UnexpectedResponse {
            code: 999,
            message: "Unknown".to_string(),
        };
        assert_eq!(err.response_code(), Some(999));

        let err = ClientError::ConnectionClosed;
        assert_eq!(err.response_code(), None);
    }

    #[test]
    fn test_error_display() {
        let err = ClientError::SmtpError {
            code: 421,
            message: "Service not available".to_string(),
        };
        assert_eq!(err.to_string(), "SMTP error: 421 - Service not available");

        let err = ClientError::ConnectionClosed;
        assert_eq!(err.to_string(), "Connection closed unexpectedly");
    }
}
