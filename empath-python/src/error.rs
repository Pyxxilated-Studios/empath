//! Error types for Python module operations

use empath_common::ModuleError;
use pyo3::PyErr;

/// Result type for Python module operations
pub type Result<T> = std::result::Result<T, ModuleError>;

/// Convert `PyO3` errors to `ModuleError`
pub fn from_pyerr(err: &PyErr) -> ModuleError {
    ModuleError::Runtime(err.to_string())
}
