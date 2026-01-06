//! Error types for the empath-common crate.
//!
//! This module provides foundational error types used across all protocols
//! and session handlers in the Empath MTA.

use std::io;

pub use empath_core::error::MessageParseError;
use thiserror::Error;

/// Errors that can occur when working with extension modules (Python, WASM, etc.)
#[derive(Debug, Error)]
pub enum ModuleError {
    /// Module file not found
    #[error("Module file not found: {0}")]
    ModuleNotFound(String),

    /// Callback not found in module
    #[error("Callback '{0}' not found in module")]
    CallbackNotFound(String),

    /// Module initialization failed
    #[error("Module initialization failed: {0}")]
    InitFailed(String),

    /// Invalid return value from callback
    #[error("Invalid return value from callback '{callback}': {reason}")]
    InvalidReturn { callback: String, reason: String },

    /// Callback timeout
    #[error("Callback '{callback}' timed out after {timeout_secs}s")]
    Timeout { callback: String, timeout_secs: u64 },

    /// I/O error
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    /// Runtime-specific error (Python interpreter, Wasmtime engine, etc.)
    #[error("{0}")]
    Runtime(String),

    /// Resource limit exceeded (memory, execution time, etc.)
    #[error("Resource limit exceeded: {0}")]
    ResourceLimit(String),

    /// Serialization/deserialization error
    #[error("Protocol error: {0}")]
    Protocol(String),
}

impl ModuleError {
    /// Returns `true` if this error is a timeout.
    #[must_use]
    pub const fn is_timeout(&self) -> bool {
        matches!(self, Self::Timeout { .. })
    }

    /// Returns `true` if this error indicates the callback doesn't exist.
    #[must_use]
    pub const fn is_callback_not_found(&self) -> bool {
        matches!(self, Self::CallbackNotFound(_))
    }
}

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
