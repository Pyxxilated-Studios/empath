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
