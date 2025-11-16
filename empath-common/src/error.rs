//! Error types for the empath-common crate.
//!
//! This module provides foundational error types used across all protocols
//! and session handlers in the Empath MTA.

use std::{io, num::NonZeroUsize};

use thiserror::Error;

/// Errors that can occur during protocol validation.
#[derive(Debug, Error)]
pub enum ProtocolError {
    /// A required configuration field is missing.
    #[error("Missing required field: {0}")]
    MissingField(&'static str),

    /// A configuration value is invalid.
    #[error("Invalid configuration for {field}: {reason}")]
    InvalidConfiguration { field: String, reason: String },

    /// Protocol-specific validation failed.
    #[error("Validation failed: {0}")]
    ValidationFailed(String),

    /// I/O error during protocol initialization (e.g., reading TLS certificates).
    #[error("I/O error during validation: {0}")]
    Io(#[from] io::Error),
}

/// Errors that can occur during session handling.
#[derive(Debug, Error)]
pub enum SessionError {
    /// Failed to initialize the session.
    #[error("Session initialization failed: {0}")]
    InitFailed(String),

    /// Protocol error occurred during session.
    #[error("Protocol error: {0}")]
    Protocol(String),

    /// Connection error occurred.
    #[error("Connection error: {0}")]
    Connection(#[from] io::Error),

    /// Session was cancelled (e.g., client disconnected).
    #[error("Session cancelled")]
    Cancelled,

    /// Shutdown signal received.
    #[error("Shutdown requested")]
    Shutdown,

    /// Session timed out.
    #[error("Session timed out after {0} seconds")]
    Timeout(u64),
}

impl SessionError {
    /// Returns `true` if the error indicates a graceful shutdown.
    #[must_use]
    pub const fn is_shutdown(&self) -> bool {
        matches!(self, Self::Shutdown | Self::Cancelled)
    }

    /// Returns `true` if the error is a client-side issue.
    #[must_use]
    pub const fn is_client_error(&self) -> bool {
        matches!(self, Self::Protocol(_) | Self::Timeout(_))
    }
}

/// Errors that can occur in the controller.
#[derive(Debug, Error)]
pub enum ControllerError {
    /// Failed to bind a listener to the specified address.
    #[error("Failed to bind listener to {address}: {source}")]
    BindFailed {
        address: String,
        #[source]
        source: io::Error,
    },

    /// Protocol validation failed.
    #[error("Protocol validation failed: {0}")]
    Protocol(#[from] ProtocolError),

    /// A listener error occurred.
    #[error("Listener error: {0}")]
    Listener(#[from] ListenerError),

    /// A listener task failed unexpectedly.
    #[error("Listener task failed: {0}")]
    ListenerFailed(#[from] tokio::task::JoinError),

    /// Shutdown timed out waiting for listeners to complete.
    #[error("Shutdown timeout after {0} seconds")]
    ShutdownTimeout(u64),

    /// Controller is already running.
    #[error("Controller is already running")]
    AlreadyRunning,
}

impl ControllerError {
    /// Returns `true` if the error is recoverable and the controller can retry.
    #[must_use]
    pub const fn is_recoverable(&self) -> bool {
        matches!(self, Self::ListenerFailed(_))
    }
}

/// Errors that can occur in the listener.
#[derive(Debug, Error)]
pub enum ListenerError {
    /// Failed to bind to socket address.
    #[error("Failed to bind to {address}: {source}")]
    BindFailed {
        address: String,
        #[source]
        source: io::Error,
    },

    /// Failed to accept an incoming connection.
    #[error("Failed to accept connection: {0}")]
    AcceptFailed(#[from] io::Error),

    /// Shutdown signal received.
    #[error("Shutdown requested")]
    Shutdown,
}

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
    Io(#[from] io::Error),

    #[error("Not enough bytes left")]
    UnexpectedEOF(NonZeroUsize),
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use super::*;

    #[test]
    fn test_protocol_error_display() {
        let err = ProtocolError::MissingField("hostname");
        assert_eq!(err.to_string(), "Missing required field: hostname");

        let err = ProtocolError::InvalidConfiguration {
            field: "port".to_string(),
            reason: "must be between 1-65535".to_string(),
        };
        assert_eq!(
            err.to_string(),
            "Invalid configuration for port: must be between 1-65535"
        );
    }

    #[test]
    fn test_session_error_classification() {
        let err = SessionError::Shutdown;
        assert!(err.is_shutdown());
        assert!(!err.is_client_error());

        let err = SessionError::Cancelled;
        assert!(err.is_shutdown());
        assert!(!err.is_client_error());

        let err = SessionError::Protocol("Invalid command".to_string());
        assert!(!err.is_shutdown());
        assert!(err.is_client_error());

        let err = SessionError::Timeout(30);
        assert!(!err.is_shutdown());
        assert!(err.is_client_error());
    }

    #[test]
    fn test_controller_error_recoverability() {
        let err = ControllerError::AlreadyRunning;
        assert!(!err.is_recoverable());

        let err = ControllerError::ShutdownTimeout(10);
        assert!(!err.is_recoverable());
    }

    #[test]
    fn test_error_source_chain() {
        let io_err = io::Error::new(io::ErrorKind::PermissionDenied, "access denied");
        let bind_err = ControllerError::BindFailed {
            address: "0.0.0.0:25".to_string(),
            source: io_err,
        };

        // Verify error source chain is preserved
        assert!(bind_err.source().is_some());
        assert_eq!(
            bind_err.to_string(),
            "Failed to bind listener to 0.0.0.0:25: access denied"
        );
    }
}
