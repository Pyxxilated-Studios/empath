use std::net::SocketAddr;

use empath_tracing::traced;
use futures_util::future::join_all;
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;

use crate::{
    controller::{Signal, SHUTDOWN_BROADCAST},
    internal,
    smtp::{extensions::Extension, session::TlsContext},
    traits::protocol::{Protocol, SessionHandler},
};

#[allow(
    clippy::unsafe_derive_deserialize,
    reason = "The unsafe aspects have nothing to do with the struct"
)]
#[derive(Deserialize, Serialize)]
pub struct Listener<Proto: Protocol> {
    #[serde(skip)]
    handler: Proto,
    socket: SocketAddr,
    #[serde(default)]
    extensions: Vec<Extension>,
    #[serde(default)]
    pub(crate) tls: Option<TlsContext>,
    #[serde(skip_serializing, default)]
    context: Proto::Context,
}

impl<Proto: Protocol> Listener<Proto> {
    #[traced(instrument(level = tracing::Level::TRACE, skip_all))]
    pub async fn serve(&self) -> anyhow::Result<()> {
        internal!("Serving {:?} with {:?}", self.socket, self.context);
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
                    tracing::debug!("Connection received on {}", self.socket);
                    let (stream, address) = connection?;
                    let handler = self.handler.handle(stream, address, &self.extensions, self.tls.clone(), self.context.clone());
                    sessions.push(tokio::spawn(async move {
                        if let Err(err) = handler.run().await {
                            internal!(level = ERROR, "Error: {err}");
                        }
                    }));
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
            extensions: Vec::default(),
            tls: None,
            socket,
            context: Proto::Context::default(),
        }
    }
}
