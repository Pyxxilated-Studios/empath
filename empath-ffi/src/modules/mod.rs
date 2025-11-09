use std::{
    fmt::{self, Display},
    sync::{Arc, Mutex, OnceLock},
};

use empath_common::{context::Context, internal};
use empath_tracing::traced;
use serde::Deserialize;
use thiserror::Error;

use super::string::StringVector;

pub mod core;
pub mod library;
pub mod validate;

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
}

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Deserialize)]
pub enum Event {
    Validate(validate::Event),
    Event(Ev),
}

#[repr(C)]
pub enum Mod {
    ValidationListener(validate::Validation),
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

///
/// This solely exists in order to have the `Validation` be parsed
/// by cbindgen. Perhaps in future it will be done in a better way.
///
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

#[derive(Debug, Default, PartialEq, Eq, Deserialize)]
pub struct Test {
    pub events_called: Vec<Ev>,
    pub validators_called: Vec<validate::Event>,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
pub enum Module {
    SharedLibrary(library::Shared),
    TestModule(Arc<Mutex<Test>>),
    /// Core validation module with session-specific configuration.
    /// Not deserialized - created programmatically by each session.
    #[serde(skip)]
    Core {
        validators: Arc<core::CoreValidators>,
    },
}

/// Module store using Arc for lock-free concurrent reads after initialization
pub static MODULE_STORE: OnceLock<Arc<[Module]>> = OnceLock::new();

impl Display for Module {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SharedLibrary(lib) => f.write_fmt(format_args!("{lib}")),
            Self::TestModule { .. } => f.write_str("Test Module"),
            Self::Core { .. } => f.write_str("Core Module"),
        }
    }
}

impl Module {
    #[traced(instrument(level = tracing::Level::TRACE, ret, skip(self, validate_context)), timing(precision = "us"))]
    fn emit(&self, event: Event, validate_context: &mut Context) -> i32 {
        match self {
            Self::SharedLibrary(lib) => lib.emit(event, validate_context),
            Self::TestModule { .. } => test::emit(self, event, validate_context),
            Self::Core { validators } => core::emit(validators, event, validate_context),
        }
    }
}

/// Initialise all modules
///
/// # Errors
/// This can error in two scenarios:
///   1. The module is invalid (e.g. the shared library cannot be found/has issues)
///   2. The modules init has an issue
///
#[traced(instrument(level = tracing::Level::TRACE, ret, skip_all), timing)]
pub fn init(modules: Vec<Module>) -> Result<(), Error> {
    internal!(level = INFO, "Initialising modules ...");

    // Add core module first so it runs before other validation modules
    let mut all_modules = vec![Module::Core {
        validators: Arc::new(core::CoreValidators::new()),
    }];
    all_modules.extend(modules);

    all_modules
        .iter_mut()
        .inspect(|module| internal!("Init: {module}"))
        .try_for_each(|module| match module {
            Module::SharedLibrary(lib) => lib.init(),
            Module::TestModule { .. } | Module::Core { .. } => Ok(()),
        })?;

    let modules: Arc<[Module]> = all_modules.into();

    // Set module store (ignore if already initialized, which can happen in tests)
    let _ = MODULE_STORE.set(modules);

    internal!(level = INFO, "Modules initialised");

    Ok(())
}

/// Dispatch an event to all modules
///
/// Returns `true` if all modules handled the event successfully (returned 0),
/// or `false` if any module failed or modules not initialized.
///
/// # Errors
/// The events being dispatched are handled with a panic handler, which should
/// catch some possible errors. If these are caught, they are returned to the
/// caller to handle.
///
pub fn dispatch(event: Event, validate_context: &mut Context) -> bool {
    let modules = if let Some(modules) = MODULE_STORE.get() {
        Arc::clone(modules)
    } else {
        internal!(
            level = ERROR,
            "Module store not initialized. Treating as dispatch failure."
        );
        return false;
    };

    internal!("Dispatching: {event:?}");

    modules
        .iter()
        .inspect(|m| internal!(level = DEBUG, "{m}"))
        .all(|module| module.emit(event, validate_context) == 0)
}

pub mod test {
    use empath_common::context::Context;

    use super::{Event, Module};

    pub(super) fn emit(module: &Module, event: Event, _validate_context: &mut Context) -> i32 {
        if let Module::TestModule(mute) = module {
            let mut inner = mute.lock().unwrap();
            match event {
                Event::Validate(ev) => inner.validators_called.push(ev),
                Event::Event(ev) => inner.events_called.push(ev),
            }
        }
        0
    }
}
