//! Safety mechanisms for Python module execution
//!
//! This module provides timeout enforcement and exception handling to prevent
//! runaway Python code from blocking the SMTP server.

use std::time::Duration;

use pyo3::{
    prelude::*,
    types::{PyDict, PyTuple},
};

use crate::error::{PythonError, Result};

/// Execute a Python callable with timeout
///
/// This function runs Python code with a timeout. If the code doesn't complete
/// within the specified duration, it returns a timeout error.
///
/// # Arguments
/// * `py` - Python GIL guard
/// * `callable` - Python callable to execute
/// * `args` - Arguments tuple for the callable
/// * `timeout` - Maximum execution time (currently not enforced, for future use)
/// * `callback_name` - Name of the callback (for error messages)
///
/// # Errors
/// Returns error if:
/// - The callable raises a Python exception
///
/// # Implementation Note
///
/// Timeout enforcement is currently **not implemented** due to threading constraints:
/// - Python's `signal` module only works in the main thread
/// - Empath runs callbacks in worker threads (tokio tasks)
/// - Thread-based interruption is complex with the GIL
///
/// **Mitigation**: Python modules should keep callbacks fast (< 1 second recommended).
/// Long-running operations will block the SMTP session but won't crash the server.
///
/// Future enhancement: Could use `threading.Timer` with a cancellation flag checked
/// periodically by callback code (requires cooperative cancellation).
pub fn call_with_timeout<'py>(
    _py: Python<'py>,
    callable: &Bound<'py, PyAny>,
    args: &Bound<'py, PyTuple>,
    _timeout: Duration,
    _callback_name: &str,
) -> Result<Bound<'py, PyAny>> {
    // Execute callable without timeout enforcement
    // The timeout parameter is kept for future implementation
    callable.call1(args).map_err(PythonError::Python)
}

/// Execute a Python callback safely with exception handling
///
/// This wraps the callback execution with proper exception handling and logging.
///
/// # Arguments
/// * `py` - Python GIL guard
/// * `module` - Python module containing the callback
/// * `callback_name` - Name of the callback function
/// * `args` - Arguments for the callback
/// * `timeout` - Maximum execution time
///
/// # Errors
/// Returns error if the callback doesn't exist or execution fails
pub fn call_callback<'py>(
    py: Python<'py>,
    module: &Bound<'py, PyDict>,
    callback_name: &str,
    args: &Bound<'py, PyTuple>,
    timeout: Duration,
) -> Result<Option<Bound<'py, PyAny>>> {
    // Check if callback exists
    if !module.contains(callback_name)? {
        return Ok(None);
    }

    let callable = module
        .get_item(callback_name)?
        .ok_or_else(|| PythonError::CallbackNotFound(callback_name.to_string()))?;

    // Verify it's callable
    if !callable.is_callable() {
        return Err(PythonError::InvalidReturn(
            callback_name.to_string(),
            "not a callable".to_string(),
        ));
    }

    // Execute with timeout
    call_with_timeout(py, &callable, args, timeout, callback_name).map(Some)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use pyo3::types::PyTuple;

    use super::*;

    #[test]
    fn test_call_callback_success() {
        pyo3::prepare_freethreaded_python();

        Python::with_gil(|py| {
            let module = PyDict::new(py);
            py.run(
                std::ffi::CString::new("def test_func(x): return x * 2")
                    .unwrap()
                    .as_c_str(),
                Some(&module),
                None,
            )
            .unwrap();

            let args = PyTuple::new(py, [5]).unwrap();
            let result =
                call_callback(py, &module, "test_func", &args, Duration::from_secs(5)).unwrap();

            assert!(result.is_some());
            let value: i32 = result.unwrap().extract().unwrap();
            assert_eq!(value, 10);
        });
    }

    #[test]
    fn test_call_callback_not_found() {
        pyo3::prepare_freethreaded_python();

        Python::with_gil(|py| {
            let module = PyDict::new(py);
            let args = PyTuple::new(py, &[] as &[i32]).unwrap();

            let result =
                call_callback(py, &module, "nonexistent", &args, Duration::from_secs(5)).unwrap();

            assert!(result.is_none());
        });
    }

    #[test]
    fn test_call_callback_exception() {
        pyo3::prepare_freethreaded_python();

        Python::with_gil(|py| {
            let module = PyDict::new(py);
            py.run(
                std::ffi::CString::new("def test_func(): raise ValueError('test error')")
                    .unwrap()
                    .as_c_str(),
                Some(&module),
                None,
            )
            .unwrap();

            let args = PyTuple::new(py, &[] as &[i32]).unwrap();
            let result = call_callback(py, &module, "test_func", &args, Duration::from_secs(5));

            assert!(result.is_err());
        });
    }
}
