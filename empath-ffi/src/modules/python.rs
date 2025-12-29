//! Python module support
//!
//! This module provides integration between Python modules and the Empath module system.

use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use empath_common::context::Context;
use empath_common::internal;
use empath_metrics::try_metrics;
use empath_python::{PythonError, PythonModule};
use serde::Deserialize;

use super::{Error, Event};

/// Python module configuration (deserializable from RON)
#[derive(Clone, Debug, Deserialize)]
pub struct Python {
    path: PathBuf,
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
            .initialize()
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

        let start = Instant::now();
        let result = module.execute_callback(callback_name, context);
        let duration = start.elapsed();

        if let Some(metrics) = try_metrics() {
            metrics
                .modules
                .record_execution("python", callback_name, duration.as_secs_f64());
        }

        match result {
            Ok(true) => 0,  // Accept
            Ok(false) => 1, // Reject
            Err(e) => {
                let timeout = matches!(e, PythonError::Timeout(_, _));
                if let Some(metrics) = try_metrics() {
                    if timeout {
                        metrics.modules.record_timeout(module_path.as_ref());
                    }

                    let error_kind = match (is_validation, timeout) {
                        (true, true) => "validation_timeout",
                        (true, false) => "validation_error",
                        (false, true) => "timeout",
                        (false, false) => "error",
                    };

                    metrics.modules.record_error_with_kind(
                        "python",
                        module_path.as_ref(),
                        error_kind,
                    );
                }

                internal!(
                    level = ERROR,
                    "Python module {} error in {callback_name}: {e}",
                    self.path.display()
                );

                // If the module set a rejection before crashing, honor it
                // This handles the case where a module validates, calls reject(), and then errors
                if let Some((status, message)) = &context.response {
                    let code = u32::from(*status);
                    if code >= 400 {
                        internal!(
                            level = WARN,
                            "Python module {} crashed after rejecting with {}: {} - honoring rejection",
                            self.path.display(),
                            code,
                            message
                        );
                        return code.try_into().unwrap_or(550);
                    }
                }

                // No prior rejection - return temporary failure for transparency
                // Log at ERROR for validation (security-critical), WARN for events (observability)
                if is_validation {
                    internal!(
                        level = ERROR,
                        "Python validation module {} failed - returning 451",
                        self.path.display()
                    );
                } else {
                    internal!(
                        level = WARN,
                        "Python event module {} failed - returning 451",
                        self.path.display()
                    );
                }
                451 // Temporary failure - be transparent about module errors
            }
        }
    }

    /// Get the module path for display
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }
}
