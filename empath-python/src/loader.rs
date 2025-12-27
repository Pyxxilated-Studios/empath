//! Python module loader and execution
//!
//! This module handles loading Python modules from .py files and executing
//! their callbacks in response to SMTP events.

use std::{path::Path, time::Duration};

use empath_common::context::Context;
use pyo3::{
    prelude::*,
    types::{PyDict, PyTuple},
};

use crate::{
    context::PyContext,
    error::{PythonError, Result},
    safety::call_callback,
};

/// Default timeout for Python callbacks (5 seconds)
const DEFAULT_TIMEOUT_SECS: u64 = 5;

/// A loaded Python module
///
/// This struct represents a Python module loaded from a .py file.
/// It maintains the module's code and provides methods for executing
/// validation callbacks.
#[derive(Debug)]
pub struct PythonModule {
    /// Path to the Python module file
    path: String,

    /// Module source code
    source: String,

    /// Timeout for callback execution
    timeout: Duration,
}

impl PythonModule {
    /// Load a Python module from a file
    ///
    /// # Arguments
    /// * `path` - Path to the .py file
    ///
    /// # Errors
    /// Returns error if the file cannot be read
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path_str = path.as_ref().to_string_lossy().to_string();
        let source = std::fs::read_to_string(&path)
            .map_err(|_| PythonError::ModuleNotFound(path_str.clone()))?;

        Ok(Self {
            path: path_str,
            source,
            timeout: Duration::from_secs(DEFAULT_TIMEOUT_SECS),
        })
    }

    /// Set the callback timeout
    #[must_use]
    pub const fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Get the module path
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Initialize the module (call `init()` if it exists)
    ///
    /// # Errors
    /// Returns error if initialization fails
    pub fn initialize(&self) -> Result<()> {
        Python::initialize();

        Python::attach(|py| {
            let module = self.get_module_dict(py)?;

            // Call init() if it exists
            let args = PyTuple::empty(py);
            if let Some(result) = call_callback(py, &module, "init", &args, self.timeout)? {
                // Check for initialization errors
                if matches!(result.extract::<bool>(), Ok(false)) {
                    return Err(PythonError::InitFailed("init() returned False".to_string()));
                }
            }

            Ok(())
        })
    }

    /// Execute a validation callback
    ///
    /// # Arguments
    /// * `callback_name` - Name of the callback function
    /// * `context` - SMTP context
    ///
    /// # Returns
    /// Returns `true` if the callback accepts, `false` if it rejects.
    /// If the callback doesn't exist, returns `true` (allow by default).
    ///
    /// # Errors
    /// Returns error if callback execution fails
    pub fn execute_callback(&self, callback_name: &str, context: &mut Context) -> Result<bool> {
        Python::initialize();

        Python::attach(|py| {
            let module = self.get_module_dict(py)?;

            // Wrap context for Python
            let py_context = PyContext::new(context.clone());
            let py_context_obj = Py::new(py, py_context)?;

            // Prepare arguments tuple
            let args = PyTuple::new(py, &[py_context_obj.clone_ref(py)])?;

            // Call the callback
            let result = call_callback(py, &module, callback_name, &args, self.timeout)?;

            // If callback doesn't exist, allow by default
            let Some(result) = result else {
                return Ok(true);
            };

            // Extract boolean result
            let type_name = result
                .get_type()
                .name()
                .map_or_else(|_| "unknown".to_string(), |name| name.to_string());
            let accept = result.extract::<bool>().map_err(|_| {
                PythonError::InvalidReturn(
                    callback_name.to_string(),
                    format!("expected bool, got {type_name}"),
                )
            })?;

            // Copy back any context modifications
            let py_context_ref = py_context_obj.borrow(py);
            let inner = py_context_ref.inner();
            let modified_context = inner.read().clone();
            *context = modified_context;

            Ok(accept)
        })
    }

    /// Get the module dictionary (compiled and ready to execute)
    ///
    /// # Errors
    /// Returns error if the module cannot be compiled or executed
    fn get_module_dict<'py>(&self, py: Python<'py>) -> Result<Bound<'py, PyDict>> {
        let module = PyDict::new(py);

        // Add Context class to the module namespace
        module.set_item("Context", py.get_type::<PyContext>())?;

        // Execute the module code
        py.run(
            std::ffi::CString::new(self.source.as_bytes())
                .map_err(|_| PythonError::InitFailed("Invalid source code".to_string()))?
                .as_c_str(),
            Some(&module),
            None,
        )
        .map_err(|err| {
            PythonError::InitFailed(format!("Failed to execute module {}: {err}", self.path))
        })?;

        Ok(module)
    }
}

/// Clone is implemented manually to avoid cloning the module dict
impl Clone for PythonModule {
    fn clone(&self) -> Self {
        Self {
            path: self.path.clone(),
            source: self.source.clone(),
            timeout: self.timeout,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use empath_common::envelope::Envelope;

    use super::*;

    fn create_test_context() -> Context {
        use std::sync::Arc;

        Context {
            envelope: Arc::new(Envelope::default()),
            id: "test.example.com".to_string(),
            banner: Arc::from("test.example.com"),
            ..Context::default()
        }
    }

    type TestResult = std::result::Result<(), String>;

    #[test]
    #[cfg_attr(miri, ignore = "PyO3 doesn't play nice with miri")]
    fn test_load_nonexistent_module() {
        let result = PythonModule::load("/nonexistent/module.py");
        assert!(result.is_err());
    }

    #[test]
    #[cfg_attr(miri, ignore = "PyO3 doesn't play nice with miri")]
    fn test_execute_simple_callback() -> TestResult {
        use std::io::Write;

        use tempfile::NamedTempFile;

        let mut file = NamedTempFile::new().map_err(|err| err.to_string())?;
        writeln!(file, "def validate_connect(ctx):").map_err(|err| err.to_string())?;
        writeln!(file, "    return True").map_err(|err| err.to_string())?;
        file.flush().map_err(|err| err.to_string())?;

        let module = PythonModule::load(file.path()).map_err(|err| err.to_string())?;
        module.initialize().map_err(|err| err.to_string())?;

        let mut context = create_test_context();
        let result = module
            .execute_callback("validate_connect", &mut context)
            .map_err(|err| err.to_string())?;
        assert!(result);
        Ok(())
    }

    #[test]
    #[cfg_attr(miri, ignore = "PyO3 doesn't play nice with miri")]
    fn test_execute_reject_callback() -> TestResult {
        use std::io::Write;

        use tempfile::NamedTempFile;

        let mut file = NamedTempFile::new().map_err(|err| err.to_string())?;
        writeln!(file, "def validate_mail_from(ctx):").map_err(|err| err.to_string())?;
        writeln!(file, "    ctx.reject(550, 'Sender blocked')").map_err(|err| err.to_string())?;
        writeln!(file, "    return False").map_err(|err| err.to_string())?;
        file.flush().map_err(|err| err.to_string())?;

        let module = PythonModule::load(file.path()).map_err(|err| err.to_string())?;
        module.initialize().map_err(|err| err.to_string())?;

        let mut context = create_test_context();
        let result = module
            .execute_callback("validate_mail_from", &mut context)
            .map_err(|err| err.to_string())?;
        assert!(!result);
        // Check that rejection was set in response field
        assert!(context.response.is_some());
        let (status, message) = context
            .response
            .ok_or_else(|| "missing response".to_string())?;
        assert_eq!(u32::from(status), 550);
        assert_eq!(message.as_ref(), "Sender blocked");
        Ok(())
    }

    #[test]
    #[cfg_attr(miri, ignore = "PyO3 doesn't play nice with miri")]
    fn test_execute_context_modification() -> TestResult {
        use std::io::Write;

        use tempfile::NamedTempFile;

        let mut file = NamedTempFile::new().map_err(|err| err.to_string())?;
        writeln!(file, "def validate_data(ctx):").map_err(|err| err.to_string())?;
        writeln!(file, "    ctx.transaction_set('spam_score', '2.5')")
            .map_err(|err| err.to_string())?;
        writeln!(file, "    return True").map_err(|err| err.to_string())?;
        file.flush().map_err(|err| err.to_string())?;

        let module = PythonModule::load(file.path()).map_err(|err| err.to_string())?;
        module.initialize().map_err(|err| err.to_string())?;

        let mut context = create_test_context();
        let result = module
            .execute_callback("validate_data", &mut context)
            .map_err(|err| err.to_string())?;
        assert!(result);
        assert_eq!(
            context.transaction.get(&Cow::Borrowed("spam_score")),
            Some(&"2.5".to_string())
        );
        Ok(())
    }

    #[test]
    #[cfg_attr(miri, ignore = "PyO3 doesn't play nice with miri")]
    fn test_callback_not_found_allows() -> TestResult {
        use std::io::Write;

        use tempfile::NamedTempFile;

        let mut file = NamedTempFile::new().map_err(|err| err.to_string())?;
        writeln!(file, "# Empty module").map_err(|err| err.to_string())?;
        file.flush().map_err(|err| err.to_string())?;

        let module = PythonModule::load(file.path()).map_err(|err| err.to_string())?;
        module.initialize().map_err(|err| err.to_string())?;

        let mut context = create_test_context();
        let result = module
            .execute_callback("validate_connect", &mut context)
            .map_err(|err| err.to_string())?;
        assert!(result); // Allow by default
        Ok(())
    }
}
