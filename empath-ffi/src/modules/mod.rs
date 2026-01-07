use std::borrow::Cow;
use std::sync::{Arc, OnceLock, RwLock};
use std::time::{Duration, Instant};

use empath_common::context::{Context, ModuleContext};
use empath_common::internal;
use empath_common::status::Status;
use empath_metrics::is_enabled;
#[cfg(feature = "metrics")]
use empath_metrics::metrics;
use empath_tracing::traced;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::task;
use tokio::time::sleep;

use super::string::StringVector;

/// Handle module execution errors with consistent logging and metrics.
///
/// This function consolidates error handling logic shared between Python and WASM
/// modules, including:
/// - Recording timeout and error metrics
/// - Logging errors at appropriate severity levels
/// - Honoring prior rejections from the module
/// - Returning 451 (temporary failure) for transparency
///
/// # Arguments
///
/// * `module_type` - Module type identifier (e.g., "python", "wasm")
/// * `module_path` - Path to the module file for logging/metrics
/// * `is_validation` - Whether this is a validation callback (affects log level)
/// * `error` - The module error that occurred
/// * `callback_name` - Name of the callback that failed
/// * `context` - Context to check for prior rejection
///
/// # Returns
///
/// Returns the SMTP response code:
/// - Prior rejection code if module set one before failing
/// - 451 (temporary failure) otherwise
pub fn handle_module_error(
    module_type: &str,
    module_path: &str,
    is_validation: bool,
    error: &error::ModuleError,
    callback_name: &str,
    context: &Context,
) -> i32 {
    let is_timeout = error.is_timeout();

    // Record metrics
    #[cfg(feature = "metrics")]
    if let Some(metrics) = metrics() {
        if is_timeout {
            metrics.modules.record_timeout(module_path);
        }

        let error_kind = match (is_validation, is_timeout) {
            (true, true) => "validation_timeout",
            (true, false) => "validation_error",
            (false, true) => "timeout",
            (false, false) => "error",
        };

        metrics
            .modules
            .record_error_with_kind(module_type, module_path, error_kind);
    }

    // Log the error at appropriate level
    let action = if is_timeout { "timed out" } else { "error" };
    if is_validation {
        internal!(
            level = ERROR,
            "{} module {} {action} in {callback_name}: {error}",
            module_type.to_uppercase(),
            module_path
        );
    } else {
        internal!(
            level = WARN,
            "{} module {} {action} in {callback_name}: {error}",
            module_type.to_uppercase(),
            module_path
        );
    }

    // Check for prior rejection - honor it if the module set one before failing
    if let Some((status, message)) = &context.response {
        let code = u32::from(*status);
        if code >= 400 {
            internal!(
                level = WARN,
                "{} module {} crashed after rejecting with {}: {} - honoring rejection",
                module_type.to_uppercase(),
                module_path,
                code,
                message
            );
            return code.try_into().unwrap_or(550);
        }
    }

    // No prior rejection - log final failure and return temporary failure
    if is_validation {
        internal!(
            level = ERROR,
            "{} validation module {} failed - returning 451",
            module_type.to_uppercase(),
            module_path
        );
    } else {
        internal!(
            level = WARN,
            "{} event module {} failed - returning 451",
            module_type.to_uppercase(),
            module_path
        );
    }

    451 // Temporary failure - be transparent about module errors
}

pub mod core;
pub mod library;
#[cfg(feature = "metrics")]
pub mod metrics;
#[cfg(feature = "python")]
pub mod python;
pub mod traits;
pub mod validate;

pub(crate) mod error;

#[cfg(feature = "wasm")]
pub mod wasm;

pub struct ModuleRegistry {
    modules: Arc<[Arc<dyn traits::Module>]>,
    outbound_bindings: Arc<RwLock<Vec<String>>>,
    dispatch_config: ModuleDispatchConfig,
}

impl std::fmt::Debug for ModuleRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModuleRegistry")
            .field("modules", &format!("[{} modules]", self.modules.len()))
            .field("outbound_bindings", &self.outbound_bindings)
            .field("dispatch_config", &self.dispatch_config)
            .finish()
    }
}

pub struct ModuleRegistryBuilder {
    modules: Vec<Box<dyn traits::Module>>,
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

    /// Add modules to the registry.
    ///
    /// Modules are dispatched in the order they are added, after the built-in
    /// core and metrics modules.
    #[must_use]
    pub fn with_modules(mut self, modules: Vec<Box<dyn traits::Module>>) -> Self {
        self.modules = modules;
        self
    }

    /// Add a single module to the registry.
    #[must_use]
    pub fn add_module(mut self, module: Box<dyn traits::Module>) -> Self {
        self.modules.push(module);
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

        let init_context = traits::ModuleInitContext {
            outbound_bindings: self.outbound_bindings.clone(),
        };

        // Add core module first so it runs before other validation modules
        let mut all_modules: Vec<Box<dyn traits::Module>> = vec![
            Box::new(core::Core::new()),
            #[cfg(debug_assertions)]
            Box::new(test::TestModule::default()),
        ];

        // Add metrics module if enabled
        #[cfg(feature = "metrics")]
        if is_enabled() {
            all_modules.push(Box::new(metrics::Metrics));
        }

        all_modules.append(&mut self.modules);

        // Initialize all modules
        for module in &mut all_modules {
            internal!("Init: {}", module.display_name());
            module.init(&init_context)?;
        }

        internal!(level = INFO, "Modules initialised");

        let modules: Vec<Arc<dyn traits::Module>> =
            all_modules.into_iter().map(Arc::from).collect();

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
    pub fn modules(&self) -> &[Arc<dyn traits::Module>] {
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

        for module in self.modules.iter() {
            let module_context = module.module_context();
            internal!(level = DEBUG, "{}", module.display_name());

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
                    let module_name = module.display_name();
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
        module: Arc<dyn traits::Module>,
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
                )
                .await;
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
        .await
    }
}

const MODULE_FAILURE_CODE: i32 = 451;

struct ModuleDispatchOutcome {
    code: i32,
    duration: Duration,
    error_kind: Option<DispatchErrorKind>,
}

async fn emit_in_context(
    module: &dyn traits::Module,
    event: Event,
    context: &mut Context,
    module_context: &ModuleContext,
) -> i32 {
    let previous = context.module_context.take();
    context.module_context = Some(module_context.clone());
    let code = module.emit(event, context).await;
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
    module: Arc<dyn traits::Module>,
    event: Event,
    mut context: Context,
    module_context: ModuleContext,
    timeout_limit: Option<Duration>,
) -> (ModuleDispatchOutcome, Option<Context>) {
    let started_at = Instant::now();

    // Use spawn_blocking to isolate potentially blocking modules (Python, WASM, C).
    // The async emit is wrapped with block_on so blocking operations don't starve
    // the main executor, allowing timeouts to fire correctly.
    let handle = tokio::runtime::Handle::current();
    let mut join_handle = task::spawn_blocking(move || {
        handle.block_on(async move {
            let start = Instant::now();
            let code = emit_in_context(module.as_ref(), event, &mut context, &module_context).await;
            (code, start.elapsed(), context)
        })
    });

    if let Some(timeout_limit) = timeout_limit {
        let outcome = tokio::select! {
            result = &mut join_handle => match result {
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
                Err(e) if e.is_panic() => ModuleDispatchOutcome {
                    code: MODULE_FAILURE_CODE,
                    duration: started_at.elapsed(),
                    error_kind: Some(DispatchErrorKind::Panicked),
                },
                Err(_) => ModuleDispatchOutcome {
                    code: MODULE_FAILURE_CODE,
                    duration: started_at.elapsed(),
                    error_kind: Some(DispatchErrorKind::ModuleFailed { code: MODULE_FAILURE_CODE }),
                },
            },
            () = sleep(timeout_limit) => {
                join_handle.abort();
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

    match join_handle.await {
        Ok((code, duration, updated_context)) => (
            ModuleDispatchOutcome {
                code,
                duration,
                error_kind: None,
            },
            Some(updated_context),
        ),
        Err(e) if e.is_panic() => (
            ModuleDispatchOutcome {
                code: MODULE_FAILURE_CODE,
                duration: started_at.elapsed(),
                error_kind: Some(DispatchErrorKind::Panicked),
            },
            None,
        ),
        Err(_) => (
            ModuleDispatchOutcome {
                code: MODULE_FAILURE_CODE,
                duration: started_at.elapsed(),
                error_kind: Some(DispatchErrorKind::ModuleFailed {
                    code: MODULE_FAILURE_CODE,
                }),
            },
            None,
        ),
    }
}

async fn emit_inline(
    module: &dyn traits::Module,
    event: Event,
    mut context: Context,
    module_context: &ModuleContext,
    timeout_limit: Option<Duration>,
) -> (ModuleDispatchOutcome, Context) {
    let start = Instant::now();
    let code = emit_in_context(module, event, &mut context, module_context).await;
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

async fn emit_inline_in_place(
    module: &dyn traits::Module,
    event: Event,
    context: &mut Context,
    module_context: &ModuleContext,
    timeout_limit: Option<Duration>,
) -> ModuleDispatchOutcome {
    let start = Instant::now();
    let code = emit_in_context(module, event, context, module_context).await;
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

fn log_dispatch_failure(
    event: Event,
    module: &dyn traits::Module,
    outcome: &ModuleDispatchOutcome,
) {
    let module_name = module.display_name();
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
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Deserialize, Serialize)]
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

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct Test {
    pub events_called: Vec<Ev>,
    pub validators_called: Vec<validate::Event>,
    pub sleep_millis: Option<u64>,
    pub return_code: i32,
}

/// Configuration for module instantiation from config files.
///
/// This enum represents the different module types that can be deserialized
/// from configuration. It's converted to `Box<dyn Module>` during registry build.
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum ModuleConfig {
    /// Shared library module (C FFI)
    SharedLibrary(library::Shared),
    /// Python module loaded from a .py file
    #[cfg(feature = "python")]
    Python(python::Python),
    /// WebAssembly module loaded from a .wasm file
    #[cfg(feature = "wasm")]
    Wasm(wasm::Wasm),
}

impl ModuleConfig {
    /// Convert the config into a boxed trait object for the registry.
    #[must_use]
    pub fn into_module(self) -> Box<dyn traits::Module> {
        match self {
            Self::SharedLibrary(shared) => Box::new(shared),
            #[cfg(feature = "python")]
            Self::Python(python) => Box::new(python),
            #[cfg(feature = "wasm")]
            Self::Wasm(wasm) => Box::new(wasm),
        }
    }
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

pub mod test {
    use std::any::Any;
    use std::sync::RwLock;
    use std::time::Duration;

    use empath_common::context::{Context, ModuleContext};
    use serde::{Deserialize, Serialize};

    use super::traits::{ModuleInitContext, ModuleInterface};
    use super::{Error, Event, Test};
    use crate::modules::traits::{BoxFuture, Module};

    fn emit_test(mute: &RwLock<Test>, event: Event) -> i32 {
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
    }

    /// Test module wrapper for trait implementation.
    #[derive(Debug, Default, Serialize, Deserialize)]
    pub struct TestModule(pub RwLock<Test>);

    impl ModuleInterface for TestModule {
        fn init(&mut self, _context: &ModuleInitContext) -> Result<(), Error> {
            Ok(())
        }

        async fn emit(&self, event: Event, _context: &mut Context) -> i32 {
            emit_test(&self.0, event)
        }

        fn module_context(&self) -> ModuleContext {
            ModuleContext::new("test", "test")
        }

        fn display_name(&self) -> String {
            "Test Module".to_string()
        }
    }

    #[typetag::serde]
    impl Module for TestModule {
        fn init(&mut self, context: &ModuleInitContext) -> Result<(), Error> {
            ModuleInterface::init(self, context)
        }

        fn emit<'a>(&'a self, event: Event, context: &'a mut Context) -> BoxFuture<'a, i32> {
            Box::pin(ModuleInterface::emit(self, event, context))
        }

        fn module_context(&self) -> ModuleContext {
            ModuleInterface::module_context(self)
        }

        fn display_name(&self) -> String {
            ModuleInterface::display_name(self)
        }

        fn as_any(&self) -> &dyn Any {
            self
        }
    }
}
