//! WebAssembly module integration
//!
//! This module provides integration between WASM modules and the Empath module system.

use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use empath_common::context::Context;
use empath_common::internal;
use empath_metrics::try_metrics;
use empath_wasm::{WasmError, WasmModule};
use serde::Deserialize;

use super::{Error, Event};

type WasmModuleLock = OnceLock<WasmModule>;

/// WebAssembly module configuration (deserializable from RON)
#[derive(Deserialize)]
pub struct Wasm {
    /// Path to the .wasm file
    path: PathBuf,

    /// Execution timeout in seconds (optional, default: 5)
    #[serde(default)]
    timeout_secs: Option<u64>,

    /// Maximum memory in bytes (optional, default: 10 MB)
    #[serde(default)]
    max_memory_bytes: Option<usize>,

    /// Enable WASI (file/network access) (optional, default: false)
    #[serde(default)]
    enable_wasi: bool,

    /// WASI preopened directories: (`host_path`, `guest_name`, `readonly`)
    /// Example: [("/etc/empath/rules", "rules", true)]
    #[serde(default)]
    wasi_dirs: Vec<(String, String, bool)>,

    /// Inherit host environment variables (optional, default: false)
    #[serde(default)]
    wasi_inherit_env: bool,

    /// Loaded WASM module (initialized once)
    #[serde(skip)]
    module: WasmModuleLock,
}

impl Clone for Wasm {
    fn clone(&self) -> Self {
        Self {
            path: self.path.clone(),
            timeout_secs: self.timeout_secs,
            max_memory_bytes: self.max_memory_bytes,
            enable_wasi: self.enable_wasi,
            wasi_dirs: self.wasi_dirs.clone(),
            wasi_inherit_env: self.wasi_inherit_env,
            module: OnceLock::new(), // Cannot clone OnceLock with data
        }
    }
}

impl Wasm {
    /// Construct a WASM module definition from a file path for programmatic/tests use.
    #[must_use]
    pub fn from_path(path: PathBuf) -> Self {
        let _ = path.as_os_str();
        Self {
            path,
            timeout_secs: None,
            max_memory_bytes: None,
            enable_wasi: false,
            wasi_dirs: Vec::new(),
            wasi_inherit_env: false,
            module: OnceLock::new(),
        }
    }

    /// Initialize the WASM module
    ///
    /// # Errors
    /// Returns error if the module cannot be loaded or initialized
    pub fn init(&self) -> Result<(), Error> {
        let mut module = WasmModule::load(&self.path).map_err(|e| {
            Error::Init(format!(
                "Failed to load WASM module {}: {e}",
                self.path.display()
            ))
        })?;
        module = module.with_outbound_bindings(super::outbound_bindings());

        if let Some(timeout_secs) = self.timeout_secs {
            module = module.with_timeout(Duration::from_secs(timeout_secs));
        }

        if let Some(max_memory_bytes) = self.max_memory_bytes {
            module = module.with_max_memory(max_memory_bytes);
        }

        if !self.wasi_dirs.is_empty() {
            module = module.with_wasi_dirs(self.wasi_dirs.clone());
        }

        if self.wasi_inherit_env {
            module = module.with_wasi_env(true);
        }

        module
            .initialize()
            .map_err(|e| Error::Init(format!("Failed to initialize WASM module: {e}")))?;

        self.module
            .set(module)
            .map_err(|_| Error::Init("Module already initialized".to_string()))?;

        Ok(())
    }

    /// Emit an event to the WASM module
    ///
    /// Returns 0 if the event was handled successfully (or callback doesn't exist),
    /// non-zero if the callback rejected the operation.
    pub fn emit(&self, event: Event, context: &mut Context) -> i32 {
        let Some(module) = self.module.get() else {
            internal!(
                level = ERROR,
                "WASM module not initialized: {}",
                self.path.display()
            );
            return 0;
        };

        // Determine log severity based on event type
        // Validation = ERROR (security-critical), Events = WARN (observability)
        let is_validation = matches!(event, Event::Validate(_));

        let callback_name = event.callback_name();

        let module_path = self.path.to_string_lossy();

        // Measure execution duration
        let start = Instant::now();

        // Execute the callback
        let result = module.execute_callback(callback_name, context);

        // Record execution duration and memory limit
        let duration = start.elapsed();
        if let Some(metrics) = try_metrics() {
            metrics
                .modules
                .record_execution("wasm", callback_name, duration.as_secs_f64());

            // Update memory limit tracking (configured max memory for this module)
            if let Ok(max_memory) = u64::try_from(module.max_memory_bytes()) {
                metrics.modules.update_memory(&module_path, max_memory);
            }
        }

        match result {
            Ok(return_code) => return_code,
            Err(e) => {
                let timeout = matches!(e, WasmError::Timeout(_, _));
                let error_kind = match (is_validation, timeout) {
                    (true, true) => "validation_timeout",
                    (true, false) => "validation_error",
                    (false, true) => "timeout",
                    (false, false) => "error",
                };

                // Record error metrics
                if let Some(metrics) = try_metrics() {
                    if timeout {
                        metrics.modules.record_timeout(&module_path);
                    }
                    metrics
                        .modules
                        .record_error_with_kind("wasm", &module_path, error_kind);
                }

                if is_validation {
                    internal!(
                        level = ERROR,
                        "WASM module {} {} in {callback_name}: {e}",
                        self.path.display(),
                        if timeout { "timed out" } else { "error" }
                    );
                } else {
                    internal!(
                        level = WARN,
                        "WASM module {} {} in {callback_name}: {e}",
                        self.path.display(),
                        if timeout { "timed out" } else { "error" }
                    );
                }

                // If the module set a rejection before crashing, honor it
                // This handles the case where a module validates, calls reject(), and then errors
                if let Some((status, message)) = &context.response {
                    let code = u32::from(*status);
                    if code >= 400 {
                        internal!(
                            level = WARN,
                            "WASM module {} crashed after rejecting with {}: {} - honoring rejection",
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
                        "WASM validation module {} failed - returning 451",
                        self.path.display()
                    );
                } else {
                    internal!(
                        level = WARN,
                        "WASM event module {} failed - returning 451",
                        self.path.display()
                    );
                }
                451 // Temporary failure - be transparent about module errors
            }
        }
    }

    /// Get the path to the WASM module file
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }
}
