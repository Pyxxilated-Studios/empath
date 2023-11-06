use std::{
    fs::File,
    io::{BufReader, Read},
    path::Path,
};

use futures_util::future::join_all;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use empath_common::{
    ffi::module::{Error, Module},
    internal,
    listener::Listener,
    logging,
};

pub mod smtp;

#[derive(Error, Debug)]
pub enum ServerError {
    #[error(transparent)]
    ModuleError(#[from] Error),

    #[error(transparent)]
    IO(#[from] std::io::Error),
}

#[derive(Serialize, Deserialize, Default)]
pub struct Server {
    #[serde(rename = "listener")]
    listeners: Vec<Box<dyn Listener>>,
    #[serde(alias = "module")]
    modules: Vec<Module>,
}

unsafe impl Send for Server {}

impl Server {
    ///
    /// # Errors
    ///
    /// If the configuration file doesn't exist, or is not readable,
    /// or if the configuration file is invalid.
    ///
    pub fn from_config(file: &str) -> std::io::Result<Self> {
        let file = Path::new(file);
        let mut reader = BufReader::new(File::open(file)?);
        let mut config = String::new();
        reader.read_to_string(&mut config)?;

        toml::from_str(&config)
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err.to_string()))
    }

    /// Run the server, which will accept connections on the
    /// port it is asked to (or the default if not chosen).
    ///
    /// # Examples
    ///
    /// ```
    /// use empath_server::Server;
    ///
    /// let server = Server::default();
    /// server.run();
    /// ```
    ///
    /// # Errors
    ///
    /// This function will return an error if there is an issue accepting a connection,
    /// or if there is an issue binding to the specific address and port combination.
    ///
    /// # Panics
    /// This will panic if it is unable to convert itself to its original configuration,
    /// which should not be possible
    ///
    pub async fn run(self) -> Result<(), ServerError> {
        logging::init();

        internal!(
            level = TRACE,
            "{}",
            toml::to_string(&self).expect("Invalid Server Configuration")
        );

        empath_common::ffi::module::init(self.modules)?;

        let iter = self.listeners.iter();
        join_all(iter.map(|listener| listener.spawn())).await;

        Ok(())
    }

    #[must_use]
    pub fn with_listener(mut self, listener: Box<dyn Listener>) -> Self {
        self.listeners.push(listener);

        self
    }

    #[must_use]
    pub fn with_module(mut self, module: Module) -> Self {
        self.modules.push(module);

        self
    }
}
