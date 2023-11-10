use std::net::SocketAddr;

use futures_util::future::join_all;
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;

use crate::{
    controller::{Signal, SHUTDOWN_BROADCAST},
    internal,
    traits::protocol::{Protocol, SessionHandler},
};

#[allow(clippy::unsafe_derive_deserialize)]
#[derive(Deserialize, Serialize)]
pub struct Listener<Proto: Protocol> {
    #[serde(skip)]
    handler: Proto,
    socket: SocketAddr,
}

impl<Proto: Protocol> Listener<Proto> {
    pub async fn serve(&self) -> anyhow::Result<()> {
        internal!("Listener::serve");
        let mut sessions = Vec::default();

        let (address, port) = (self.socket.ip(), self.socket.port());
        let listener = TcpListener::bind(self.socket).await?;

        let mut receiver = SHUTDOWN_BROADCAST.subscribe();

        loop {
            tokio::select! {
                sig = receiver.recv() => {
                    if matches!(sig, Ok(Signal::Shutdown)) {
                        internal!(level = INFO, "SMTP Listener {}:{} Received Shutdown signal, finishing sessions ...", address, port);
                        join_all(sessions).await;
                        SHUTDOWN_BROADCAST.send(Signal::Finalised)?;
                        break;
                    }
                }

                connection = listener.accept() => {
                    tracing::debug!("Connection received");
                    let (stream, address) = connection?;
                    let handler = self.handler.handle(stream, address);
                    sessions.push(tokio::spawn(async move { handler.run().await }));
                }
            }
        }

        Ok(())
    }
}

impl<Proto: Protocol> From<SocketAddr> for Listener<Proto> {
    fn from(socket: SocketAddr) -> Self {
        Self {
            handler: Proto::default(),
            socket,
        }
    }
}
