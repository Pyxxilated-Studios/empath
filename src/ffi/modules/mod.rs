use core::fmt::{self, Display};
#[cfg(test)]
use std::sync::Mutex;
use std::sync::{Arc, LazyLock, RwLock};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{internal, smtp::context::Context};

use super::string::StringVector;

pub mod library;
pub mod validate;

type Init = unsafe extern "C" fn(StringVector) -> i32;
type DeclareModule = unsafe extern "C" fn() -> Mod;

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Ev {
    ConnectionOpened,
    ConnectionClosed,
}

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Event {
    Validate(validate::Event),
    Event(Ev),
}

#[repr(C)]
#[expect(dead_code)]
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
#[no_mangle]
pub const extern "C" fn __cbindgen_hack_please_remove() -> *mut Mod {
    std::ptr::null_mut()
}

#[derive(Error, Debug)]
#[expect(dead_code)]
pub enum Error {
    #[error("Module load error: {0}")]
    Load(#[from] libloading::Error),

    #[error("Init error: {0}")]
    Init(String),

    #[error("Validation Error: {0}")]
    Validation(String),
}

#[cfg(test)]
#[derive(Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Test {
    pub(crate) validate_connect_called: bool,
    pub(crate) validate_mail_from_called: bool,
    pub(crate) validate_data_called: bool,
    pub(crate) event_called: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Module {
    SharedLibrary(library::Shared),
    #[cfg(test)]
    TestModule(Arc<Mutex<Test>>),
}

pub static MODULE_STORE: LazyLock<RwLock<Vec<Module>>> = LazyLock::new(RwLock::default);

impl Display for Module {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SharedLibrary(lib) => f.write_fmt(format_args!("{lib}")),
            #[cfg(test)]
            Self::TestModule { .. } => f.write_str("Test Module"),
        }
    }
}

impl Module {
    fn emit(&self, event: Event, validate_context: &mut Context) -> i32 {
        match self {
            Self::SharedLibrary(ref lib) => lib.emit(event, validate_context),
            #[cfg(test)]
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
pub fn init(modules: Vec<Module>) -> anyhow::Result<()> {
    internal!(level = INFO, "Initialising modules ...");

    for mut module in modules {
        internal!("Init: {module}");

        match module {
            Module::SharedLibrary(ref mut lib) => lib.init()?,
            #[cfg(test)]
            Module::TestModule { .. } => {}
        }

        MODULE_STORE
            .write()
            .expect("Unable to write module")
            .push(module);
    }

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

#[cfg(test)]
pub(crate) mod test {
    use std::sync::Arc;

    use crate::smtp::context::Context;

    use super::{validate, Event, Module};

    pub(crate) fn test_module() -> Module {
        Module::TestModule(Arc::default())
    }

    pub(super) fn emit(module: &Module, event: Event, _validate_context: &mut Context) -> i32 {
        if let Module::TestModule(ref mute) = module {
            let mut inner = mute.lock().unwrap();
            match event {
                Event::Validate(validate::Event::Connect) => inner.validate_connect_called = true,
                Event::Validate(validate::Event::MailFrom) => {
                    inner.validate_mail_from_called = true
                }
                Event::Validate(validate::Event::Data) => inner.validate_data_called = true,
                Event::Validate(_) => todo!(),
                Event::Event(_) => inner.event_called = true,
            }
        }
        0
    }
}
