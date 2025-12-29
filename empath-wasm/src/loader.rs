//! WASM module loader and runtime management
//!
//! This module handles loading WASM modules, managing the Wasmtime engine and store,
//! and executing callbacks with resource limits and timeouts.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use empath_common::context::Context;
use empath_common::internal;
use wasmtime::component::{Component, Linker};
use wasmtime::{Config, Engine, Store};
use wasmtime_wasi::p2::add_to_linker_sync;
use wasmtime_wasi::{DirPerms, FilePerms, WasiCtxBuilder};

use crate::bindings::Validator;
use crate::bindings::exports::empath::validator::{events as wit_events, validation as wit};
use crate::error::{Result, WasmError};
use crate::host::WasmStore;

/// Default timeout for callback execution (5 seconds)
const DEFAULT_TIMEOUT_SECS: u64 = 5;

/// Default maximum memory for WASM modules (10 MB)
const DEFAULT_MAX_MEMORY_BYTES: usize = 10 * 1024 * 1024;

/// Epoch tick interval in milliseconds (10ms = 100Hz)
const EPOCH_TICK_MS: u64 = 10;

/// Deterministic WASI configuration for reproducible module tests.
#[derive(Clone, Debug)]
pub struct DeterministicWasiConfig {
    pub wall_clock: Duration,
    pub monotonic_clock: u64,
    pub random_seed: u64,
    pub insecure_seed: u128,
}

impl Default for DeterministicWasiConfig {
    fn default() -> Self {
        Self {
            wall_clock: Duration::from_secs(0),
            monotonic_clock: 0,
            random_seed: 0x4D59_5DF4_D0F3_3173,
            insecure_seed: 0,
        }
    }
}

#[derive(Debug)]
struct FixedWallClock {
    now: Duration,
    resolution: Duration,
}

impl FixedWallClock {
    const fn new(now: Duration) -> Self {
        Self {
            now,
            resolution: Duration::from_nanos(1),
        }
    }
}

impl wasmtime_wasi::HostWallClock for FixedWallClock {
    fn resolution(&self) -> Duration {
        self.resolution
    }

    fn now(&self) -> Duration {
        self.now
    }
}

#[derive(Debug)]
struct FixedMonotonicClock {
    now: u64,
    resolution: u64,
}

impl FixedMonotonicClock {
    const fn new(now: u64) -> Self {
        Self { now, resolution: 1 }
    }
}

impl wasmtime_wasi::HostMonotonicClock for FixedMonotonicClock {
    fn resolution(&self) -> u64 {
        self.resolution
    }

    fn now(&self) -> u64 {
        self.now
    }
}

#[derive(Debug)]
struct DeterministicRng {
    state: u64,
}

impl DeterministicRng {
    const INCREMENT: u64 = 1;
    const MULTIPLIER: u64 = 6_364_136_223_846_793_005;

    const fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    const fn next_state(&mut self) -> u64 {
        self.state = self
            .state
            .wrapping_mul(Self::MULTIPLIER)
            .wrapping_add(Self::INCREMENT);
        self.state
    }
}

impl wasmtime_wasi::RngCore for DeterministicRng {
    fn next_u32(&mut self) -> u32 {
        (self.next_state() >> 32) as u32
    }

    fn next_u64(&mut self) -> u64 {
        self.next_state()
    }

    fn fill_bytes(&mut self, dest: &mut [u8]) {
        for chunk in dest.chunks_mut(8) {
            let bytes = self.next_u64().to_le_bytes();
            let len = chunk.len();
            chunk.copy_from_slice(&bytes[..len]);
        }
    }

    fn try_fill_bytes(&mut self, dest: &mut [u8]) -> std::result::Result<(), rand_core::Error> {
        self.fill_bytes(dest);
        Ok(())
    }
}

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

impl From<&Context> for wit::ValidationContext {
    fn from(ctx: &Context) -> Self {
        let mut transaction = Vec::with_capacity(ctx.transaction.len());
        for (key, value) in &ctx.transaction {
            transaction.push((key.to_string(), value.clone()));
        }

        Self {
            extended: ctx.extended,
            id: ctx.id.clone(),
            sender: Some(ctx.sender()),
            recipients: ctx.recipients(),
            max_message_size: ctx.max_message_size as u64,
            transaction,
            tracking_id: ctx.tracking_id.as_ref().map(ToString::to_string),
        }
    }
}

impl From<&Context> for wit_events::EventContext {
    fn from(ctx: &Context) -> Self {
        let mut transaction = Vec::with_capacity(ctx.transaction.len());
        for (key, value) in &ctx.transaction {
            transaction.push((key.to_string(), value.clone()));
        }

        Self {
            extended: ctx.extended,
            id: ctx.id.clone(),
            sender: Some(ctx.sender()),
            recipients: ctx.recipients(),
            max_message_size: ctx.max_message_size as u64,
            transaction,
            tracking_id: ctx.tracking_id.as_ref().map(ToString::to_string),
        }
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
    wit_context: &wit::ValidationContext,
) -> Result<i32> {
    let Some(validation) = WasmValidation::from_name(callback_name) else {
        return Ok(-1); // Unknown callback
    };

    validation.dispatch(validator, store, wit_context)
}

/// Call an event callback
///
/// # Errors
/// Returns `WasmError::Timeout` if the callback exceeds the configured timeout
fn call_event_callback(
    callback_name: &str,
    validator: &Validator,
    store: &mut Store<WasmStore>,
    wit_context: &wit_events::EventContext,
) -> Result<()> {
    let Some(event) = WasmEvent::from_name(callback_name) else {
        return Ok(()); // Unknown callback
    };

    event.dispatch(validator, store, wit_context)
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

    /// Configured outbound bindings for validation helpers
    outbound_bindings: Arc<[String]>,

    /// WASI preopened directories: (`host_path`, `guest_name`, readonly)
    wasi_dirs: Vec<(String, String, bool)>,

    /// Inherit host environment variables
    wasi_inherit_env: bool,

    /// Deterministic WASI overrides for tests
    deterministic_wasi: Option<DeterministicWasiConfig>,
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
            outbound_bindings: Arc::from(Vec::<String>::new().into_boxed_slice()),
            wasi_dirs: Vec::new(),
            wasi_inherit_env: false,
            deterministic_wasi: None,
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

    /// Set the configured outbound bindings for validation helpers.
    #[must_use]
    pub fn with_outbound_bindings(mut self, bindings: Vec<String>) -> Self {
        self.outbound_bindings = Arc::from(bindings.into_boxed_slice());
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

    /// Enable deterministic clocks and RNG for reproducible WASM tests.
    #[must_use]
    pub const fn with_deterministic_wasi(mut self, config: DeterministicWasiConfig) -> Self {
        self.deterministic_wasi = Some(config);
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
        let wasm_store = WasmStore::new(
            dummy_context,
            self.max_memory_bytes,
            self.timeout,
            wasi_ctx,
            Arc::clone(&self.outbound_bindings),
        )?;
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
            Arc::clone(&self.outbound_bindings),
        )?;
        let mut store = Store::new(&self.engine, wasm_store);
        store.limiter(|s| s);

        // Set epoch deadline for timeout enforcement
        // Convert timeout to epoch ticks (10ms per tick)
        let timeout_ticks = self.timeout.as_millis() / u128::from(EPOCH_TICK_MS);
        let timeout_ticks = u64::try_from(timeout_ticks).unwrap_or(u64::MAX);
        store.set_epoch_deadline(timeout_ticks);
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

        // Call the appropriate validation or event function based on callback name
        let return_code = if callback_name.starts_with("validate_") {
            // Convert context to WIT validation format
            let wit_context = {
                let ctx_lock = context_arc.lock().map_err(|e| {
                    WasmError::Internal(format!("Failed to lock context mutex: {e}"))
                })?;
                wit::ValidationContext::from(&*ctx_lock)
            };
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
            // Convert context to WIT event format
            let wit_context = {
                let ctx_lock = context_arc.lock().map_err(|e| {
                    WasmError::Internal(format!("Failed to lock context mutex: {e}"))
                })?;
                wit_events::EventContext::from(&*ctx_lock)
            };
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

        if let Some(config) = &self.deterministic_wasi {
            builder.wall_clock(FixedWallClock::new(config.wall_clock));
            builder.monotonic_clock(FixedMonotonicClock::new(config.monotonic_clock));
            builder.secure_random(DeterministicRng::new(config.random_seed));
            builder.insecure_random(DeterministicRng::new(
                config.random_seed ^ 0x9E37_79B9_7F4A_7C15,
            ));
            builder.insecure_random_seed(config.insecure_seed);
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

macro_rules! define_wasm_validations {
    (
        $(
            $variant:ident {
                name: $name:literal,
                call: $call:ident,
            }
        ),+ $(,)?
    ) => {
        #[derive(Copy, Clone, Debug, Eq, PartialEq)]
        enum WasmValidation {
            $( $variant ),*
        }

        impl WasmValidation {
            const fn name(self) -> &'static str {
                match self {
                    $( WasmValidation::$variant => $name ),*
                }
            }

            fn from_name(name: &str) -> Option<Self> {
                match name {
                    $( $name => Some(WasmValidation::$variant) ),*,
                    _ => None,
                }
            }

            fn dispatch(
                self,
                validator: &Validator,
                store: &mut Store<WasmStore>,
                wit_context: &wit::ValidationContext,
            ) -> Result<i32> {
                let callback_name = self.name();
                let timeout_secs = store.data().timeout.as_secs();

                let call_result = match self {
                    $(
                        WasmValidation::$variant => validator
                            .empath_validator_validation()
                            .$call(store, wit_context),
                    )*
                };

                let result = call_result.map_err(|e| {
                    if is_timeout_error(&e) {
                        WasmError::Timeout(callback_name.to_string(), timeout_secs)
                    } else {
                        e.into()
                    }
                })?;

                Ok(validation_result_to_code(callback_name, result))
            }
        }
    };
}

define_wasm_validations! {
    Connect {
        name: "validate_connect",
        call: call_validate_connect,
    },
    Ehlo {
        name: "validate_ehlo",
        call: call_validate_ehlo,
    },
    MailFrom {
        name: "validate_mail_from",
        call: call_validate_mail_from,
    },
    RcptTo {
        name: "validate_rcpt_to",
        call: call_validate_rcpt_to,
    },
    Data {
        name: "validate_data",
        call: call_validate_data,
    },
    Starttls {
        name: "validate_starttls",
        call: call_validate_starttls,
    },
}

macro_rules! define_wasm_events {
    (
        $(
            $variant:ident {
                name: $name:literal,
                call: $call:ident,
            }
        ),+ $(,)?
    ) => {
        #[derive(Copy, Clone, Debug, Eq, PartialEq)]
        enum WasmEvent {
            $( $variant ),*
        }

        impl WasmEvent {
            const fn name(self) -> &'static str {
                match self {
                    $( WasmEvent::$variant => $name ),*
                }
            }

            fn from_name(name: &str) -> Option<Self> {
                match name {
                    $( $name => Some(WasmEvent::$variant) ),*,
                    _ => None,
                }
            }

            fn dispatch(
                self,
                validator: &Validator,
                store: &mut Store<WasmStore>,
                wit_context: &wit_events::EventContext,
            ) -> Result<()> {
                let callback_name = self.name();
                let timeout_secs = store.data().timeout.as_secs();

                let call_result = match self {
                    $( WasmEvent::$variant => validator.empath_validator_events().$call(store, wit_context) ),*
                };

                call_result.map_err(|e| {
                    if is_timeout_error(&e) {
                        WasmError::Timeout(callback_name.to_string(), timeout_secs)
                    } else {
                        e.into()
                    }
                })?;

                internal!(level = DEBUG, "WASM callback '{callback_name}' completed");
                Ok(())
            }
        }
    };
}

define_wasm_events! {
    ConnectionOpened {
        name: "on_connection_opened",
        call: call_on_connection_opened,
    },
    ConnectionClosed {
        name: "on_connection_closed",
        call: call_on_connection_closed,
    },
    DeliveryAttempt {
        name: "on_delivery_attempt",
        call: call_on_delivery_attempt,
    },
    DeliverySuccess {
        name: "on_delivery_success",
        call: call_on_delivery_success,
    },
    DeliveryFailure {
        name: "on_delivery_failure",
        call: call_on_delivery_failure,
    },
    SmtpError {
        name: "on_smtp_error",
        call: call_on_smtp_error,
    },
    SmtpMessageReceived {
        name: "on_smtp_message_received",
        call: call_on_smtp_message_received,
    },
    DnsLookup {
        name: "on_dns_lookup",
        call: call_on_dns_lookup,
    },
}

#[cfg(test)]
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

    #[test]
    fn wasm_event_mapping_round_trip() {
        let pairs = [
            ("on_connection_opened", WasmEvent::ConnectionOpened),
            ("on_connection_closed", WasmEvent::ConnectionClosed),
            ("on_delivery_attempt", WasmEvent::DeliveryAttempt),
            ("on_delivery_success", WasmEvent::DeliverySuccess),
            ("on_delivery_failure", WasmEvent::DeliveryFailure),
            ("on_smtp_error", WasmEvent::SmtpError),
            ("on_smtp_message_received", WasmEvent::SmtpMessageReceived),
            ("on_dns_lookup", WasmEvent::DnsLookup),
        ];

        for (name, event) in pairs {
            assert_eq!(WasmEvent::from_name(name), Some(event));
            assert_eq!(event.name(), name);
        }
    }

    #[test]
    fn wasm_event_from_name_unknown_returns_none() {
        assert!(WasmEvent::from_name("on_unknown_event").is_none());
    }

    #[test]
    fn wasm_validation_mapping_round_trip() {
        let pairs = [
            ("validate_connect", WasmValidation::Connect),
            ("validate_ehlo", WasmValidation::Ehlo),
            ("validate_mail_from", WasmValidation::MailFrom),
            ("validate_rcpt_to", WasmValidation::RcptTo),
            ("validate_data", WasmValidation::Data),
            ("validate_starttls", WasmValidation::Starttls),
        ];

        for (name, validation) in pairs {
            assert_eq!(WasmValidation::from_name(name), Some(validation));
            assert_eq!(validation.name(), name);
        }
    }

    #[test]
    fn wasm_validation_from_name_unknown_returns_none() {
        assert!(WasmValidation::from_name("validate_unknown").is_none());
    }
}
