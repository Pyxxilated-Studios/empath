//! Error types for the empath-smtp server.
//!
//! This module provides typed error handling for SMTP server operations including
//! connection handling, TLS upgrades, and protocol operations.

use std::io;

use thiserror::Error;

/// Errors that can occur during connection operations.
#[derive(Debug, Error)]
pub enum ConnectionError {
    /// I/O error during connection operations.
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    /// Error sending data to client.
    #[error("Failed to send data: {0}")]
    Send(String),

    /// Error receiving data from client.
    #[error("Failed to receive data: {0}")]
    Receive(String),

    /// Connection was closed by peer.
    #[error("Connection closed by peer")]
    Closed,

    /// Formatting error while preparing response.
    #[error("Response formatting error: {0}")]
    Format(#[from] std::fmt::Error),
}

/// Errors that can occur during TLS operations.
#[derive(Debug, Error)]
pub enum TlsError {
    /// I/O error during TLS operations.
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    /// Failed to load TLS certificate.
    #[error("Failed to load TLS certificate from {path}: {source}")]
    CertificateLoad {
        path: String,
        #[source]
        source: io::Error,
    },

    /// Failed to load TLS private key.
    #[error("Failed to load TLS private key from {path}: {reason}")]
    KeyLoad { path: String, reason: String },

    /// TLS handshake or upgrade failed.
    #[error("TLS upgrade failed: {0}")]
    UpgradeFailed(String),

    /// Rustls library error.
    #[error("TLS error: {0}")]
    Rustls(String),
}

impl From<tokio_rustls::rustls::Error> for TlsError {
    fn from(err: tokio_rustls::rustls::Error) -> Self {
        Self::Rustls(err.to_string())
    }
}

/// Specialized `Result` type for connection operations.
pub type ConnectionResult<T> = std::result::Result<T, ConnectionError>;

/// Specialized `Result` type for TLS operations.
pub type TlsResult<T> = std::result::Result<T, TlsError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_connection_error_display() {
        let err = ConnectionError::Send("timeout".to_string());
        assert_eq!(err.to_string(), "Failed to send data: timeout");

        let err = ConnectionError::Closed;
        assert_eq!(err.to_string(), "Connection closed by peer");
    }

    #[test]
    fn test_tls_error_display() {
        let err = TlsError::KeyLoad {
            path: "/path/to/key.pem".to_string(),
            reason: "invalid format".to_string(),
        };
        assert_eq!(
            err.to_string(),
            "Failed to load TLS private key from /path/to/key.pem: invalid format"
        );

        let err = TlsError::UpgradeFailed("handshake error".to_string());
        assert_eq!(err.to_string(), "TLS upgrade failed: handshake error");
    }

    #[test]
    fn test_io_error_conversion() {
        let io_err = io::Error::new(io::ErrorKind::ConnectionReset, "connection reset");
        let conn_err: ConnectionError = io_err.into();
        assert!(matches!(conn_err, ConnectionError::Io(_)));
    }

    #[test]
    fn test_format_error_conversion() {
        let fmt_err = std::fmt::Error;
        let conn_err: ConnectionError = fmt_err.into();
        assert!(matches!(conn_err, ConnectionError::Format(_)));
    }
}
