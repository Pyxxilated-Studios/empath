use std::net::SocketAddr;

use empath_tracing::traced;
use futures_util::future::join_all;
use serde::Deserialize;
use tokio::net::TcpListener;

use crate::{
    Signal,
    error::{ListenerError, ProtocolError},
    internal,
    traits::protocol::{Protocol, SessionHandler},
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
    /// Apply a function to transform the args for this listener
    ///
    /// This allows injecting runtime dependencies before initialization
    pub fn map_args<F>(&mut self, f: &F)
    where
        F: Fn(Proto::Args) -> Proto::Args,
    {
        self.args = f(self.args.clone());
    }

    ///
    /// # Errors
    /// Any error during initialisation of the listener may propogate here
    ///
    #[traced(instrument(skip(self)), timing(precision = "ns"))]
    pub fn init(&mut self) -> Result<(), ProtocolError> {
        self.handler.validate(&mut self.args)
    }

    ///
    /// # Errors
    /// This may fail to bind to the given socket
    ///
    #[traced(instrument(level = tracing::Level::TRACE, skip(self, shutdown)), timing(precision = "s"))]
    pub async fn serve(
        &self,
        shutdown: tokio::sync::broadcast::Receiver<Signal>,
    ) -> Result<(), ListenerError> {
        internal!(
            "Serving {:?} with {:?}, args: {:?}",
            self.socket,
            self.context,
            self.args
        );

        let mut sessions = Vec::default();
        let (address, port) = (self.socket.ip(), self.socket.port());
        let listener =
            TcpListener::bind(self.socket)
                .await
                .map_err(|e| ListenerError::BindFailed {
                    address: self.socket.to_string(),
                    source: e,
                })?;

        let mut shutdown_signal = shutdown.resubscribe();

        loop {
            tokio::select! {
                sig = shutdown_signal.recv() => {
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

                    let signal = shutdown.resubscribe();
                    sessions.push(tokio::spawn(async move {
                        if let Err(err) = handler.run(signal.resubscribe()).await {
                            internal!(level = ERROR, "Error: {err}");
                        }
                    }));
                }
            }
        }
    }
}
