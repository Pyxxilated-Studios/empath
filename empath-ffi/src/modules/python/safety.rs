//! Safety mechanisms for Python module execution
//!
//! This module provides timeout enforcement and exception handling to prevent
//! runaway Python code from blocking the SMTP server.

use std::time::Duration;

use pyo3::prelude::*;
use pyo3::types::{PyDict, PyTuple};

use crate::error::{ModuleError, Result};

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
    Ok(callable.call1(args)?)
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
        .ok_or_else(|| ModuleError::CallbackNotFound(callback_name.to_string()))?;

    // Verify it's callable
    if !callable.is_callable() {
        return Err(ModuleError::InvalidReturn {
            callback: callback_name.to_string(),
            reason: "not a callable".to_string(),
        });
    }

    // Execute with timeout
    call_with_timeout(py, &callable, args, timeout, callback_name).map(Some)
}

#[cfg(test)]
mod tests {
    use pyo3::types::PyTuple;

    use super::*;

    type TestResult = std::result::Result<(), String>;

    #[test]
    #[cfg_attr(miri, ignore = "PyO3 doesn't play nice with miri")]
    fn test_call_callback_success() -> TestResult {
        Python::initialize();

        let result: std::result::Result<(), ModuleError> =
            Python::attach(|py| -> std::result::Result<(), ModuleError> {
                let module = PyDict::new(py);
                py.run(
                    std::ffi::CString::new("def test_func(x): return x * 2")
                        .map_err(|_| ModuleError::InitFailed("Invalid source".to_string()))?
                        .as_c_str(),
                    Some(&module),
                    None,
                )?;

                let args = PyTuple::new(py, [5])?;
                let result =
                    call_callback(py, &module, "test_func", &args, Duration::from_secs(5))?;

                assert!(result.is_some());
                let value: i32 = result
                    .ok_or_else(|| ModuleError::InitFailed("missing callback result".to_string()))?
                    .extract()?;
                assert_eq!(value, 10);
                Ok(())
            });
        result.map_err(|e| e.to_string())?;
        Ok(())
    }

    #[test]
    #[cfg_attr(miri, ignore = "PyO3 doesn't play nice with miri")]
    fn test_call_callback_not_found() -> TestResult {
        Python::initialize();

        let result: std::result::Result<(), ModuleError> =
            Python::attach(|py| -> std::result::Result<(), ModuleError> {
                let module = PyDict::new(py);
                let args = PyTuple::new(py, &[] as &[i32])?;

                let result =
                    call_callback(py, &module, "nonexistent", &args, Duration::from_secs(5))?;

                assert!(result.is_none());
                Ok(())
            });
        result.map_err(|e| e.to_string())?;
        Ok(())
    }

    #[test]
    #[cfg_attr(miri, ignore = "PyO3 doesn't play nice with miri")]
    fn test_call_callback_exception() -> TestResult {
        Python::initialize();

        let result: std::result::Result<(), String> =
            Python::attach(|py| -> std::result::Result<(), String> {
                let module = PyDict::new(py);
                py.run(
                    std::ffi::CString::new("def test_func(): raise ValueError('test error')")
                        .map_err(|err| err.to_string())?
                        .as_c_str(),
                    Some(&module),
                    None,
                )
                .map_err(|err| err.to_string())?;

                let args = PyTuple::new(py, &[] as &[i32]).map_err(|err| err.to_string())?;
                let result = call_callback(py, &module, "test_func", &args, Duration::from_secs(5));

                assert!(result.is_err());
                Ok(())
            });
        result?;
        Ok(())
    }
}
