//! Error types for module operations (Python, WASM, `SharedLibrary`, etc.)

use std::io;

use thiserror::Error;

/// Result type for module operations
pub type Result<T> = std::result::Result<T, ModuleError>;

/// Errors that can occur when working with extension modules (Python, WASM, etc.)
#[derive(Debug, Error)]
#[non_exhaustive]
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

    /// Python runtime error
    #[cfg(feature = "python")]
    #[error("Python error: {0}")]
    Python(#[from] pyo3::PyErr),

    /// WASM runtime error (Wasmtime engine, etc.)
    #[cfg(feature = "wasm")]
    #[error(transparent)]
    Runtime(#[from] anyhow::Error),

    /// Poison Error
    #[error("Poison error: {0}")]
    Poisoned(String),

    /// Resource limit exceeded (memory, execution time, etc.)
    #[error("Resource limit exceeded: {0}")]
    ResourceLimit(String),

    /// Serialization/deserialization error (WASM protocol)
    #[cfg(feature = "wasm")]
    #[error("Protocol error: {0}")]
    Protocol(#[from] postcard::Error),
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
