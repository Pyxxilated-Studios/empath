use std::{
    fmt::Display,
    sync::{LazyLock, RwLock},
};

use libloading::Library;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{context::Context, internal};

use super::string::StringVector;

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Event {
    ValidateData,
}

impl Display for Event {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ValidateData => f.write_str("validate_data"),
        }
    }
}

#[repr(C)]
pub struct Validators {
    pub validate_data: Option<unsafe extern "C" fn(&mut Context) -> i32>,
}

#[repr(C)]
#[allow(clippy::module_name_repetitions)]
pub struct ValidationModule {
    pub module_name: *const libc::c_char,
    pub init: unsafe extern "C" fn(StringVector) -> i32,
    pub validators: Validators,
}

unsafe impl Send for ValidationModule {}
unsafe impl Sync for ValidationModule {}

impl ValidationModule {
    ///
    /// Emit an event to this library's validation module
    ///
    pub fn emit(&self, event: Event, context: &mut Context) {
        match event {
            Event::ValidateData => {
                unsafe { self.validators.validate_data.map(|func| func(context)) };
            }
        }
    }
}

///
/// This solely exists in order to have the `ValidationModule` be parsed
/// by cbindgen. Perhaps in future it will be done in a better way.
///
#[no_mangle]
pub const extern "C" fn __cbindgen_hack_please_remove() -> *mut ValidationModule {
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

#[allow(
    clippy::unsafe_derive_deserialize,
    reason = "The unsafe aspects have nothing to do with the struct"
)]
#[derive(Serialize, Deserialize)]
pub struct SharedLibrary {
    pub name: String,
    pub arguments: Vec<String>,
    #[serde(skip)]
    module: Option<ValidationModule>,
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
    fn init(&mut self) -> Result<(), Error> {
        unsafe {
            let lib = Library::new(&self.name)?;

            let module = lib.get::<unsafe extern "C" fn() -> ValidationModule>(b"create_module")?();
            let arguments = self.arguments.clone();
            match std::panic::catch_unwind(|| (module.init)(arguments.into())) {
                Ok(response) => {
                    internal!("init: {response:#?}");
                    self.module = Some(module);
                    self.library = Some(lib);
                    Ok(())
                }
                Err(err) => Err(Error::Init(format!("{err:#?}"))),
            }
        }
    }

    fn emit(&self, event: Event, vctx: &mut Context) {
        if let Some(ref module) = self.module {
            module.emit(event, vctx);
        }
    }
}

#[derive(Serialize, Deserialize)]
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
    fn emit(&self, event: Event, vctx: &mut Context) {
        match self {
            Self::SharedLibrary(ref lib) => lib.emit(event, vctx),
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
pub fn init(modules: Vec<Module>) -> Result<(), Error> {
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
pub fn dispatch(event: Event, vctx: &mut Context) {
    let store = MODULE_STORE.read().expect("Unable to load modules");

    internal!("Dispatching: {}", event);

    store.iter().for_each(|module| module.emit(event, vctx));
}
