//! Error types for Python module operations

use pyo3::PyErr;

/// Result type for Python module operations
pub type Result<T> = std::result::Result<T, PythonError>;

/// Errors that can occur when working with Python modules
#[derive(Debug, thiserror::Error)]
pub enum PythonError {
    /// Python interpreter error
    #[error("Python error: {0}")]
    Python(#[from] PyErr),

    /// Module file not found
    #[error("Module file not found: {0}")]
    ModuleNotFound(String),

    /// Callback not found in module
    #[error("Callback '{0}' not found in module")]
    CallbackNotFound(String),

    /// Callback timeout
    #[error("Callback '{0}' timed out after {1}s")]
    Timeout(String, u64),

    /// Module initialization failed
    #[error("Module initialization failed: {0}")]
    InitFailed(String),

    /// Invalid return value from callback
    #[error("Invalid return value from callback '{0}': {1}")]
    InvalidReturn(String, String),

    /// I/O error
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}
