use std::sync::Arc;
use std::{
    fmt::Display,
    sync::{LazyLock, RwLock},
};

use libloading::Library;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{internal, smtp::context::Context};

use super::string::StringVector;

type Init = unsafe extern "C" fn(StringVector) -> i32;
type DeclareModule = unsafe extern "C" fn() -> Mod;

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum ValidateEvent {
    Connect,
    MailFrom,
    Data,
}

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Ev {
    ConnectionOpened,
    ConnectionClosed,
}

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Event {
    Validate(ValidateEvent),
    Event(Ev),
}

#[repr(C)]
#[allow(dead_code)]
pub enum Mod {
    ValidationListener(Validation),
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

#[repr(C)]
#[allow(clippy::struct_field_names)]
pub struct Validators {
    pub validate_connect: Option<unsafe extern "C" fn(&mut Context) -> i32>,
    pub validate_mail_from: Option<unsafe extern "C" fn(&mut Context) -> i32>,
    pub validate_data: Option<unsafe extern "C" fn(&mut Context) -> i32>,
}

#[repr(C)]
#[allow(clippy::module_name_repetitions)]
pub struct Validation {
    pub module_name: *const libc::c_char,
    pub init: Init,
    pub validators: Validators,
}

unsafe impl Send for Validation {}

unsafe impl Sync for Validation {}

impl Validation {
    ///
    /// Emit an event to this library's validation module
    ///
    pub fn emit(&self, event: Event, context: &mut Context) -> i32 {
        match event {
            Event::Validate(ValidateEvent::Data) => unsafe {
                self.validators.validate_data.map(|func| func(context))
            },
            Event::Validate(ValidateEvent::MailFrom) => unsafe {
                self.validators.validate_mail_from.map(|func| func(context))
            },
            Event::Validate(ValidateEvent::Connect) => unsafe {
                self.validators.validate_connect.map(|func| func(context))
            },
            _ => None,
        }
        .inspect(|v| internal!("{event:?} = {v}"))
        .unwrap_or_default()
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
#[allow(dead_code)]
pub enum Error {
    #[error("Module load error: {0}")]
    Load(#[from] libloading::Error),

    #[error("Init error: {0}")]
    Init(String),

    #[error("Validation Error: {0}")]
    Validation(String),
}

#[allow(
    clippy::unsafe_derive_deserialize,
    reason = "The unsafe aspects have nothing to do with the struct"
)]
#[derive(Serialize, Deserialize)]
pub struct SharedLibrary {
    pub name: String,
    pub arguments: Arc<[Arc<str>]>,
    #[serde(skip)]
    module: Option<Mod>,
    #[serde(skip)]
    library: Option<Library>,
}

unsafe impl Send for SharedLibrary {}

unsafe impl Sync for SharedLibrary {}

impl Display for SharedLibrary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_fmt(format_args!("{}: {:?}", self.name, self.arguments))
    }
}

impl SharedLibrary {
    fn init(&mut self) -> anyhow::Result<()> {
        unsafe {
            let lib = Library::new(&self.name)?;

            let module = lib.get::<DeclareModule>(b"declare_module")?();
            let response = module.init(&self.arguments);
            internal!("init: {response:#?}");
            self.module = Some(module);
            self.library = Some(lib);

            Ok(())
        }
    }

    fn emit(&self, event: Event, validate_context: &mut Context) -> i32 {
        self.module
            .as_ref()
            .map(|module| module.emit(event, validate_context))
            .unwrap_or_default()
    }
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Module {
    SharedLibrary(SharedLibrary),
}

static MODULE_STORE: LazyLock<RwLock<Vec<Module>>> = LazyLock::new(RwLock::default);

impl Display for Module {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SharedLibrary(lib) => f.write_fmt(format_args!("{lib}")),
        }
    }
}

impl Module {
    fn emit(&self, event: Event, validate_context: &mut Context) -> i32 {
        match self {
            Self::SharedLibrary(ref lib) => lib.emit(event, validate_context),
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
