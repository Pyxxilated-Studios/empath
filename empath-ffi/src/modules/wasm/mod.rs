//! WebAssembly module support
//!
//! This module provides integration between WASM modules and the Empath module system.

use std::any::Any;
use std::fmt;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

use empath_common::context::{Context, ModuleContext};
use empath_common::internal;
use serde::{Deserialize, Serialize};

use super::traits::ModuleInitContext;
use super::{Error, Event, handle_module_error};
use crate::modules::traits::Module;

pub mod bindings;
pub mod host;
pub mod loader;

pub use host::{SerializableContext, WasmStore};
pub use loader::{DeterministicWasiConfig, WasmLoader};

type WasmLoaderLock = OnceLock<WasmLoader>;

/// WebAssembly module configuration (deserializable from RON)
#[derive(Deserialize, Serialize)]
pub struct Wasm {
    /// Path to the .wasm file
    path: PathBuf,

    /// Module initialization arguments
    #[serde(alias = "args", default)]
    arguments: Vec<String>,

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
    module: WasmLoaderLock,
}

impl fmt::Debug for Wasm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Wasm")
            .field("path", &self.path)
            .field("arguments", &self.arguments)
            .field("timeout_secs", &self.timeout_secs)
            .field("max_memory_bytes", &self.max_memory_bytes)
            .field("enable_wasi", &self.enable_wasi)
            .field("wasi_dirs", &self.wasi_dirs)
            .field("wasi_inherit_env", &self.wasi_inherit_env)
            .finish_non_exhaustive()
    }
}

impl Clone for Wasm {
    fn clone(&self) -> Self {
        Self {
            path: self.path.clone(),
            arguments: self.arguments.clone(),
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
    pub const fn from_path(path: PathBuf) -> Self {
        Self {
            path,
            arguments: Vec::new(),
            timeout_secs: None,
            max_memory_bytes: None,
            enable_wasi: false,
            wasi_dirs: Vec::new(),
            wasi_inherit_env: false,
            module: OnceLock::new(),
        }
    }

    /// Get the path to the WASM module file
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }
}

#[typetag::serde]
impl Module for Wasm {
    fn init(&mut self, context: &ModuleInitContext) -> std::result::Result<(), Error> {
        let mut module = WasmLoader::load(&self.path).map_err(|e| {
            Error::Init(format!(
                "Failed to load WASM module {}: {e}",
                self.path.display()
            ))
        })?;
        module = module.with_outbound_bindings(context.outbound_bindings.clone());
        module = module.with_args(self.arguments.clone());

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

    fn emit(&self, event: Event, context: &mut Context) -> i32 {
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

        // Execute the callback
        let result = module.execute_callback(callback_name, context);

        // Record memory limit
        #[cfg(feature = "metrics")]
        if let Some(metrics) = empath_metrics::metrics() {
            // Update memory limit tracking (configured max memory for this module)
            if let Ok(max_memory) = u64::try_from(module.max_memory_bytes()) {
                metrics.modules.update_memory(&module_path, max_memory);
            }
        }

        match result {
            Ok(return_code) => return_code,
            Err(e) => handle_module_error(
                "wasm",
                &module_path,
                is_validation,
                &e,
                callback_name,
                context,
            ),
        }
    }

    fn module_context(&self) -> ModuleContext {
        ModuleContext::new("wasm", self.path.display().to_string())
    }

    fn display_name(&self) -> String {
        format!("WASM Module: {}", self.path.display())
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

    use super::{Event, Wasm};
    use crate::modules::Ev;
    use crate::modules::traits::Module;

    #[test]
    fn emit_returns_zero_when_uninitialized() {
        let module = Wasm {
            path: PathBuf::from("module.wasm"),
            arguments: Vec::new(),
            timeout_secs: None,
            max_memory_bytes: None,
            enable_wasi: false,
            wasi_dirs: Vec::new(),
            wasi_inherit_env: false,
            module: OnceLock::new(),
        };
        let mut context = Context::default();

        let result = module.emit(Event::Event(Ev::ConnectionOpened), &mut context);

        assert_eq!(result, 0);
    }

    #[test]
    fn path_returns_configured_path() {
        let module = Wasm::from_path(PathBuf::from("module.wasm"));

        assert_eq!(module.path(), PathBuf::from("module.wasm").as_path());
    }
}
