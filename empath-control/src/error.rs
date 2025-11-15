//! Error types for control operations

use thiserror::Error;

/// Errors that can occur during control operations
#[derive(Debug, Error)]
pub enum ControlError {
    /// I/O error communicating with the control socket
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Protocol deserialization error
    #[error("Protocol error: {0}")]
    ProtocolDeserialization(#[from] bincode::error::DecodeError),

    /// Protocol serialization error
    #[error("Protocol error: {0}")]
    ProtocolSerialization(#[from] bincode::error::EncodeError),

    /// Server returned an error
    #[error("Server error: {0}")]
    ServerError(String),

    /// Connection closed unexpectedly
    #[error("Connection closed")]
    ConnectionClosed,

    /// Request timeout
    #[error("Request timeout")]
    Timeout,

    /// Control socket path is invalid
    #[error("Invalid socket path: {0}")]
    InvalidSocketPath(String),
}

/// Result type for control operations
pub type Result<T> = std::result::Result<T, ControlError>;
