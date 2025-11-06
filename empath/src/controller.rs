use std::sync::LazyLock;

use empath_common::{Signal, controller::Controller, internal, logging, tracing};
use empath_ffi::modules::{self, Module};
use empath_smtp::Smtp;
use empath_tracing::traced;
use serde::Deserialize;
use tokio::sync::broadcast;

#[allow(
    clippy::unsafe_derive_deserialize,
    reason = "The unsafe aspects have nothing to do with the struct"
)]
#[derive(Default, Deserialize)]
pub struct Empath {
    #[serde(alias = "smtp")]
    smtp_controller: Controller<Smtp>,
    #[serde(alias = "module", default)]
    modules: Vec<Module>,
    #[serde(alias = "spool")]
    spool: empath_spool::Controller,
    #[serde(alias = "delivery", default)]
    delivery: empath_delivery::DeliveryProcessor,
}

pub static SHUTDOWN_BROADCAST: LazyLock<broadcast::Sender<Signal>> = LazyLock::new(|| {
    let (sender, _receiver) = broadcast::channel(64);
    sender
});

#[traced(instrument(level = tracing::Level::TRACE))]
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

impl Empath {
    /// Run this controller, and everything it controls
    ///
    /// # Errors
    ///
    /// This function will return an error if any of the configured modules fail
    /// to initialise.
    #[traced(instrument(level = tracing::Level::TRACE, skip_all, err), timing(precision = "s"))]
    pub async fn run(mut self) -> anyhow::Result<()> {
        logging::init();
        self.spool.init()?;

        internal!("Controller running");

        modules::init(self.modules)?;

        // Inject the spool into all SMTP listeners before initialization
        // We need both: the concrete Arc<Controller> for serve() and Arc<dyn Spool> for sessions
        let spool_controller = std::sync::Arc::new(self.spool);
        self.smtp_controller
            .map_args(|args| args.with_spool(spool_controller.clone()));

        self.smtp_controller.init()?;

        // Initialize delivery controller with the same spool controller
        self.delivery.init(spool_controller.clone())?;

        let ret = tokio::select! {
            r = self.smtp_controller.control(vec![SHUTDOWN_BROADCAST.subscribe()]) => {
                r
            }
            r = spool_controller.serve(SHUTDOWN_BROADCAST.subscribe()) => {
                r
            }
            r = self.delivery.serve(SHUTDOWN_BROADCAST.subscribe()) => {
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
