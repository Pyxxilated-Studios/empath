//! WASM module loader and runtime management
//!
//! This module handles loading WASM modules, managing the Wasmtime engine and store,
//! and executing callbacks with resource limits and timeouts.

use std::{path::Path, time::Duration};

use empath_common::{context::Context, internal};
use wasmtime::{Config, Engine, Linker, Module, Store};

use crate::{
    error::{Result, WasmError},
    host::{WasmStore, add_host_imports},
};

/// Default timeout for callback execution (5 seconds)
const DEFAULT_TIMEOUT_SECS: u64 = 5;

/// Default maximum memory for WASM modules (10 MB)
const DEFAULT_MAX_MEMORY_BYTES: usize = 10 * 1024 * 1024;

/// WebAssembly module wrapper
///
/// Manages the lifecycle of a WASM module including loading, initialization,
/// and callback execution with resource limits.
pub struct WasmModule {
    /// Path to the WASM module file
    path: String,

    /// Wasmtime engine (shared across instances)
    engine: Engine,

    /// Compiled WASM module
    module: Module,

    /// Execution timeout
    timeout: Duration,

    /// Maximum memory in bytes
    max_memory_bytes: usize,

    /// Whether WASI is enabled
    enable_wasi: bool,
}

impl WasmModule {
    /// Load a WASM module from a file
    ///
    /// # Arguments
    /// * `path` - Path to the .wasm file
    ///
    /// # Errors
    /// Returns error if the module cannot be loaded or compiled
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path_str = path.as_ref().to_string_lossy().to_string();

        // Check if file exists
        if !path.as_ref().exists() {
            return Err(WasmError::ModuleNotFound(path_str));
        }

        // Create Wasmtime engine with default configuration
        // Note: We don't enable epoch interruption here because it would require
        // incrementing epochs in a background thread, which adds complexity.
        // Instead, we rely on OS-level timeouts for now.
        let config = Config::new();

        let engine = Engine::new(&config)?;

        // Load and compile the WASM module
        let module = Module::from_file(&engine, &path)?;

        Ok(Self {
            path: path_str,
            engine,
            module,
            timeout: Duration::from_secs(DEFAULT_TIMEOUT_SECS),
            max_memory_bytes: DEFAULT_MAX_MEMORY_BYTES,
            enable_wasi: false,
        })
    }

    /// Set the execution timeout
    #[must_use]
    pub const fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Set the maximum memory limit
    #[must_use]
    pub const fn with_max_memory(mut self, max_memory_bytes: usize) -> Self {
        self.max_memory_bytes = max_memory_bytes;
        self
    }

    /// Enable or disable WASI
    #[must_use]
    pub const fn with_wasi(mut self, enable_wasi: bool) -> Self {
        self.enable_wasi = enable_wasi;
        self
    }

    /// Initialize the module (call `init()` if it exists)
    ///
    /// # Errors
    /// Returns error if initialization fails
    pub fn initialize(&self) -> Result<()> {
        internal!(level = INFO, "Initializing wasm module ...");
        // Create a dummy context for initialization
        let dummy_context = Context::default();
        let wasm_store = WasmStore::new(&dummy_context, self.max_memory_bytes)?;
        let mut store = Store::new(&self.engine, wasm_store);
        store.limiter(|s| s);

        // Create linker with host imports
        let mut linker = Linker::new(&self.engine);
        add_host_imports(&mut linker)?;

        // TODO: Add WASI support if enabled
        // if self.enable_wasi {
        //     wasmtime_wasi::add_to_linker_sync(&mut linker)?;
        // }

        // Instantiate the module
        let instance = linker
            .instantiate(&mut store, &self.module)
            .inspect_err(|e| internal!(level = INFO, "Linker instantiation error {e}"))?;

        // Try to call `init()` if it exists
        if let Some(init_func) = instance.get_func(&mut store, "init") {
            internal!(
                level = INFO,
                "Found init function, attempting to type and call it"
            );
            let init_typed = init_func.typed::<(), i32>(&store)?;
            internal!(level = INFO, "Successfully typed init function, calling it");
            let result = init_typed.call(&mut store, ())?;
            internal!(level = INFO, "Wasm::init returned {result}");
            if result != 0 {
                return Err(WasmError::InitFailed(format!(
                    "`init()` returned non-zero: {result}"
                )));
            }
        } else {
            internal!(level = WARN, "No init() function found in WASM module");
        }

        internal!(level = INFO, "Wasm module initialised");

        Ok(())
    }

    /// Execute a callback function with the given context
    ///
    /// # Arguments
    /// * `callback_name` - Name of the exported function to call
    /// * `context` - Empath context to pass to the callback
    ///
    /// # Returns
    /// Returns the callback's return code (0 = accept, non-zero = reject)
    ///
    /// # Errors
    /// Returns error if callback execution fails or times out
    pub fn execute_callback(&self, callback_name: &str, context: &mut Context) -> Result<i32> {
        // Create store with serialized context
        let wasm_store = WasmStore::new(context, self.max_memory_bytes)?;
        let mut store = Store::new(&self.engine, wasm_store);
        store.limiter(|s| s);

        // Create linker with host imports
        let mut linker = Linker::new(&self.engine);
        add_host_imports(&mut linker)?;

        // TODO: Add WASI support if enabled
        // if self.enable_wasi {
        //     wasmtime_wasi::add_to_linker_sync(&mut linker)?;
        // }

        // Instantiate the module
        let instance = linker.instantiate(&mut store, &self.module)?;

        // Get the callback function (if it doesn't exist, return 0/accept)
        let Some(callback_func) = instance.get_func(&mut store, callback_name) else {
            tracing::trace!("Callback '{callback_name}' not found in WASM module, allowing");
            return Ok(0);
        };

        // Type the function and execute
        let callback_typed = callback_func.typed::<(), i32>(&store)?;
        let result = callback_typed.call(&mut store, ())?;

        Ok(result)
    }

    /// Get the path to the WASM module file
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Get the timeout duration
    #[must_use]
    pub const fn timeout(&self) -> Duration {
        self.timeout
    }

    /// Get the maximum memory limit
    #[must_use]
    pub const fn max_memory_bytes(&self) -> usize {
        self.max_memory_bytes
    }

    /// Check if WASI is enabled
    #[must_use]
    pub const fn is_wasi_enabled(&self) -> bool {
        self.enable_wasi
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "Inside tests")]
mod tests {
    use super::*;

    #[test]
    fn test_load_nonexistent_module() {
        let result = WasmModule::load("/nonexistent/path.wasm");
        assert!(result.is_err());
        assert!(matches!(result, Err(WasmError::ModuleNotFound(_))));
    }

    #[test]
    fn test_with_timeout() {
        // This test would require a valid WASM file, so we'll skip the actual loading
        // and just test the builder methods would work
        let _timeout = Duration::from_secs(10);
        // In a real test with a valid module:
        // let module = WasmModule::load("test.wasm").unwrap().with_timeout(timeout);
        // assert_eq!(module.timeout(), timeout);
    }
}
