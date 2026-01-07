//! Python module support
//!
//! This module provides integration between Python modules and the Empath module system.

use std::any::Any;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

use empath_common::context::{Context, ModuleContext};
use empath_common::internal;
use serde::{Deserialize, Serialize};

use super::traits::ModuleInitContext;
use super::{Error, Event, handle_module_error};
use crate::modules::traits::Module;

pub mod context;
pub mod loader;
pub mod safety;

pub use context::PyContext;

/// Python module configuration (deserializable from RON)
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Python {
    path: PathBuf,
    #[serde(alias = "args", default)]
    arguments: Vec<String>,
    #[serde(default)]
    timeout_secs: Option<u64>,
    /// Loaded module (populated during init)
    #[serde(skip)]
    module: OnceLock<loader::Python>,
}

impl Python {
    /// Get the module path for display
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }
}

#[typetag::serde]
impl Module for Python {
    fn init(&mut self, _context: &ModuleInitContext) -> Result<(), Error> {
        let mut module = loader::Python::load(&self.path).map_err(|e| {
            Error::Init(format!(
                "Failed to load Python module {}: {e:?}",
                self.path.display()
            ))
        })?;

        if let Some(timeout_secs) = self.timeout_secs {
            module = module.with_timeout(Duration::from_secs(timeout_secs));
        }

        module
            .initialize(&self.arguments)
            .map_err(|e| Error::Init(format!("Failed to initialize Python module: {e:?}")))?;

        self.module
            .set(module)
            .map_err(|_| Error::Init("Module already initialized".to_string()))?;

        Ok(())
    }

    fn emit(&self, event: Event, context: &mut Context) -> i32 {
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

    fn module_context(&self) -> ModuleContext {
        ModuleContext::new("python", self.path.display().to_string())
    }

    fn display_name(&self) -> String {
        format!("Python Module: {}", self.path.display())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::OnceLock;

    use empath_common::context::Context;

    use super::{Event, Python};
    use crate::modules::Ev;
    use crate::modules::traits::Module;

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
