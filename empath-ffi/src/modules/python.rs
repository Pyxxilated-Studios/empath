//! Python module support
//!
//! This module provides integration between Python modules and the Empath module system.

use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

use empath_common::context::Context;
use empath_common::internal;
use empath_python::PythonModule;
use serde::Deserialize;

use super::{Error, Event, handle_module_error};

/// Python module configuration (deserializable from RON)
#[derive(Clone, Debug, Deserialize)]
pub struct Python {
    path: PathBuf,
    #[serde(alias = "args", default)]
    arguments: Vec<String>,
    #[serde(default)]
    timeout_secs: Option<u64>,
    /// Loaded module (populated during init)
    #[serde(skip)]
    module: OnceLock<PythonModule>,
}

impl Python {
    /// Initialize the Python module by loading the .py file
    ///
    /// # Errors
    /// Returns error if the module cannot be loaded or initialized
    pub fn init(&self) -> Result<(), Error> {
        let mut module = PythonModule::load(&self.path).map_err(|e| {
            Error::Init(format!(
                "Failed to load Python module {}: {e}",
                self.path.display()
            ))
        })?;

        if let Some(timeout_secs) = self.timeout_secs {
            module = module.with_timeout(Duration::from_secs(timeout_secs));
        }

        module
            .initialize(&self.arguments)
            .map_err(|e| Error::Init(format!("Failed to initialize Python module: {e}")))?;

        self.module
            .set(module)
            .map_err(|_| Error::Init("Module already initialized".to_string()))?;

        Ok(())
    }

    /// Emit an event to the Python module
    pub fn emit(&self, event: Event, context: &mut Context) -> i32 {
        // Get the loaded module
        let Some(module) = self.module.get() else {
            internal!(
                level = ERROR,
                "Python module not initialized: {}",
                self.path.display()
            );
            return 0; // Fail open
        };

        // Determine log severity based on event type
        // Validation = ERROR (security-critical), Events = WARN (observability)
        let is_validation = matches!(event, Event::Validate(_));

        let callback_name = event.callback_name();
        let module_path = self.path.to_string_lossy();

        let result = module.execute_callback(callback_name, context);

        match result {
            Ok(true) => 0,  // Accept
            Ok(false) => 1, // Reject
            Err(e) => handle_module_error(
                "python",
                &module_path,
                is_validation,
                &e,
                callback_name,
                context,
            ),
        }
    }

    /// Get the module path for display
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::OnceLock;

    use empath_common::context::Context;

    use super::{Event, Python};
    use crate::modules::Ev;

    #[test]
    fn emit_returns_zero_when_uninitialized() {
        let module = Python {
            path: PathBuf::from("module.py"),
            arguments: Vec::new(),
            timeout_secs: None,
            module: OnceLock::new(),
        };
        let mut context = Context::default();

        let result = module.emit(Event::Event(Ev::ConnectionOpened), &mut context);

        assert_eq!(result, 0);
    }

    #[test]
    fn path_returns_configured_path() {
        let module = Python {
            path: PathBuf::from("module.py"),
            arguments: Vec::new(),
            timeout_secs: None,
            module: OnceLock::new(),
        };

        assert_eq!(module.path(), PathBuf::from("module.py").as_path());
    }
}
