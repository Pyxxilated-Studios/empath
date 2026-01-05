use std::net::SocketAddr;

use empath_tracing::traced;
use serde::Deserialize;
use tokio::net::TcpListener;
use tokio::task::JoinSet;

use crate::error::{ListenerError, ProtocolError};
use crate::traits::protocol::{Protocol, SessionHandler};
use crate::{Signal, internal};

pub struct Listener<Proto: Protocol> {
    handler: Proto,
    socket: SocketAddr,
    args: Proto::Args,
    context: Proto::Context,
}

#[derive(Deserialize)]
struct ListenerConfig<Proto: Protocol> {
    socket: SocketAddr,
    #[serde(skip_serializing, default, flatten)]
    args: Proto::Args,
    context: Proto::Context,
}

impl<'de, Proto: Protocol> Deserialize<'de> for Listener<Proto> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let config = ListenerConfig::<Proto>::deserialize(deserializer)?;
        Ok(Self {
            handler: Proto::default(),
            socket: config.socket,
            args: config.args,
            context: config.context,
        })
    }
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

    /// # Errors
    /// Any error during initialisation of the listener may propogate here
    #[traced(instrument(skip(self)), timing(precision = "ns"))]
    pub fn init(&mut self) -> Result<(), ProtocolError> {
        self.handler.validate(&mut self.args)
    }

    /// # Errors
    /// This may fail to bind to the given socket
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

        let mut sessions = JoinSet::new();
        let (address, port) = (self.socket.ip(), self.socket.port());
        let listener =
            TcpListener::bind(self.socket)
                .await
                .map_err(|e| ListenerError::BindFailed {
                    address: self.socket.to_string(),
                    source: e,
                })?;

        let mut shutdown_signal = shutdown.resubscribe();

        let mut draining = false;

        loop {
            tokio::select! {
                sig = shutdown_signal.recv() => {
                    match sig {
                        Ok(Signal::Shutdown) => {
                            internal!(level = INFO, "{} Listener {}:{} Received Shutdown signal, aborting {} active sessions ...", Proto::ty(), address, port, sessions.len());
                            sessions.abort_all();
                            return Ok(());
                        }
                        Ok(Signal::Drain(target)) if target == self.socket => {
                            if !draining {
                                draining = true;
                                internal!(level = INFO, "{} Listener {}:{} entering drain mode ({} active sessions)", Proto::ty(), address, port, sessions.len());
                            }
                        }
                        Ok(_) => {}
                        Err(e) => {
                            internal!(level = WARN, "Listener {}:{} received shutdown channel error: {e}", address, port);
                            return Ok(());
                        }
                    }
                }

                // Handle completed sessions (including panics)
                Some(result) = sessions.join_next() => {
                    match result {
                        Ok(()) => {
                            // Session completed normally
                            tracing::debug!("Session completed on {}", self.socket);
                        }
                        Err(e) if e.is_cancelled() => {
                            // Session was cancelled (shutdown)
                            tracing::debug!("Session cancelled on {}", self.socket);
                        }
                        Err(e) if e.is_panic() => {
                            // Session panicked - log error but continue serving
                            internal!(level = ERROR, "Session panicked on {}: {:?}", self.socket, e);
                        }
                        Err(e) => {
                            // Other join errors
                            internal!(level = ERROR, "Session join error on {}: {e}", self.socket);
                        }
                    }
                    if draining && sessions.is_empty() {
                        internal!(level = INFO, "{} Listener {}:{} drain complete", Proto::ty(), address, port);
                        return Ok(());
                    }
                }

                connection = listener.accept(), if !draining => {
                    tracing::debug!("Connection received on {}", self.socket);

                    let (stream, address) = connection?;
                    let handler = self.handler.handle(stream, address, self.context.clone(), self.args.clone());

                    let signal = shutdown.resubscribe();
                    sessions.spawn(async move {
                        if let Err(err) = handler.run(signal.resubscribe()).await {
                            internal!(level = ERROR, "Error: {err}");
                        }
                    });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use anyhow::{Result, anyhow};
    use serde::Deserialize;
    use tokio::net::TcpStream;
    use tokio::time::Duration;

    use super::*;
    use crate::error::ProtocolError;

    #[derive(Debug, Clone, Default, Deserialize)]
    struct DummyArgs {
        value: u32,
    }

    #[derive(Debug, Clone, Default, Deserialize)]
    struct DummyContext {
        name: String,
    }

    #[derive(Default)]
    struct DummyProtocol;

    struct DummySession;

    impl SessionHandler for DummySession {
        fn run(
            self,
            _signal: tokio::sync::broadcast::Receiver<Signal>,
        ) -> impl std::future::Future<Output = Result<(), crate::error::SessionError>> + Send
        {
            std::future::ready(Ok(()))
        }
    }

    impl Protocol for DummyProtocol {
        type Args = DummyArgs;
        type Context = DummyContext;
        type Session = DummySession;

        fn handle(
            &self,
            _stream: TcpStream,
            _address: SocketAddr,
            _context: Self::Context,
            _args: Self::Args,
        ) -> Self::Session {
            DummySession
        }

        fn validate(&mut self, args: &mut Self::Args) -> Result<(), ProtocolError> {
            if args.value == 0 {
                return Err(ProtocolError::MissingField("value"));
            }
            Ok(())
        }

        fn ty() -> &'static str {
            "dummy"
        }
    }

    #[test]
    fn map_args_updates_listener_args() -> Result<()> {
        let mut listener: Listener<DummyProtocol> = ron::from_str(
            r#"
{
  "socket": "127.0.0.1:0",
  "value": 1,
  "context": (name: "alpha"),
}
"#,
        )?;
        listener.map_args(&|args| DummyArgs {
            value: args.value + 1,
        });
        assert_eq!(listener.args.value, 2);
        assert_eq!(listener.context.name, "alpha");
        Ok(())
    }

    #[test]
    fn init_rejects_invalid_args() -> Result<()> {
        let mut listener: Listener<DummyProtocol> = ron::from_str(
            r#"
{
  "socket": "127.0.0.1:0",
  "value": 0,
  "context": (name: "alpha"),
}
"#,
        )?;
        let err = listener
            .init()
            .err()
            .ok_or_else(|| anyhow!("expected invalid args to fail validation"))?;
        assert!(matches!(err, ProtocolError::MissingField("value")));
        Ok(())
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)]
    async fn serve_shutdown_exits_cleanly() -> Result<()> {
        let mut listener: Listener<DummyProtocol> = ron::from_str(
            r#"
{
  "socket": "127.0.0.1:0",
  "value": 1,
  "context": (name: "alpha"),
}
"#,
        )?;
        listener.init()?;

        let (tx, rx) = tokio::sync::broadcast::channel(1);
        let handle = tokio::spawn(async move { listener.serve(rx).await });

        tokio::time::sleep(Duration::from_millis(50)).await;
        tx.send(Signal::Shutdown)?;
        let result = tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .map_err(|err| anyhow!("serve timed out: {err}"))?;
        let result = result.map_err(|err| anyhow!("serve join error: {err}"))?;
        if let Err(ListenerError::BindFailed { ref source, .. }) = result
            && source.kind() == std::io::ErrorKind::PermissionDenied
        {
            return Ok(());
        }
        result.map_err(|err| anyhow!("serve error: {err}"))?;
        Ok(())
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)]
    async fn serve_reports_bind_failure() -> Result<()> {
        let occupied = match std::net::TcpListener::bind("127.0.0.1:0") {
            Ok(listener) => listener,
            Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied => return Ok(()),
            Err(err) => return Err(err.into()),
        };
        let addr = occupied.local_addr()?;
        let config = format!(
            r#"
{{
  "socket": "{addr}",
  "value": 1,
  "context": (name: "alpha"),
}}
"#
        );
        let listener: Listener<DummyProtocol> = ron::from_str(&config)?;
        let (_tx, rx) = tokio::sync::broadcast::channel(1);
        let err = listener
            .serve(rx)
            .await
            .err()
            .ok_or_else(|| anyhow!("expected bind failure"))?;
        match err {
            ListenerError::BindFailed { source, .. }
                if source.kind() == std::io::ErrorKind::PermissionDenied =>
            {
                Ok(())
            }
            ListenerError::BindFailed { .. } => Ok(()),
            _ => Err(anyhow!("expected bind failure")),
        }
    }
}
