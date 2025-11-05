use empath_tracing::traced;
use futures_util::future::join_all;
use serde::Deserialize;
use tokio::sync::broadcast::Receiver;

use crate::{Signal, internal, listener::Listener, traits::Protocol};

#[derive(Default, Deserialize)]
pub struct Controller<Proto: Protocol> {
    #[serde(alias = "listener")]
    listeners: Vec<Listener<Proto>>,
}

impl<Proto: Protocol> Controller<Proto> {
    /// Map over the args of all listeners, allowing modification before initialization
    ///
    /// This is useful for injecting dependencies that cannot be deserialized from TOML,
    /// such as shared spool controllers or other runtime resources.
    pub fn map_args<F>(&mut self, f: F)
    where
        F: Fn(Proto::Args) -> Proto::Args,
    {
        for listener in &mut self.listeners {
            listener.map_args(&f);
        }
    }

    ///
    /// Initialise this controller
    ///
    /// # Errors
    /// Any errors initialising this controller
    ///
    pub fn init(&mut self) -> anyhow::Result<()> {
        internal!("Initialising Controller for {}", Proto::ty());

        self.listeners.iter().try_for_each(Listener::init)
    }

    ///
    /// # Errors
    /// If any of the listeners have a failure
    ///
    #[traced(instrument(level = tracing::Level::TRACE, skip(self, signals)), timing(precision = "s"))]
    pub async fn control(self, signals: Vec<Receiver<Signal>>) -> anyhow::Result<()> {
        join_all(
            self.listeners
                .iter()
                .map(|l| l.serve(signals[0].resubscribe())),
        )
        .await
        .into_iter()
        .try_for_each(|a| a)
    }
}
