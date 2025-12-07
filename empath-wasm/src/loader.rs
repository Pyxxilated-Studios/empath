//! WASM module loader and runtime management
//!
//! This module handles loading WASM modules, managing the Wasmtime engine and store,
//! and executing callbacks with resource limits and timeouts.

use std::{
    path::Path,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
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

/// Epoch tick interval in milliseconds (10ms = 100Hz)
const EPOCH_TICK_MS: u64 = 10;

/// Shared Wasmtime engine across all WASM modules (for efficiency and compiled code sharing)
static SHARED_ENGINE: OnceLock<Arc<Engine>> = OnceLock::new();

/// Flag to ensure epoch manager task is started only once
static EPOCH_TASK_STARTED: AtomicBool = AtomicBool::new(false);

/// Get or initialize the shared Wasmtime engine
///
/// This function ensures only one Engine is created and shared across all WASM modules.
/// The Engine is configured with epoch interruption enabled for timeout enforcement.
///
/// # Errors
/// Returns `WasmError::EngineCreation` if the Wasmtime engine cannot be initialized.
/// This typically only occurs if the system is fundamentally misconfigured or lacks
/// required resources.
fn get_or_init_engine() -> Result<Arc<Engine>> {
    if let Some(engine) = SHARED_ENGINE.get() {
        return Ok(Arc::clone(engine));
    }

    internal!(
        level = TRACE,
        "Creating shared Wasmtime engine with epoch interruption"
    );

    let mut config = Config::new();
    config.wasm_component_model(true);
    config.epoch_interruption(true); // Enable timeout enforcement

    let engine = Engine::new(&config)
        .map(Arc::new)
        .map_err(|e| WasmError::EngineCreation(e.to_string()))?;

    // Set may fail if another thread initialized concurrently; fall back to the stored value.
    if SHARED_ENGINE.set(Arc::clone(&engine)).is_ok() {
        Ok(engine)
    } else {
        SHARED_ENGINE
            .get()
            .map(Arc::clone)
            .ok_or_else(|| WasmError::EngineCreation("Failed to initialize Wasmtime engine".into()))
    }
}

/// Start the epoch manager task if not already running
///
/// This spawns a tokio task that increments the engine's epoch counter every 10ms.
/// The task runs indefinitely but uses minimal resources (just incrementing a counter).
fn start_epoch_manager(engine: Arc<Engine>) {
    // Only start the task once using atomic compare-and-swap
    if !EPOCH_TASK_STARTED.swap(true, Ordering::SeqCst) {
        internal!(level = TRACE, "Starting epoch manager task (10ms interval)");

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(EPOCH_TICK_MS));
            // Skip missed ticks if the runtime is overloaded
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

            loop {
                interval.tick().await;
                engine.increment_epoch();
            }
        });
    }
}

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
        tracking_id: ctx.tracking_id.as_ref().map(ToString::to_string),
    }
}

/// Convert validation result to return code
///
/// Returns 0 for Accept, or the SMTP error code for Reject
///
/// # SMTP Code Validation (RFC 5321)
///
/// Validates that rejection codes are in the valid SMTP range (400-599).
/// Invalid codes are replaced with 550 (Requested action not taken) to ensure
/// RFC compliance and prevent protocol errors.
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
            let code = rejection.code.cast_signed();

            // Validate SMTP code is in valid range (400-599 per RFC 5321)
            let validated_code = if (400..=599).contains(&code) {
                code
            } else {
                internal!(
                    level = WARN,
                    "WASM callback '{callback_name}' returned invalid SMTP code {}, using 550 instead",
                    code
                );
                550 // Default to "Requested action not taken" for invalid codes
            };

            internal!(
                level = INFO,
                "WASM callback '{callback_name}' returned: Reject({}, {})",
                validated_code,
                rejection.message
            );

            validated_code
        }
    }
}

/// Check if a Wasmtime error is caused by an epoch timeout
fn is_timeout_error(err: &wasmtime::Error) -> bool {
    // Prefer structured information when available.
    if let Some(trap) = err.downcast_ref::<wasmtime::Trap>()
        && trap.to_string().to_ascii_lowercase().contains("interrupt")
    {
        return true;
    }

    // Fallback to string search for host-propagated errors.
    let combined = format!("{err:?} {err}").to_ascii_lowercase();
    combined.contains("epoch deadline exceeded") || combined.contains("interrupt")
}

/// Call a validation callback and return the result code
///
/// # Errors
/// Returns `WasmError::Timeout` if the callback exceeds the configured timeout
fn call_validation_callback(
    callback_name: &str,
    validator: &Validator,
    store: &mut Store<WasmStore>,
    wit_context: &wit::Context,
) -> Result<i32> {
    // Capture timeout before calling (to avoid borrow issues)
    let timeout_secs = store.data().timeout.as_secs();

    let result = match callback_name {
        "validate_connect" => validator
            .empath_validator_validation()
            .call_validate_connect(store, wit_context)
            .map_err(|e| {
                if is_timeout_error(&e) {
                    WasmError::Timeout(callback_name.to_string(), timeout_secs)
                } else {
                    e.into()
                }
            })?,
        "validate_ehlo" => validator
            .empath_validator_validation()
            .call_validate_ehlo(store, wit_context)
            .map_err(|e| {
                if is_timeout_error(&e) {
                    WasmError::Timeout(callback_name.to_string(), timeout_secs)
                } else {
                    e.into()
                }
            })?,
        "validate_mail_from" => validator
            .empath_validator_validation()
            .call_validate_mail_from(store, wit_context)
            .map_err(|e| {
                if is_timeout_error(&e) {
                    WasmError::Timeout(callback_name.to_string(), timeout_secs)
                } else {
                    e.into()
                }
            })?,
        "validate_rcpt_to" => validator
            .empath_validator_validation()
            .call_validate_rcpt_to(store, wit_context)
            .map_err(|e| {
                if is_timeout_error(&e) {
                    WasmError::Timeout(callback_name.to_string(), timeout_secs)
                } else {
                    e.into()
                }
            })?,
        "validate_data" => validator
            .empath_validator_validation()
            .call_validate_data(store, wit_context)
            .map_err(|e| {
                if is_timeout_error(&e) {
                    WasmError::Timeout(callback_name.to_string(), timeout_secs)
                } else {
                    e.into()
                }
            })?,
        "validate_starttls" => validator
            .empath_validator_validation()
            .call_validate_starttls(store, wit_context)
            .map_err(|e| {
                if is_timeout_error(&e) {
                    WasmError::Timeout(callback_name.to_string(), timeout_secs)
                } else {
                    e.into()
                }
            })?,
        _ => return Ok(-1), // Unknown callback
    };

    Ok(validation_result_to_code(callback_name, result))
}

/// Call an event callback
///
/// # Errors
/// Returns `WasmError::Timeout` if the callback exceeds the configured timeout
fn call_event_callback(
    callback_name: &str,
    validator: &Validator,
    store: &mut Store<WasmStore>,
    wit_context: &wit::Context,
) -> Result<()> {
    // Capture timeout before calling (to avoid borrow issues)
    let timeout_secs = store.data().timeout.as_secs();

    match callback_name {
        "on_connection_opened" => validator
            .empath_validator_events()
            .call_on_connection_opened(store, wit_context)
            .map_err(|e| {
                if is_timeout_error(&e) {
                    WasmError::Timeout(callback_name.to_string(), timeout_secs)
                } else {
                    e.into()
                }
            })?,
        "on_connection_closed" => validator
            .empath_validator_events()
            .call_on_connection_closed(store, wit_context)
            .map_err(|e| {
                if is_timeout_error(&e) {
                    WasmError::Timeout(callback_name.to_string(), timeout_secs)
                } else {
                    e.into()
                }
            })?,
        "on_delivery_attempt" => validator
            .empath_validator_events()
            .call_on_delivery_attempt(store, wit_context)
            .map_err(|e| {
                if is_timeout_error(&e) {
                    WasmError::Timeout(callback_name.to_string(), timeout_secs)
                } else {
                    e.into()
                }
            })?,
        "on_delivery_success" => validator
            .empath_validator_events()
            .call_on_delivery_success(store, wit_context)
            .map_err(|e| {
                if is_timeout_error(&e) {
                    WasmError::Timeout(callback_name.to_string(), timeout_secs)
                } else {
                    e.into()
                }
            })?,
        "on_delivery_failure" => validator
            .empath_validator_events()
            .call_on_delivery_failure(store, wit_context)
            .map_err(|e| {
                if is_timeout_error(&e) {
                    WasmError::Timeout(callback_name.to_string(), timeout_secs)
                } else {
                    e.into()
                }
            })?,
        "on_smtp_error" => validator
            .empath_validator_events()
            .call_on_smtp_error(store, wit_context)
            .map_err(|e| {
                if is_timeout_error(&e) {
                    WasmError::Timeout(callback_name.to_string(), timeout_secs)
                } else {
                    e.into()
                }
            })?,
        "on_smtp_message_received" => validator
            .empath_validator_events()
            .call_on_smtp_message_received(store, wit_context)
            .map_err(|e| {
                if is_timeout_error(&e) {
                    WasmError::Timeout(callback_name.to_string(), timeout_secs)
                } else {
                    e.into()
                }
            })?,
        "on_dns_lookup" => validator
            .empath_validator_events()
            .call_on_dns_lookup(store, wit_context)
            .map_err(|e| {
                if is_timeout_error(&e) {
                    WasmError::Timeout(callback_name.to_string(), timeout_secs)
                } else {
                    e.into()
                }
            })?,
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

    /// Shared Wasmtime engine (with epoch interruption enabled for timeouts)
    engine: Arc<Engine>,

    /// Compiled WASM component
    component: Component,

    /// Execution timeout (enforced via epoch deadlines)
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

        // Get or create the shared Wasmtime engine (with epoch interruption enabled)
        let engine = get_or_init_engine()?;

        // Start the epoch manager task (only runs once across all modules)
        start_epoch_manager(Arc::clone(&engine));

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
        let wasm_store =
            WasmStore::new(dummy_context, self.max_memory_bytes, self.timeout, wasi_ctx)?;
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
    /// This function intentionally panics under specific conditions that indicate programming
    /// errors or corrupted state:
    ///
    /// - **Line 529**: If the context mutex lock fails. This indicates the mutex was poisoned
    ///   by a panic in a host function, meaning the context is in an unknown state. Continuing
    ///   would be unsafe.
    ///
    /// - **Line 567**: If `Arc::try_unwrap()` fails, indicating more than one strong reference
    ///   to the context Arc exists. This should never happen as the store (which holds the
    ///   only other reference) is explicitly dropped at line 560.
    ///
    /// - **Line 569**: If `Mutex::into_inner()` fails due to the mutex being poisoned. This
    ///   would indicate a panic occurred in a host function during callback execution.
    ///
    /// All these conditions represent unrecoverable states where continuing execution could
    /// lead to data corruption. Panicking is the intentional fail-fast behavior.
    #[allow(clippy::expect_used, reason = "Panic on poisoned mutex is intentional")]
    pub fn execute_callback(&self, callback_name: &str, context: &mut Context) -> Result<i32> {
        // Build WASI context
        let wasi_ctx = self.build_wasi_ctx()?;

        // Wrap context in Arc<Mutex<>> for thread-safe access from host functions
        // Using clone instead of mem::take ensures the caller retains a valid context
        // even if an error occurs during execution. This is now cheap because
        // Envelope is wrapped in Arc (NEW-WASM-11), making clone ~80% faster.
        let context_arc = Arc::new(Mutex::new(context.clone()));

        // Create store with wrapped context
        let wasm_store = WasmStore::new(
            Arc::clone(&context_arc),
            self.max_memory_bytes,
            self.timeout,
            wasi_ctx,
        )?;
        let mut store = Store::new(&self.engine, wasm_store);
        store.limiter(|s| s);

        // Set epoch deadline for timeout enforcement
        // Convert timeout to epoch ticks (10ms per tick)
        let timeout_ticks = self.timeout.as_millis() / u128::from(EPOCH_TICK_MS);
        #[allow(
            clippy::cast_possible_truncation,
            reason = "Timeouts are expected to be reasonable (<584 years)"
        )]
        store.set_epoch_deadline(timeout_ticks as u64);
        internal!(
            level = TRACE,
            "Set epoch deadline: {} ticks ({:?} timeout)",
            timeout_ticks,
            self.timeout
        );

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
            let ctx_lock = context_arc
                .lock()
                .map_err(|e| WasmError::Internal(format!("Failed to lock context mutex: {e}")))?;
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
        // try_unwrap fails if there are multiple Arc references
        // into_inner fails if the mutex is poisoned (would require a panic in host functions)
        let result_context = Arc::try_unwrap(context_arc)
            .map_err(|arc| {
                WasmError::Internal(format!(
                    "Context Arc has {} strong references (expected 1)",
                    Arc::strong_count(&arc)
                ))
            })?
            .into_inner()
            .map_err(|e| WasmError::Internal(format!("Context mutex poisoned: {e}")))?;

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
    #[cfg_attr(miri, ignore)]
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

    #[test]
    fn validation_result_to_code_accept_is_zero() {
        let result = wit::ValidationResult::Accept;
        assert_eq!(validation_result_to_code("accept", result), 0);
    }

    #[test]
    fn validation_result_to_code_clamps_invalid_codes() {
        let rejection = wit::Rejection {
            code: 999,
            message: "bad code".to_string(),
        };
        let result = wit::ValidationResult::Reject(rejection);
        assert_eq!(validation_result_to_code("invalid", result), 550);
    }

    #[test]
    fn timeout_error_detection_matches_epoch_message() {
        let timeout_err = wasmtime::Error::msg("epoch deadline exceeded");
        assert!(is_timeout_error(&timeout_err));

        // Use an error that is not an interrupt
        let other_err = wasmtime::Error::msg("component instantiation failed");
        assert!(
            !is_timeout_error(&other_err),
            "non-interrupt traps should not be treated as timeout"
        );
    }
}
