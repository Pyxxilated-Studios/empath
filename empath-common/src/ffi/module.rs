use std::fmt::Display;

use libloading::{Library, Symbol};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{ffi::InitFunc, log::Logger};

#[derive(Error, Debug)]
pub enum ModuleError {
    #[error("Module load error: {0}")]
    Load(#[from] libloading::Error),

    #[error("Init error: {0}")]
    Init(String),
}

#[derive(Serialize, Deserialize)]
pub struct SharedLibrary {
    pub name: String,
    pub arguments: Vec<String>,
}

impl Display for SharedLibrary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_fmt(format_args!("{}: {:?}", self.name, self.arguments))
    }
}

impl SharedLibrary {
    fn init(&self) -> Result<(), ModuleError> {
        unsafe {
            let lib = Library::new(&self.name)?;

            let init: Symbol<InitFunc> = lib.get(b"init")?;
            match std::panic::catch_unwind(|| init()) {
                Ok(response) => Ok(Logger::internal(&format!("init: {response}"))),
                Err(err) => Err(ModuleError::Init(format!("{:#?}", err))),
            }
        }
    }
}

#[derive(Serialize, Deserialize)]
pub enum Module {
    SharedLibrary(SharedLibrary),
}

impl Display for Module {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Module::SharedLibrary(lib) => f.write_fmt(format_args!("{}", lib)),
        }
    }
}

impl Module {
    pub fn init(modules: &Vec<Module>) -> Result<(), ModuleError> {
        for module in modules {
            Logger::internal("Init: {module}");

            match module {
                Module::SharedLibrary(lib) => lib.init()?,
            }
        }

        Ok(())
    }
}
