#![feature(lazy_cell)]

use std::{
    fs::File,
    io::{BufReader, Read},
    path::Path,
    sync::LazyLock,
};

use futures_util::future::join_all;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use empath_common::{
    ffi::module::{Error, Module},
    internal,
    listener::Listener,
    logging,
    tracing::debug,
};
use tokio::sync::broadcast::{self, error::RecvError};

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

#[derive(Debug, Clone, Copy)]
pub enum Signal {
    Shutdown,
    Finalised,
}

pub static SHUTDOWN_BROADCAST: LazyLock<broadcast::Sender<Signal>> = LazyLock::new(|| {
    let (sender, _receiver) = broadcast::channel(64);
    sender
});

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

        join_all(self.listeners.iter().map(|listener| listener.spawn())).await;

        debug!("Finished with Server::run");

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

    /// Shutdown all listeners
    ///
    /// # Errors
    /// If all receivers for the channel have dropped (should be unlikely, and should have bubbled up
    /// before reaching this point)
    ///
    pub async fn shutdown() -> std::io::Result<()> {
        let mut receiver = SHUTDOWN_BROADCAST.subscribe();

        SHUTDOWN_BROADCAST
            .send(Signal::Shutdown)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Interrupted, e.to_string()))?;
        debug!("Shutdown Signal sent. Waiting for responses...");

        loop {
            match receiver.recv().await {
                Ok(s) => debug!("Received {s:?}"),
                Err(RecvError::Closed) => break,
                Err(e) => debug!("Received: {e:?}"),
            }
        }

        Ok(())
    }
}
