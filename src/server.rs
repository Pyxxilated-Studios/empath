use serde::{Deserialize, Serialize};

use crate::{internal, listener::Listener, traits::protocol::Protocol};

#[derive(Default, Deserialize, Serialize)]
pub struct Server<Proto: Protocol> {
    #[serde(alias = "listener")]
    listeners: Vec<Listener<Proto>>,
}

impl<Proto: Protocol> Server<Proto> {
    pub async fn serve(&self) -> anyhow::Result<()> {
        internal!("Server::serve");
        futures_util::future::join_all(self.listeners.iter().map(Listener::serve)).await;

        Ok(())
    }
}
