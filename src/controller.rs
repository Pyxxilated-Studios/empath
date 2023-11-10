use std::sync::LazyLock;

use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::{
    ffi::module::{self, Module},
    internal,
    server::Server,
    smtp::Smtp,
};

#[allow(clippy::unsafe_derive_deserialize)]
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
    let _ = tokio::signal::ctrl_c().await;
    internal!("CTRL+C entered -- Enter it again to force shutdown");

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
        crate::logging::init();

        internal!("Controller running");

        module::init(self.modules)?;

        tokio::select! {
            _ = self.smtp_server.serve() => {}
            _ = shutdown() => {}
        };

        internal!("Shutting down...");

        Ok(())
    }
}
