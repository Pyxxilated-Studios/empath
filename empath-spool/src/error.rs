//! Error types for the empath-spool crate.
//!
//! This module provides typed error handling for spool operations including
//! file I/O, serialization, and validation.

use std::io;

use thiserror::Error;

use crate::SpooledMessageId;

/// Top-level spool error type.
///
/// All spool operations return this error type, which categorizes failures
/// into I/O, serialization, validation, and logical errors.
#[derive(Debug, Error)]
pub enum SpoolError {
    /// I/O operation failed (file read/write/delete).
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    /// Serialization or deserialization error.
    #[error("Serialization error: {0}")]
    Serialization(#[from] SerializationError),

    /// Message not found in spool.
    #[error("Message not found: {0}")]
    NotFound(SpooledMessageId),

    /// Spool directory validation failed.
    #[error("Spool validation error: {0}")]
    Validation(#[from] ValidationError),

    /// Internal error (lock poisoning, etc.).
    #[error("Internal error: {0}")]
    Internal(String),

    /// Message already exists in spool.
    #[error("Message already exists: {0}")]
    AlreadyExists(SpooledMessageId),

    /// File watcher error.
    #[error("File watcher error: {0}")]
    WatchError(String),
}

/// Serialization and deserialization errors.
#[derive(Debug, Error)]
pub enum SerializationError {
    /// Bincode serialization failed.
    #[error("Bincode encode error: {0}")]
    Encode(String),

    /// Bincode deserialization failed.
    #[error("Bincode decode error: {0}")]
    Decode(String),

    /// Invalid message format (corrupted data).
    #[error("Invalid message format: {0}")]
    InvalidFormat(String),

    /// Message data is corrupted or incomplete.
    #[error("Corrupted message data: {0}")]
    Corrupted(String),
}

/// Spool directory validation errors.
#[derive(Debug, Error)]
pub enum ValidationError {
    /// Spool path does not exist.
    #[error("Spool path does not exist: {0}")]
    PathNotFound(String),

    /// Spool path is not a directory.
    #[error("Spool path is not a directory: {0}")]
    NotDirectory(String),

    /// Spool path is not writable.
    #[error("Spool path is not writable: {0}")]
    NotWritable(String),

    /// Spool path is not readable.
    #[error("Spool path is not readable: {0}")]
    NotReadable(String),

    /// Invalid spool configuration.
    #[error("Invalid spool configuration: {0}")]
    InvalidConfiguration(String),
}

/// Specialized `Result` type for spool operations.
pub type Result<T> = std::result::Result<T, SpoolError>;

// Convenience conversion for lock poisoning
impl<T> From<std::sync::PoisonError<T>> for SpoolError {
    fn from(e: std::sync::PoisonError<T>) -> Self {
        Self::Internal(format!("Lock poisoned: {e}"))
    }
}

// Convert bincode errors to our error type
impl From<bincode::Error> for SerializationError {
    fn from(e: bincode::Error) -> Self {
        match *e {
            bincode::ErrorKind::Io(io_err) => {
                Self::Decode(format!("I/O error during deserialization: {io_err}"))
            }
            bincode::ErrorKind::InvalidUtf8Encoding(utf8_err) => {
                Self::InvalidFormat(format!("Invalid UTF-8: {utf8_err}"))
            }
            bincode::ErrorKind::InvalidBoolEncoding(val) => {
                Self::InvalidFormat(format!("Invalid bool encoding: {val}"))
            }
            bincode::ErrorKind::InvalidCharEncoding => {
                Self::InvalidFormat("Invalid char encoding".to_string())
            }
            bincode::ErrorKind::InvalidTagEncoding(val) => {
                Self::InvalidFormat(format!("Invalid tag encoding: {val}"))
            }
            bincode::ErrorKind::DeserializeAnyNotSupported => {
                Self::Decode("Deserialize any not supported".to_string())
            }
            bincode::ErrorKind::SizeLimit => Self::Decode("Size limit exceeded".to_string()),
            bincode::ErrorKind::SequenceMustHaveLength => {
                Self::InvalidFormat("Sequence must have length".to_string())
            }
            bincode::ErrorKind::Custom(msg) => Self::Decode(msg),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spool_error_display() {
        let id = SpooledMessageId::generate();
        let err = SpoolError::NotFound(id.clone());
        assert_eq!(err.to_string(), format!("Message not found: {id}"));

        let err = SpoolError::AlreadyExists(id.clone());
        assert_eq!(err.to_string(), format!("Message already exists: {id}"));

        let err = SpoolError::Internal("test error".to_string());
        assert_eq!(err.to_string(), "Internal error: test error");
    }

    #[test]
    fn test_serialization_error_display() {
        let err = SerializationError::Encode("test".to_string());
        assert_eq!(err.to_string(), "Bincode encode error: test");

        let err = SerializationError::Corrupted("missing header".to_string());
        assert_eq!(err.to_string(), "Corrupted message data: missing header");
    }

    #[test]
    fn test_validation_error_display() {
        let err = ValidationError::PathNotFound("/var/spool".to_string());
        assert_eq!(err.to_string(), "Spool path does not exist: /var/spool");

        let err = ValidationError::NotWritable("/tmp/spool".to_string());
        assert_eq!(err.to_string(), "Spool path is not writable: /tmp/spool");
    }

    #[test]
    fn test_io_error_conversion() {
        let io_err = io::Error::new(io::ErrorKind::NotFound, "file not found");
        let spool_err: SpoolError = io_err.into();
        assert!(matches!(spool_err, SpoolError::Io(_)));
    }

    #[test]
    fn test_error_chain() {
        let io_err = io::Error::new(io::ErrorKind::PermissionDenied, "access denied");
        let spool_err = SpoolError::from(io_err);

        assert!(matches!(spool_err, SpoolError::Io(_)));
        assert!(spool_err.to_string().contains("access denied"));
    }
}
