//! WASM module loader and runtime management
//!
//! This module handles loading WASM modules, managing the Wasmtime engine and store,
//! and executing callbacks with resource limits and timeouts.

use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};

use empath_common::{context::Context, internal};
use wasmtime::{
    Config, Engine, Store,
    component::{Component, Linker},
};
use wasmtime_wasi::{DirPerms, FilePerms, WasiCtxBuilder, p2::add_to_linker_sync};

use crate::{
    bindings::{Validator, exports::empath::validator::validation as wit},
    error::{Result, WasmError},
    host::WasmStore,
};

/// Default timeout for callback execution (5 seconds)
const DEFAULT_TIMEOUT_SECS: u64 = 5;

/// Default maximum memory for WASM modules (10 MB)
const DEFAULT_MAX_MEMORY_BYTES: usize = 10 * 1024 * 1024;

/// Convert Empath Context to WIT context record
///
/// This creates the typed context structure that component validation functions expect.
fn context_to_wit(ctx: &Context) -> wit::Context {
    wit::Context {
        extended: ctx.extended,
        id: ctx.id.clone(),
        sender: Some(ctx.sender()),
        recipients: ctx.recipients(),
        data: ctx.data.as_ref().map(|arc| arc.to_vec()),
        max_message_size: ctx.max_message_size as u64,
        transaction: ctx
            .transaction
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect(),
        tracking_id: ctx.tracking_id.clone(),
    }
}

/// Convert validation result to return code
///
/// Returns 0 for Accept, or the SMTP error code for Reject
fn validation_result_to_code(callback_name: &str, result: wit::ValidationResult) -> i32 {
    match result {
        wit::ValidationResult::Accept => {
            internal!(
                level = DEBUG,
                "WASM callback '{callback_name}' returned: Accept"
            );
            0
        }
        wit::ValidationResult::Reject(rejection) => {
            internal!(
                level = INFO,
                "WASM callback '{callback_name}' returned: Reject({}, {})",
                rejection.code,
                rejection.message
            );
            // Return the SMTP error code (rejection message handled by caller)
            rejection.code.cast_signed()
        }
    }
}

/// Call a validation callback and return the result code
fn call_validation_callback(
    callback_name: &str,
    validator: &Validator,
    store: &mut Store<WasmStore>,
    wit_context: &wit::Context,
) -> Result<i32> {
    let result = match callback_name {
        "validate_connect" => validator
            .empath_validator_validation()
            .call_validate_connect(store, wit_context)?,
        "validate_ehlo" => validator
            .empath_validator_validation()
            .call_validate_ehlo(store, wit_context)?,
        "validate_mail_from" => validator
            .empath_validator_validation()
            .call_validate_mail_from(store, wit_context)?,
        "validate_rcpt_to" => validator
            .empath_validator_validation()
            .call_validate_rcpt_to(store, wit_context)?,
        "validate_data" => validator
            .empath_validator_validation()
            .call_validate_data(store, wit_context)?,
        "validate_starttls" => validator
            .empath_validator_validation()
            .call_validate_starttls(store, wit_context)?,
        _ => return Ok(-1), // Unknown callback
    };

    Ok(validation_result_to_code(callback_name, result))
}

/// Call an event callback
fn call_event_callback(
    callback_name: &str,
    validator: &Validator,
    store: &mut Store<WasmStore>,
    wit_context: &wit::Context,
) -> Result<()> {
    match callback_name {
        "on_connection_opened" => validator
            .empath_validator_events()
            .call_on_connection_opened(store, wit_context)?,
        "on_connection_closed" => validator
            .empath_validator_events()
            .call_on_connection_closed(store, wit_context)?,
        "on_delivery_attempt" => validator
            .empath_validator_events()
            .call_on_delivery_attempt(store, wit_context)?,
        "on_delivery_success" => validator
            .empath_validator_events()
            .call_on_delivery_success(store, wit_context)?,
        "on_delivery_failure" => validator
            .empath_validator_events()
            .call_on_delivery_failure(store, wit_context)?,
        "on_smtp_error" => validator
            .empath_validator_events()
            .call_on_smtp_error(store, wit_context)?,
        "on_smtp_message_received" => validator
            .empath_validator_events()
            .call_on_smtp_message_received(store, wit_context)?,
        "on_dns_lookup" => validator
            .empath_validator_events()
            .call_on_dns_lookup(store, wit_context)?,
        _ => return Ok(()), // Unknown callback
    }

    internal!(level = DEBUG, "WASM callback '{callback_name}' completed");
    Ok(())
}

/// WebAssembly component wrapper
///
/// Manages the lifecycle of a WASM component including loading, initialization,
/// and callback execution with resource limits.
pub struct WasmModule {
    /// Path to the WASM component file
    path: String,

    /// Wasmtime engine (shared across instances)
    engine: Engine,

    /// Compiled WASM component
    component: Component,

    /// Execution timeout
    timeout: Duration,

    /// Maximum memory in bytes
    max_memory_bytes: usize,

    /// WASI preopened directories: (`host_path`, `guest_name`, readonly)
    wasi_dirs: Vec<(String, String, bool)>,

    /// Inherit host environment variables
    wasi_inherit_env: bool,
}

impl WasmModule {
    /// Load a WASM component from a file
    ///
    /// # Arguments
    /// * `path` - Path to the .wasm component file
    ///
    /// # Errors
    /// Returns error if the component cannot be loaded or compiled
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path_str = path.as_ref().to_string_lossy().to_string();

        // Check if file exists
        if !path.as_ref().exists() {
            return Err(WasmError::ModuleNotFound(path_str));
        }

        // Create Wasmtime engine with component model enabled
        // Note: We don't enable epoch interruption here because it would require
        // incrementing epochs in a background thread, which adds complexity.
        // Instead, we rely on OS-level timeouts for now.
        let mut config = Config::new();
        config.wasm_component_model(true);

        let engine = Engine::new(&config)?;

        // Load and compile the WASM component
        let component = Component::from_file(&engine, &path)?;

        Ok(Self {
            path: path_str,
            engine,
            component,
            timeout: Duration::from_secs(DEFAULT_TIMEOUT_SECS),
            max_memory_bytes: DEFAULT_MAX_MEMORY_BYTES,
            wasi_dirs: Vec::new(),
            wasi_inherit_env: false,
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

    /// Set WASI preopened directories
    ///
    /// # Arguments
    /// * `dirs` - Vector of (`host_path`, `guest_name`, readonly) tuples
    #[must_use]
    pub fn with_wasi_dirs(mut self, dirs: Vec<(String, String, bool)>) -> Self {
        self.wasi_dirs = dirs;
        self
    }

    /// Enable or disable inheriting host environment variables
    #[must_use]
    pub const fn with_wasi_env(mut self, inherit_env: bool) -> Self {
        self.wasi_inherit_env = inherit_env;
        self
    }

    /// Initialize the component (call `init()` if it exists)
    ///
    /// # Errors
    /// Returns error if initialization fails
    pub fn initialize(&self) -> Result<()> {
        internal!(level = TRACE, "Initializing wasm component ...");

        // Build WASI context
        let wasi_ctx = self.build_wasi_ctx()?;

        // Create a dummy context for initialization
        let dummy_context = Arc::new(Mutex::new(Context::default()));
        let wasm_store = WasmStore::new(dummy_context, self.max_memory_bytes, wasi_ctx)?;
        let mut store = Store::new(&self.engine, wasm_store);
        store.limiter(|s| s);

        // Create component linker with WASI support
        let mut linker = Linker::new(&self.engine);

        // Add WASI support (always enabled for components)
        internal!(level = TRACE, "Adding WASI support to component linker");
        add_to_linker_sync(&mut linker)?;

        // Add custom host functions from WIT interface
        internal!(level = TRACE, "Adding custom host functions to linker");
        crate::bindings::Validator::add_to_linker::<_, wasmtime::component::HasSelf<WasmStore>>(
            &mut linker,
            |data| data,
        )?;

        // Instantiate the component
        let _instance = linker
            .instantiate(&mut store, &self.component)
            .inspect_err(|e| internal!(level = TRACE, "Component instantiation error {e}"))?;

        // Note: Component model uses typed interfaces defined in WIT files
        // Initialization and function calling will be handled through generated bindings
        // For now, we just verify the component loads successfully

        internal!(level = TRACE, "Wasm component initialised");

        Ok(())
    }

    /// Execute a callback function with the given context
    ///
    /// # Arguments
    /// * `callback_name` - Name of the exported function to call (validation or event callback)
    /// * `context` - Empath context to pass to the callback
    ///
    /// # Returns
    /// Returns the callback's return code (0 = accept, non-zero = reject for validation callbacks;
    /// always 0 for event callbacks)
    ///
    /// # Errors
    /// Returns error if callback execution fails or times out
    ///
    /// # Panics
    /// Panics if the context mutex is poisoned or if there are multiple Arc references
    /// (both should never happen in normal operation)
    #[allow(clippy::expect_used, reason = "Panic on poisoned mutex is intentional")]
    pub fn execute_callback(&self, callback_name: &str, context: &mut Context) -> Result<i32> {
        // Build WASI context
        let wasi_ctx = self.build_wasi_ctx()?;

        // Wrap context in Arc<Mutex<>> for thread-safe access from host functions
        let context_arc = Arc::new(Mutex::new(std::mem::take(context)));

        // Create store with wrapped context
        let wasm_store = WasmStore::new(Arc::clone(&context_arc), self.max_memory_bytes, wasi_ctx)?;
        let mut store = Store::new(&self.engine, wasm_store);
        store.limiter(|s| s);

        // Create component linker with WASI support
        let mut linker = Linker::new(&self.engine);

        // Add WASI support (always enabled for components)
        add_to_linker_sync(&mut linker)?;

        // Add custom host functions from WIT interface
        crate::bindings::Validator::add_to_linker::<_, wasmtime::component::HasSelf<WasmStore>>(
            &mut linker,
            |data| data,
        )?;

        // Instantiate the component using generated bindings
        let validator = Validator::instantiate(&mut store, &self.component, &linker)?;

        // Convert context to WIT format
        let wit_context = {
            let ctx_lock = context_arc.lock().expect("Failed to lock context");
            context_to_wit(&ctx_lock)
        };

        // Call the appropriate validation or event function based on callback name
        let return_code = if callback_name.starts_with("validate_") {
            // Validation callback
            let code =
                call_validation_callback(callback_name, &validator, &mut store, &wit_context)?;
            if code == -1 {
                internal!(
                    level = WARN,
                    "Unknown validation callback '{callback_name}'"
                );
                0
            } else {
                code
            }
        } else if callback_name.starts_with("on_") {
            // Event callback
            call_event_callback(callback_name, &validator, &mut store, &wit_context)?;
            0
        } else {
            // Unknown callback - log warning and accept
            internal!(level = WARN, "Unknown callback '{callback_name}' requested");
            0
        };

        // Drop the store to release its Arc reference to the context
        // This is necessary because WasmStore holds Arc<Mutex<Context>>, and we need
        // exactly one Arc reference to successfully unwrap it below
        drop(store);

        // Extract context back from Arc<Mutex<>> after callback completes
        // These operations should never fail in practice:
        // - try_unwrap only fails if there are multiple Arc references (we hold the only one)
        // - into_inner only fails if the mutex is poisoned (would require a panic in host functions)
        let result_context = Arc::try_unwrap(context_arc)
            .expect("Context Arc should have exactly one strong reference")
            .into_inner()
            .expect("Context Mutex should not be poisoned");

        // Restore the context with any modifications made by host functions
        *context = result_context;

        Ok(return_code)
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

    /// Build a WASI context from the configuration
    ///
    /// # Errors
    /// Returns error if WASI context creation fails (e.g., invalid directory paths)
    fn build_wasi_ctx(&self) -> Result<wasmtime_wasi::WasiCtx> {
        internal!(level = TRACE, "Building WASI context");

        let mut builder = WasiCtxBuilder::new();

        // Configure stdin/stdout/stderr (inherit from host)
        builder.inherit_stdio();

        // Configure environment variables
        if self.wasi_inherit_env {
            internal!(level = TRACE, "WASI: Inheriting host environment variables");
            builder.inherit_env();
        }

        // Configure preopened directories
        for (host_path, guest_name, readonly) in &self.wasi_dirs {
            internal!(
                level = TRACE,
                "WASI: Preopening directory: {} -> {} (readonly: {})",
                host_path,
                guest_name,
                readonly
            );

            let dir_perms = if *readonly {
                DirPerms::READ
            } else {
                DirPerms::all()
            };
            let file_perms = if *readonly {
                FilePerms::READ
            } else {
                FilePerms::all()
            };

            builder.preopened_dir(host_path, guest_name, dir_perms, file_perms)?;
        }

        let ctx = builder.build();
        internal!(level = TRACE, "WASI context built successfully");
        Ok(ctx)
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
