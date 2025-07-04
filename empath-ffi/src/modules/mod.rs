use core::fmt::{self, Display};
use std::sync::{Arc, LazyLock, Mutex, RwLock};

use empath_common::{context::Context, internal};
use empath_tracing::traced;
use serde::Deserialize;
use thiserror::Error;

use super::string::StringVector;

pub mod library;
pub mod validate;

type Init = unsafe extern "C" fn(StringVector) -> i32;
type DeclareModule = unsafe extern "C" fn() -> Mod;

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Deserialize)]
pub enum Ev {
    ConnectionOpened,
    ConnectionClosed,
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
}

pub static MODULE_STORE: LazyLock<RwLock<Vec<Module>>> = LazyLock::new(RwLock::default);

impl Display for Module {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SharedLibrary(lib) => f.write_fmt(format_args!("{lib}")),
            Self::TestModule { .. } => f.write_str("Test Module"),
        }
    }
}

impl Module {
    #[traced(instrument(level = tracing::Level::TRACE, ret, skip(self, validate_context)), timing(precision = "us"))]
    fn emit(&self, event: Event, validate_context: &mut Context) -> i32 {
        match self {
            Self::SharedLibrary(lib) => lib.emit(event, validate_context),
            Self::TestModule { .. } => test::emit(self, event, validate_context),
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
/// # Panics
/// This will panic if it is unable to write to the module store
///
#[traced(instrument(level = tracing::Level::TRACE, ret, skip_all), timing)]
pub fn init(mut modules: Vec<Module>) -> anyhow::Result<()> {
    internal!(level = INFO, "Initialising modules ...");

    modules
        .iter_mut()
        .inspect(|module| internal!("Init: {module}"))
        .try_for_each(|module| match module {
            Module::SharedLibrary(lib) => lib.init(),
            Module::TestModule { .. } => Ok(()),
        })?;

    MODULE_STORE
        .write()
        .expect("Unable to write module")
        .extend(modules.into_iter());

    internal!(level = INFO, "Modules initialised");

    Ok(())
}

/// Dispatch an event to all modules
///
/// # Errors
/// The events being dispatched are handled with a panic handler, which should
/// catch some possible errors. If these are caught, they are returned to the
/// caller to handle.
///
/// # Panics
/// This will panic if it fails to read the Module Store
///
pub fn dispatch(event: Event, validate_context: &mut Context) -> bool {
    let store = MODULE_STORE.read().expect("Unable to load modules");

    internal!("Dispatching: {event:?}");

    store
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
