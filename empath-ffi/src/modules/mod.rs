use std::borrow::Cow;
use std::fmt::{self, Display};
use std::sync::{Arc, OnceLock, RwLock};

use empath_common::context::{Context, ModuleContext};
use empath_common::internal;
use empath_common::status::Status;
use empath_metrics::is_enabled;
use empath_tracing::traced;
use serde::Deserialize;
use thiserror::Error;

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

static OUTBOUND_BINDINGS: OnceLock<RwLock<Vec<String>>> = OnceLock::new();

fn outbound_bindings_store() -> &'static RwLock<Vec<String>> {
    OUTBOUND_BINDINGS.get_or_init(|| RwLock::new(Vec::new()))
}

/// Replace the configured outbound bindings list for module helpers.
pub fn set_outbound_bindings(bindings: Vec<String>) {
    if let Ok(mut guard) = outbound_bindings_store().write() {
        *guard = bindings;
    }
}

/// Fetch configured outbound bindings for module helpers.
#[must_use]
pub fn outbound_bindings() -> Vec<String> {
    outbound_bindings_store()
        .read()
        .map(|guard| guard.clone())
        .unwrap_or_default()
}

/// Validate that a binding name exists in configuration.
#[must_use]
pub fn outbound_binding_exists(name: &str) -> bool {
    outbound_bindings_store()
        .read()
        .map(|guard| guard.iter().any(|binding| binding == name))
        .unwrap_or(false)
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
}

#[derive(Deserialize)]
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

/// Module store using Arc for lock-free concurrent reads after initialization
pub static MODULE_STORE: OnceLock<Arc<[Module]>> = OnceLock::new();

/// Error category reported by module dispatch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DispatchErrorKind {
    /// Module store was not initialized.
    NotInitialized,
    /// Module returned a non-zero code.
    ModuleFailed { code: i32 },
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
}

/// Initialise all modules
///
/// # Errors
/// This can error in two scenarios:
///   1. The module is invalid (e.g. the shared library cannot be found/has issues)
///   2. The modules init has an issue
#[traced(instrument(level = tracing::Level::TRACE, ret, skip(modules)), timing)]
pub fn init(modules: Vec<Module>) -> Result<(), Error> {
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

    all_modules.extend(modules);

    all_modules
        .iter_mut()
        .inspect(|module| internal!("Init: {module}"))
        .try_for_each(|module| match module {
            Module::SharedLibrary(lib) => lib.init(),
            Module::TestModule { .. } | Module::Core { .. } => Ok(()),
            #[cfg(feature = "metrics")]
            Module::Metrics => Ok(()),
            #[cfg(feature = "python")]
            Module::Python(py) => py.init(),
            #[cfg(feature = "wasm")]
            Module::Wasm(wasm) => wasm.init(),
        })?;

    // Set module store (ignore if already initialized, which can happen in tests)
    internal!(level = INFO, "Modules initialised");
    let _ = MODULE_STORE.set(all_modules.into());

    Ok(())
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

/// Dispatch an event to all modules
///
/// Validation events fail closed (non-zero results reject).
/// Event callbacks fail open (non-zero results are logged and ignored).
///
/// # Errors
/// The events being dispatched are handled with a panic handler, which should
/// catch some possible errors. If these are caught, they are returned to the
/// caller to handle.
pub fn dispatch(event: Event, validate_context: &mut Context) -> DispatchResult {
    let modules = if let Some(modules) = MODULE_STORE.get() {
        Arc::clone(modules)
    } else {
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

    internal!("Dispatching: {event:?}");

    let mut event_error: Option<DispatchResult> = None;
    for module in modules.iter().inspect(|m| internal!(level = DEBUG, "{m}")) {
        let previous = validate_context.module_context.take();
        validate_context.module_context = Some(module.module_context());
        let code = module.emit(event, validate_context);
        validate_context.module_context = previous;
        if code == 0 {
            continue;
        }

        match event {
            Event::Validate(_) => {
                return rejection_from_code(code, validate_context);
            }
            Event::Event(_) => {
                let module_name = format!("{module}");
                internal!(
                    level = WARN,
                    "Event module {module_name} returned non-zero code {code}"
                );
                if event_error.is_none() {
                    event_error = Some(DispatchResult::Error {
                        module: module_name,
                        kind: DispatchErrorKind::ModuleFailed { code },
                    });
                }
            }
        }
    }

    event_error.unwrap_or(DispatchResult::Accept)
}

pub mod test {
    use empath_common::context::Context;

    use super::{Event, Module};

    pub(super) fn emit(module: &Module, event: Event, _validate_context: &mut Context) -> i32 {
        if let Module::TestModule(mute) = module {
            // Use poisoned data if lock is poisoned (test module only exists in debug builds)
            let mut inner = mute
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            match event {
                Event::Validate(ev) => inner.validators_called.push(ev),
                Event::Event(ev) => inner.events_called.push(ev),
            }
        }
        0
    }
}
