use std::sync::LazyLock;

use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::{
    ffi::modules::{self, Module},
    internal, logging,
    server::Server,
    smtp::Smtp,
};

#[allow(
    clippy::unsafe_derive_deserialize,
    reason = "The unsafe aspects have nothing to do with the struct"
)]
#[derive(Default, Deserialize, Serialize)]
pub struct Controller {
    #[serde(alias = "smtp")]
    smtp_server: Server<Smtp>,
    #[serde(alias = "module")]
    modules: Vec<Module>,
}

#[derive(Debug, Clone, Copy)]
pub enum Signal {
    Shutdown,
    Finalised,
}

pub static SHUTDOWN_BROADCAST: LazyLock<broadcast::Sender<Signal>> = LazyLock::new(|| {
    let (sender, _receiver) = broadcast::channel(64);
    sender
});

async fn shutdown() -> anyhow::Result<()> {
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;

    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            internal!("CTRL+C entered -- Enter it again to force shutdown");
        }
        _ = terminate.recv() => {
            internal!("Terminate Signal received, shutting down");
        }
    };

    let mut receiver = SHUTDOWN_BROADCAST.subscribe();

    SHUTDOWN_BROADCAST
        .send(Signal::Shutdown)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Interrupted, e.to_string()))?;

    loop {
        tokio::select! {
            sig = receiver.recv() => {
                match sig {
                    Ok(s) => tracing::debug!("Received {s:?}"),
                    Err(broadcast::error::RecvError::Closed) => break,
                    Err(e) => tracing::debug!("Received: {e:?}"),
                }
            }

            _ = tokio::signal::ctrl_c() => {
                break;
            }
        }
    }

    Ok(())
}

impl Controller {
    /// Run this controller, and everything it controls
    ///
    /// # Errors
    ///
    /// This function will return an error if any of the configured modules fail
    /// to initialise.
    pub async fn run(self) -> anyhow::Result<()> {
        logging::init();

        internal!("Controller running");

        modules::init(self.modules)?;

        let ret =
            tokio::select! {
                r = self.smtp_server.serve() => {
                    r
                }
                r = shutdown() => {
                    r
                }
            };

        internal!("Shutting down...");

        ret
    }
}
