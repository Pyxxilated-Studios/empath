use std::net::SocketAddr;

use futures_util::future::join_all;
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;

use empath_common::{
    internal, tracing,
    traits::protocol::{Protocol, SessionHandler},
    Signal,
};
use empath_tracing::traced;

use crate::{extensions::Extension, session::TlsContext};

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

impl<Proto: Protocol<ExtraArgs = (Vec<Extension>, Option<TlsContext>)>> Listener<Proto> {
    #[traced(instrument(level = tracing::Level::TRACE, skip_all, err))]
    pub async fn serve(
        &self,
        mut shutdown: tokio::sync::broadcast::Receiver<Signal>,
    ) -> anyhow::Result<()> {
        internal!("Serving {:?} with {:?}", self.socket, self.context);
        let mut sessions = Vec::default();

        let (address, port) = (self.socket.ip(), self.socket.port());
        let listener = TcpListener::bind(self.socket).await?;

        if let Some(tls) = self.tls.as_ref() {
            if !tls.certificate.try_exists()? {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("Unable to find TLS Certificate {:?}", tls.certificate),
                )
                .into());
            }

            if !tls.key.try_exists()? {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("Unable to find TLS Key {:?}", tls.key),
                )
                .into());
            }
        }

        loop {
            tokio::select! {
                sig = shutdown.recv() => {
                    if matches!(sig, Ok(Signal::Shutdown)) {
                        internal!(level = INFO, "SMTP Listener {}:{} Received Shutdown signal, finishing sessions ...", address, port);
                        join_all(sessions).await;
                        // SHUTDOWN_BROADCAST.send(Signal::Finalised)?;
                        break;
                    }
                }

                connection = listener.accept() => {
                    tracing::debug!("Connection received on {}", self.socket);
                    let (stream, address) = connection?;
                    let handler = self.handler.handle(stream, address, self.context.clone(), (self.extensions.clone(), self.tls.clone()));
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
