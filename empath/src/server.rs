use empath_tracing::traced;
use serde::{Deserialize, Serialize};

use crate::{listener::Listener, traits::protocol::Protocol};

#[derive(Default, Deserialize, Serialize)]
pub struct Server<Proto: Protocol> {
    #[serde(alias = "listener")]
    listeners: Vec<Listener<Proto>>,
}

impl<Proto: Protocol> Server<Proto> {
    #[traced(instrument(level = tracing::Level::TRACE, skip_all), timing(precision = "us"))]
    pub async fn serve(&self) -> anyhow::Result<()> {
        futures_util::future::join_all(self.listeners.iter().map(Listener::serve)).await;

        Ok(())
    }
}
