use std::net::SocketAddr;

use empath_tracing::traced;
use futures_util::future::join_all;
use serde::Deserialize;
use tokio::net::TcpListener;

use crate::{
    internal,
    traits::protocol::{Protocol, SessionHandler},
    Signal,
};

#[allow(
    clippy::unsafe_derive_deserialize,
    reason = "The unsafe aspects have nothing to do with the struct"
)]
#[derive(Deserialize)]
pub struct Listener<Proto: Protocol> {
    #[serde(skip)]
    handler: Proto,
    socket: SocketAddr,
    #[serde(skip_serializing, default, flatten)]
    args: Proto::Args,
    context: Proto::Context,
}

unsafe impl<P: Protocol> Send for Listener<P> {}
unsafe impl<P: Protocol> Sync for Listener<P> {}

impl<Proto: Protocol> Listener<Proto> {
    #[traced(instrument(skip(self)), timing(precision = "ns"))]
    pub fn init(&self) -> anyhow::Result<()> {
        self.handler.validate(&self.args)
    }

    #[traced(instrument(level = tracing::Level::TRACE, skip(self, shutdown)), timing(precision = "s"))]
    pub async fn serve(
        &self,
        mut shutdown: tokio::sync::broadcast::Receiver<Signal>,
    ) -> anyhow::Result<()> {
        internal!("Serving {:?} with {:?}", self.socket, self.context);

        let mut sessions = Vec::default();
        let (address, port) = (self.socket.ip(), self.socket.port());
        let listener = TcpListener::bind(self.socket).await?;

        loop {
            tokio::select! {
                sig = shutdown.recv() => {
                    if matches!(sig, Ok(Signal::Shutdown)) {
                        internal!(level = INFO, "{} Listener {}:{} Received Shutdown signal, finishing sessions ...", Proto::ty(), address, port);
                        join_all(sessions).await;
                        return Ok(());
                    }
                }

                connection = listener.accept() => {
                    tracing::debug!("Connection received on {}", self.socket);

                    let (stream, address) = connection?;
                    let handler = self.handler.handle(stream, address, self.context.clone(), self.args.clone());

                    sessions.push(tokio::spawn(async move {
                        if let Err(err) = handler.run().await {
                            internal!(level = ERROR, "Error: {err}");
                        }
                    }));
                }
            }
        }
    }
}
