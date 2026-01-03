use std::borrow::Cow;
use std::fmt::{self, Display};
use std::sync::{Arc, OnceLock, RwLock};
use std::time::{Duration, Instant};

use empath_common::context::{Context, ModuleContext};
use empath_common::internal;
use empath_common::status::Status;
use empath_metrics::is_enabled;
#[cfg(feature = "metrics")]
use empath_metrics::metrics;
use empath_tracing::traced;
use serde::Deserialize;
use thiserror::Error;
use tokio::task;
use tokio::time::sleep;

use super::string::StringVector;

pub mod core;
pub mod library;
#[cfg(feature = "metrics")]
pub mod metrics;
#[cfg(feature = "python")]
pub mod python;
pub mod validate;
#[cfg(feature = "wasm")]
pub mod wasm;

#[derive(Debug)]
pub struct ModuleRegistry {
    modules: Arc<[Arc<Module>]>,
    outbound_bindings: Arc<RwLock<Vec<String>>>,
    dispatch_config: ModuleDispatchConfig,
}

pub struct ModuleRegistryBuilder {
    modules: Vec<Module>,
    outbound_bindings: Vec<String>,
    dispatch_config: ModuleDispatchConfig,
}

impl Default for ModuleRegistryBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl ModuleRegistryBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self {
            modules: Vec::new(),
            outbound_bindings: Vec::new(),
            dispatch_config: ModuleDispatchConfig::default(),
        }
    }

    #[must_use]
    pub fn with_modules(mut self, modules: Vec<Module>) -> Self {
        self.modules = modules;
        self
    }

    #[must_use]
    pub fn with_outbound_bindings(mut self, bindings: Vec<String>) -> Self {
        self.outbound_bindings = bindings;
        self
    }

    #[must_use]
    pub const fn with_dispatch_config(mut self, dispatch_config: ModuleDispatchConfig) -> Self {
        self.dispatch_config = dispatch_config;
        self
    }

    /// Build and initialize the module registry.
    ///
    /// # Errors
    /// Returns an error if any module fails to initialize.
    #[traced(instrument(level = tracing::Level::TRACE, ret, skip(self)), timing)]
    pub fn build(mut self) -> Result<ModuleRegistry, Error> {
        internal!(level = INFO, "Initialising modules ...");

        // Add core module first so it runs before other validation modules
        let mut all_modules = vec![
            Module::Core {
                validators: Arc::new(core::CoreValidators::new()),
            },
            #[cfg(debug_assertions)]
            Module::TestModule(RwLock::default()),
        ];

        // Add metrics module if enabled
        #[cfg(feature = "metrics")]
        if is_enabled() {
            all_modules.push(Module::Metrics);
        }

        all_modules.append(&mut self.modules);

        let init_context = ModuleInitContext {
            outbound_bindings: self.outbound_bindings.clone(),
        };

        all_modules
            .iter_mut()
            .inspect(|module| internal!("Init: {module}"))
            .try_for_each(|module| module.init_with(&init_context))?;

        internal!(level = INFO, "Modules initialised");

        let modules = all_modules.into_iter().map(Arc::new).collect::<Vec<_>>();

        Ok(ModuleRegistry {
            modules: modules.into(),
            outbound_bindings: Arc::new(RwLock::new(self.outbound_bindings)),
            dispatch_config: self.dispatch_config,
        })
    }
}

/// Dispatch failure handling policy for module callbacks.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DispatchFailureMode {
    FailOpen,
    FailClosed,
}

const fn default_validation_failure_mode() -> DispatchFailureMode {
    DispatchFailureMode::FailClosed
}

const fn default_event_failure_mode() -> DispatchFailureMode {
    DispatchFailureMode::FailOpen
}

/// Module dispatch isolation strategy.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ModuleIsolation {
    #[default]
    None,
    SpawnBlocking,
}

/// Configuration for module dispatch behavior.
#[derive(Clone, Debug, Deserialize)]
pub struct ModuleDispatchConfig {
    /// Timeout for validation callbacks (seconds).
    #[serde(default)]
    pub validation_timeout_secs: Option<u64>,
    /// Timeout for event callbacks (seconds).
    #[serde(default)]
    pub event_timeout_secs: Option<u64>,
    /// Failure policy for validation callbacks.
    #[serde(default = "default_validation_failure_mode")]
    pub validation_failure_mode: DispatchFailureMode,
    /// Failure policy for event callbacks.
    #[serde(default = "default_event_failure_mode")]
    pub event_failure_mode: DispatchFailureMode,
    /// Isolation strategy for module execution.
    #[serde(default)]
    pub isolation: ModuleIsolation,
}

impl Default for ModuleDispatchConfig {
    fn default() -> Self {
        Self {
            validation_timeout_secs: None,
            event_timeout_secs: None,
            validation_failure_mode: default_validation_failure_mode(),
            event_failure_mode: default_event_failure_mode(),
            isolation: ModuleIsolation::None,
        }
    }
}

impl ModuleDispatchConfig {
    fn timeout_for(&self, event: Event) -> Option<Duration> {
        match event {
            Event::Validate(_) => self.validation_timeout_secs.map(Duration::from_secs),
            Event::Event(_) => self.event_timeout_secs.map(Duration::from_secs),
        }
    }

    const fn failure_mode_for(&self, event: Event) -> DispatchFailureMode {
        match event {
            Event::Validate(_) => self.validation_failure_mode,
            Event::Event(_) => self.event_failure_mode,
        }
    }
}

static MODULE_REGISTRY: OnceLock<ModuleRegistry> = OnceLock::new();

impl ModuleRegistry {
    #[must_use]
    pub fn global() -> Option<&'static Self> {
        MODULE_REGISTRY.get()
    }

    /// Install this registry as the global instance.
    ///
    /// # Errors
    ///
    /// Returns an error if the global registry cannot be installed.
    pub fn install(self) -> Result<&'static Self, Error> {
        if let Some(existing) = MODULE_REGISTRY.get() {
            return Ok(existing);
        }

        let _ = MODULE_REGISTRY.set(self);
        MODULE_REGISTRY
            .get()
            .ok_or_else(|| Error::Init("Module registry missing after install".to_string()))
    }

    #[must_use]
    pub fn modules(&self) -> &[Arc<Module>] {
        &self.modules
    }

    /// Replace the configured outbound bindings list for module helpers.
    pub fn set_outbound_bindings(&self, bindings: Vec<String>) {
        if let Ok(mut guard) = self.outbound_bindings.write() {
            *guard = bindings;
        }
    }

    /// Fetch configured outbound bindings for module helpers.
    #[must_use]
    pub fn outbound_bindings(&self) -> Vec<String> {
        self.outbound_bindings
            .read()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }

    /// Validate that a binding name exists in configuration.
    #[must_use]
    pub fn outbound_binding_exists(&self, name: &str) -> bool {
        self.outbound_bindings
            .read()
            .map(|guard| guard.iter().any(|binding| binding == name))
            .unwrap_or(false)
    }

    /// Dispatch an event to all modules.
    ///
    /// Failure handling is governed by `ModuleDispatchConfig`.
    ///
    /// # Errors
    /// The events being dispatched are handled with a panic handler, which should
    /// catch some possible errors. If these are caught, they are returned to the
    /// caller to handle.
    pub async fn dispatch(&self, event: Event, validate_context: &mut Context) -> DispatchResult {
        internal!("Dispatching: {event:?}");

        let failure_mode = self.dispatch_config.failure_mode_for(event);
        let timeout = self.dispatch_config.timeout_for(event);
        let callback_name = event.callback_name();
        let mut event_error: Option<DispatchResult> = None;

        for module in self
            .modules
            .iter()
            .inspect(|m| internal!(level = DEBUG, "{}", m.as_ref()))
        {
            let module_context = module.as_ref().module_context();
            let outcome = self
                .emit_with_policy(
                    Arc::clone(module),
                    event,
                    validate_context,
                    module_context.clone(),
                    failure_mode,
                    timeout,
                )
                .await;

            #[cfg(feature = "metrics")]
            if let Some(metrics) = metrics() {
                metrics.modules.record_execution(
                    &module_context.module_type,
                    callback_name,
                    outcome.duration.as_secs_f64(),
                );
            }

            if let Some(kind) = outcome.error_kind.as_ref() {
                record_dispatch_error_metrics(&module_context, event, kind);
            }

            if outcome.error_kind.is_none() && outcome.code == 0 {
                continue;
            }

            match event {
                Event::Validate(_) => {
                    if matches!(failure_mode, DispatchFailureMode::FailOpen) {
                        log_dispatch_failure(event, module.as_ref(), &outcome);
                        continue;
                    }

                    let code = outcome.code;
                    return rejection_from_code(code, validate_context);
                }
                Event::Event(_) => {
                    let module_name = format!("{}", module.as_ref());
                    let kind = outcome
                        .error_kind
                        .clone()
                        .unwrap_or(DispatchErrorKind::ModuleFailed { code: outcome.code });

                    if matches!(failure_mode, DispatchFailureMode::FailClosed) {
                        return DispatchResult::Error {
                            module: module_name,
                            kind,
                        };
                    }

                    log_dispatch_failure(event, module.as_ref(), &outcome);
                    if event_error.is_none() {
                        event_error = Some(DispatchResult::Error {
                            module: module_name,
                            kind,
                        });
                    }
                }
            }
        }

        event_error.unwrap_or(DispatchResult::Accept)
    }

    pub async fn dispatch_with_registry(
        registry: Option<&Self>,
        event: Event,
        validate_context: &mut Context,
    ) -> DispatchResult {
        let Some(registry) = registry else {
            internal!(
                level = ERROR,
                "Module store not initialized. Treating as dispatch failure."
            );
            if matches!(event, Event::Validate(_)) {
                validate_context.response = Some((
                    Status::ActionUnavailable,
                    Cow::Borrowed("Validation modules unavailable"),
                ));
            }
            return DispatchResult::Error {
                module: "module-store".to_string(),
                kind: DispatchErrorKind::NotInitialized,
            };
        };

        registry.dispatch(event, validate_context).await
    }

    async fn emit_with_policy(
        &self,
        module: Arc<Module>,
        event: Event,
        validate_context: &mut Context,
        module_context: ModuleContext,
        failure_mode: DispatchFailureMode,
        timeout_limit: Option<Duration>,
    ) -> ModuleDispatchOutcome {
        let use_isolation = matches!(
            self.dispatch_config.isolation,
            ModuleIsolation::SpawnBlocking
        );
        let use_clone = matches!(failure_mode, DispatchFailureMode::FailOpen) || use_isolation;

        if use_clone {
            let working_context = validate_context.clone();
            let (outcome, updated_context) = if use_isolation {
                emit_isolated(
                    module,
                    event,
                    working_context,
                    module_context,
                    timeout_limit,
                )
                .await
            } else {
                let (outcome, updated_context) = emit_inline(
                    module.as_ref(),
                    event,
                    working_context,
                    &module_context,
                    timeout_limit,
                );
                (outcome, Some(updated_context))
            };

            if outcome.error_kind.is_none()
                && let Some(updated_context) = updated_context
            {
                apply_context_update(
                    validate_context,
                    updated_context,
                    failure_mode,
                    outcome.code,
                    true,
                );
            }

            return outcome;
        }

        emit_inline_in_place(
            module.as_ref(),
            event,
            validate_context,
            &module_context,
            timeout_limit,
        )
    }
}

const MODULE_FAILURE_CODE: i32 = 451;

struct ModuleDispatchOutcome {
    code: i32,
    duration: Duration,
    error_kind: Option<DispatchErrorKind>,
}

fn emit_in_context(
    module: &Module,
    event: Event,
    context: &mut Context,
    module_context: &ModuleContext,
) -> i32 {
    let previous = context.module_context.take();
    context.module_context = Some(module_context.clone());
    let code = module.emit(event, context);
    context.module_context = previous;
    code
}

fn timeout_overrun(limit: Duration, elapsed: Duration) -> Option<Duration> {
    if elapsed > limit { Some(limit) } else { None }
}

fn apply_context_update(
    target: &mut Context,
    updated: Context,
    failure_mode: DispatchFailureMode,
    code: i32,
    completed: bool,
) {
    if !completed {
        return;
    }

    let should_apply = matches!(failure_mode, DispatchFailureMode::FailClosed) || code == 0;
    if should_apply {
        *target = updated;
    }
}

async fn emit_isolated(
    module: Arc<Module>,
    event: Event,
    mut context: Context,
    module_context: ModuleContext,
    timeout_limit: Option<Duration>,
) -> (ModuleDispatchOutcome, Option<Context>) {
    let started_at = Instant::now();
    let mut handle = task::spawn_blocking(move || {
        let start = Instant::now();
        let code = emit_in_context(module.as_ref(), event, &mut context, &module_context);
        (code, start.elapsed(), context)
    });

    if let Some(timeout_limit) = timeout_limit {
        let outcome = tokio::select! {
            result = &mut handle => match result {
                Ok((code, duration, updated_context)) => {
                    return (
                        ModuleDispatchOutcome {
                            code,
                            duration,
                            error_kind: None,
                        },
                        Some(updated_context),
                    );
                }
                Err(_) => ModuleDispatchOutcome {
                    code: MODULE_FAILURE_CODE,
                    duration: started_at.elapsed(),
                    error_kind: Some(DispatchErrorKind::Panicked),
                },
            },
            () = sleep(timeout_limit) => {
                handle.abort();
                ModuleDispatchOutcome {
                    code: MODULE_FAILURE_CODE,
                    duration: timeout_limit,
                    error_kind: Some(DispatchErrorKind::Timeout {
                        timeout: timeout_limit,
                    }),
                }
            }
        };

        return (outcome, None);
    }

    match handle.await {
        Ok((code, duration, updated_context)) => (
            ModuleDispatchOutcome {
                code,
                duration,
                error_kind: None,
            },
            Some(updated_context),
        ),
        Err(_) => (
            ModuleDispatchOutcome {
                code: MODULE_FAILURE_CODE,
                duration: started_at.elapsed(),
                error_kind: Some(DispatchErrorKind::Panicked),
            },
            None,
        ),
    }
}

fn emit_inline(
    module: &Module,
    event: Event,
    mut context: Context,
    module_context: &ModuleContext,
    timeout_limit: Option<Duration>,
) -> (ModuleDispatchOutcome, Context) {
    let start = Instant::now();
    let code = emit_in_context(module, event, &mut context, module_context);
    let duration = start.elapsed();
    let error_kind = timeout_limit.and_then(|limit| {
        timeout_overrun(limit, duration).map(|limit| DispatchErrorKind::Timeout { timeout: limit })
    });
    let code = error_kind.as_ref().map_or(code, |_| MODULE_FAILURE_CODE);

    (
        ModuleDispatchOutcome {
            code,
            duration,
            error_kind,
        },
        context,
    )
}

fn emit_inline_in_place(
    module: &Module,
    event: Event,
    context: &mut Context,
    module_context: &ModuleContext,
    timeout_limit: Option<Duration>,
) -> ModuleDispatchOutcome {
    let start = Instant::now();
    let code = emit_in_context(module, event, context, module_context);
    let duration = start.elapsed();
    let error_kind = timeout_limit.and_then(|limit| {
        timeout_overrun(limit, duration).map(|limit| DispatchErrorKind::Timeout { timeout: limit })
    });
    let code = error_kind.as_ref().map_or(code, |_| MODULE_FAILURE_CODE);

    ModuleDispatchOutcome {
        code,
        duration,
        error_kind,
    }
}

#[cfg(feature = "metrics")]
fn record_dispatch_error_metrics(
    module_context: &ModuleContext,
    event: Event,
    error_kind: &DispatchErrorKind,
) {
    let Some(metrics) = metrics() else {
        return;
    };

    let is_validation = matches!(event, Event::Validate(_));
    let error_kind_label = match error_kind {
        DispatchErrorKind::Timeout { .. } => {
            if is_validation {
                "validation_timeout"
            } else {
                "timeout"
            }
        }
        DispatchErrorKind::Panicked => {
            if is_validation {
                "validation_panic"
            } else {
                "panic"
            }
        }
        DispatchErrorKind::NotInitialized => "not_initialized",
        DispatchErrorKind::ModuleFailed { .. } => {
            if is_validation {
                "validation_error"
            } else {
                "error"
            }
        }
    };

    metrics.modules.record_error_with_kind(
        &module_context.module_type,
        &module_context.module_name,
        error_kind_label,
    );

    if matches!(error_kind, DispatchErrorKind::Timeout { .. }) {
        metrics.modules.record_timeout(&module_context.module_name);
    }
}

#[cfg(not(feature = "metrics"))]
fn record_dispatch_error_metrics(
    _module_context: &ModuleContext,
    _event: Event,
    _error_kind: &DispatchErrorKind,
) {
}

fn log_dispatch_failure(event: Event, module: &Module, outcome: &ModuleDispatchOutcome) {
    let module_name = format!("{module}");
    let callback_name = event.callback_name();
    let is_validation = matches!(event, Event::Validate(_));

    if let Some(error_kind) = outcome.error_kind.as_ref() {
        match error_kind {
            DispatchErrorKind::Timeout { timeout } => {
                if is_validation {
                    internal!(
                        level = ERROR,
                        "Module {module_name} timed out in {callback_name} after {timeout:?}"
                    );
                } else {
                    internal!(
                        level = WARN,
                        "Module {module_name} timed out in {callback_name} after {timeout:?}"
                    );
                }
            }
            DispatchErrorKind::Panicked => {
                if is_validation {
                    internal!(
                        level = ERROR,
                        "Module {module_name} panicked in {callback_name}"
                    );
                } else {
                    internal!(
                        level = WARN,
                        "Module {module_name} panicked in {callback_name}"
                    );
                }
            }
            DispatchErrorKind::NotInitialized => {
                if is_validation {
                    internal!(
                        level = ERROR,
                        "Module {module_name} not initialized for {callback_name}"
                    );
                } else {
                    internal!(
                        level = WARN,
                        "Module {module_name} not initialized for {callback_name}"
                    );
                }
            }
            DispatchErrorKind::ModuleFailed { code } => {
                if is_validation {
                    internal!(
                        level = ERROR,
                        "Module {module_name} returned non-zero code {code} in {callback_name}"
                    );
                } else {
                    internal!(
                        level = WARN,
                        "Module {module_name} returned non-zero code {code} in {callback_name}"
                    );
                }
            }
        }
        return;
    }

    if outcome.code != 0 {
        if is_validation {
            internal!(
                level = ERROR,
                "Module {module_name} returned non-zero code {} in {callback_name}",
                outcome.code
            );
        } else {
            internal!(
                level = WARN,
                "Module {module_name} returned non-zero code {} in {callback_name}",
                outcome.code
            );
        }
    }
}

type Init = unsafe extern "C" fn(StringVector) -> i32;
type DeclareModule = unsafe extern "C" fn() -> Mod;

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Deserialize)]
pub enum Ev {
    ConnectionOpened,
    ConnectionClosed,
    /// Triggered before attempting delivery to a mail server
    DeliveryAttempt,
    /// Triggered when delivery succeeds
    DeliverySuccess,
    /// Triggered when delivery fails (temporary or permanent)
    DeliveryFailure,
    /// Triggered when an SMTP command returns an error response (4xx/5xx)
    /// Error status available in `context.response`
    SmtpError,
    /// Triggered when a complete message is received from a client
    /// Message size available in `context.data`
    SmtpMessageReceived,
    /// Triggered after a DNS lookup completes
    /// Cache hit/miss status available in `context.metadata["dns_cache_status"]`
    DnsLookup,
}

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Deserialize)]
pub enum Event {
    Validate(validate::Event),
    Event(Ev),
}

impl Event {
    #[must_use]
    pub const fn callback_name(&self) -> &'static str {
        match self {
            Self::Validate(validate::Event::Connect) => "validate_connect",
            Self::Validate(validate::Event::Ehlo) => "validate_ehlo",
            Self::Validate(validate::Event::MailFrom) => "validate_mail_from",
            Self::Validate(validate::Event::RcptTo) => "validate_rcpt_to",
            Self::Validate(validate::Event::Data) => "validate_data",
            Self::Validate(validate::Event::PreSpool) => "validate_pre_spool",
            Self::Validate(validate::Event::StartTls) => "validate_starttls",
            Self::Validate(validate::Event::Auth) => "validate_auth",
            Self::Event(Ev::ConnectionOpened) => "on_connection_opened",
            Self::Event(Ev::ConnectionClosed) => "on_connection_closed",
            Self::Event(Ev::DeliveryAttempt) => "on_delivery_attempt",
            Self::Event(Ev::DeliverySuccess) => "on_delivery_success",
            Self::Event(Ev::DeliveryFailure) => "on_delivery_failure",
            Self::Event(Ev::SmtpError) => "on_smtp_error",
            Self::Event(Ev::SmtpMessageReceived) => "on_smtp_message_received",
            Self::Event(Ev::DnsLookup) => "on_dns_lookup",
        }
    }
}

#[derive(Debug)]
#[repr(C)]
pub enum Mod {
    ValidationListener(validate::ModuleValidation),
    EventListener {
        module_name: *const libc::c_char,
        init: Init,
        emit: unsafe extern "C" fn(Ev, &mut Context) -> i32,
    },
}

unsafe impl Send for Mod {}
unsafe impl Sync for Mod {}

impl Mod {
    pub fn emit(&self, event: Event, context: &mut Context) -> i32 {
        match self {
            Self::ValidationListener(validator) => validator.emit(event, context),
            Self::EventListener { emit, .. } => {
                if let Event::Event(ev) = event {
                    unsafe {
                        emit(ev, context);
                    }
                }
                0
            }
        }
    }

    #[must_use]
    pub fn init(&self, arguments: &[Arc<str>]) -> i32 {
        unsafe {
            match self {
                Self::ValidationListener(validator) => (validator.init)(arguments.into()),
                Self::EventListener { init, .. } => init(arguments.into()),
            }
        }
    }
}

/// This solely exists in order to have the `ModuleValidation` be parsed
/// by cbindgen. Perhaps in future it will be done in a better way.
#[unsafe(no_mangle)]
pub const extern "C" fn __cbindgen_hack_please_remove() -> *mut Mod {
    std::ptr::null_mut()
}

#[derive(Error, Debug)]
pub enum Error {
    #[error("Module load error: {0}")]
    Load(#[from] libloading::Error),

    #[error("Init error: {0}")]
    Init(String),

    #[error("Validation Error: {0}")]
    Validation(String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct Test {
    pub events_called: Vec<Ev>,
    pub validators_called: Vec<validate::Event>,
    pub sleep_millis: Option<u64>,
    pub return_code: i32,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum Module {
    SharedLibrary(library::Shared),
    TestModule(RwLock<Test>),
    /// Core validation module with session-specific configuration.
    /// Not deserialized - created programmatically by each session.
    #[serde(skip)]
    Core {
        validators: Arc<core::CoreValidators>,
    },
    /// OpenTelemetry metrics module for observability.
    /// Not deserialized - created programmatically when metrics are enabled.
    #[cfg(feature = "metrics")]
    #[serde(skip)]
    Metrics,
    /// Python module loaded from a .py file
    #[cfg(feature = "python")]
    Python(python::Python),
    /// WebAssembly module loaded from a .wasm file
    #[cfg(feature = "wasm")]
    Wasm(wasm::Wasm),
}

/// Error category reported by module dispatch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DispatchErrorKind {
    /// Module store was not initialized.
    NotInitialized,
    /// Module returned a non-zero code.
    ModuleFailed { code: i32 },
    /// Module timed out.
    Timeout { timeout: Duration },
    /// Module panicked.
    Panicked,
}

/// Result of dispatching an event to the module pipeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DispatchResult {
    /// All modules accepted the event or no validation was required.
    Accept,
    /// A validation module rejected the operation.
    Reject {
        status: Status,
        message: Cow<'static, str>,
    },
    /// Dispatch encountered an error that should be logged by the caller.
    Error {
        module: String,
        kind: DispatchErrorKind,
    },
}

impl Display for Module {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SharedLibrary(lib) => f.write_fmt(format_args!("{lib}")),
            Self::TestModule { .. } => f.write_str("Test Module"),
            Self::Core { .. } => f.write_str("Core Module"),
            #[cfg(feature = "metrics")]
            Self::Metrics => f.write_str("Metrics Module"),
            #[cfg(feature = "python")]
            Self::Python(py) => f.write_fmt(format_args!("Python Module: {}", py.path().display())),
            #[cfg(feature = "wasm")]
            Self::Wasm(wasm) => f.write_fmt(format_args!("WASM Module: {}", wasm.path().display())),
        }
    }
}

impl Module {
    fn module_context(&self) -> ModuleContext {
        match self {
            Self::SharedLibrary(lib) => ModuleContext::new("shared_library", lib.name.clone()),
            Self::TestModule { .. } => ModuleContext::new("test", "test"),
            Self::Core { .. } => ModuleContext::new("core", "core"),
            #[cfg(feature = "metrics")]
            Self::Metrics => ModuleContext::new("metrics", "metrics"),
            #[cfg(feature = "python")]
            Self::Python(py) => ModuleContext::new("python", py.path().display().to_string()),
            #[cfg(feature = "wasm")]
            Self::Wasm(wasm) => ModuleContext::new("wasm", wasm.path().display().to_string()),
        }
    }

    #[traced(instrument(level = tracing::Level::TRACE, ret, skip(self, validate_context)), timing(precision = "us"))]
    fn emit(&self, event: Event, validate_context: &mut Context) -> i32 {
        match self {
            Self::SharedLibrary(lib) => lib.emit(event, validate_context),
            Self::TestModule { .. } => test::emit(self, event, validate_context),
            Self::Core { validators } => core::emit(validators, event, validate_context),
            #[cfg(feature = "metrics")]
            Self::Metrics => {
                metrics::emit(event, validate_context);
                0
            }
            #[cfg(feature = "python")]
            Self::Python(py) => py.emit(event, validate_context),
            #[cfg(feature = "wasm")]
            Self::Wasm(wasm) => wasm.emit(event, validate_context),
        }
    }

    fn init_with(&mut self, context: &ModuleInitContext) -> Result<(), Error> {
        match self {
            Self::SharedLibrary(lib) => lib.init(),
            Self::TestModule { .. } | Self::Core { .. } => Ok(()),
            #[cfg(feature = "metrics")]
            Self::Metrics => Ok(()),
            #[cfg(feature = "python")]
            Self::Python(py) => py.init(),
            #[cfg(feature = "wasm")]
            Self::Wasm(wasm) => wasm.init(&context.outbound_bindings),
        }
    }
}

fn rejection_from_code(code: i32, context: &mut Context) -> DispatchResult {
    let status_code = u32::try_from(code).unwrap_or(451);
    let status = Status::from(status_code);
    let message = context
        .response
        .as_ref()
        .and_then(|(existing_status, message)| {
            if existing_status.is_temporary() || existing_status.is_permanent() {
                Some(message.clone())
            } else {
                None
            }
        })
        .unwrap_or(Cow::Borrowed("Rejected"));

    if context.response.is_none()
        || context
            .response
            .as_ref()
            .is_some_and(|(existing_status, _)| {
                !existing_status.is_temporary() && !existing_status.is_permanent()
            })
    {
        context.response = Some((status, message.clone()));
    }

    DispatchResult::Reject { status, message }
}

struct ModuleInitContext {
    outbound_bindings: Vec<String>,
}

pub mod test {
    use std::time::Duration;

    use empath_common::context::Context;

    use super::{Event, Module};

    pub(super) fn emit(module: &Module, event: Event, _validate_context: &mut Context) -> i32 {
        if let Module::TestModule(mute) = module {
            // Use poisoned data if lock is poisoned (test module only exists in debug builds)
            let (sleep_millis, return_code) = {
                let mut inner = mute
                    .write()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                match event {
                    Event::Validate(ev) => inner.validators_called.push(ev),
                    Event::Event(ev) => inner.events_called.push(ev),
                }
                (inner.sleep_millis, inner.return_code)
            };

            if let Some(sleep_millis) = sleep_millis {
                std::thread::sleep(Duration::from_millis(sleep_millis));
            }

            return_code
        } else {
            0
        }
    }
}
