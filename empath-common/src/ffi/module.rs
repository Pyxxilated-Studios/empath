use std::{
    fmt::Display,
    sync::{LazyLock, RwLock},
};

use libloading::{Library, Symbol};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    context::Context,
    ffi::{InitFunc, ValidateData},
    internal,
};

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
    library: Option<Library>,
}

impl Display for SharedLibrary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_fmt(format_args!("{}: {:?}", self.name, self.arguments))
    }
}

impl SharedLibrary {
    fn init(&mut self) -> Result<(), Error> {
        unsafe {
            let lib = Library::new(&self.name)?;

            let init: Symbol<InitFunc> = lib.get(b"init")?;
            match std::panic::catch_unwind(|| init()) {
                Ok(response) => {
                    internal!("init: {response}");
                    self.library = Some(lib);
                    Ok(())
                }
                Err(err) => Err(Error::Init(format!("{err:#?}"))),
            }
        }
    }

    fn emit(&self, event: &str, vctx: &Context) -> Result<(), Error> {
        if let Some(ref lib) = self.library {
            unsafe {
                lib.get::<ValidateData>(event.as_bytes())
                    .map(|handler| handler(vctx))?;
            }
        }

        Ok(())
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
    fn emit(&self, event: &str, vctx: &Context) -> Result<(), Error> {
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
pub fn init(modules: Vec<Module>) -> Result<(), Error> {
    internal!(level = INFO, "Initialising modules ...");
    let mut store = MODULE_STORE.write().expect("Unable to write modules");

    for mut module in modules {
        internal!("Init: {module}");

        match module {
            Module::SharedLibrary(ref mut lib) => lib.init()?,
        }

        store.push(module);
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
pub fn dispatch(event: &str, vctx: &Context) -> Result<(), Error> {
    let store = MODULE_STORE.read().expect("Unable to load modules");

    internal!("Dispatching: {}", event);

    store.iter().try_for_each(|module| module.emit(event, vctx))
}
