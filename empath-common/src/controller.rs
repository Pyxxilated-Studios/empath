use empath_tracing::traced;
use futures_util::future::join_all;
use serde::Deserialize;
use tokio::sync::broadcast::Receiver;

use crate::error::ControllerError;
use crate::listener::Listener;
use crate::traits::Protocol;
use crate::{Signal, internal};

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

    /// Initialise this controller
    ///
    /// # Errors
    /// Any errors initialising this controller
    pub fn init(&mut self) -> Result<(), ControllerError> {
        internal!("Initialising Controller for {}", Proto::ty());

        self.listeners
            .iter_mut()
            .try_for_each(Listener::init)
            .map_err(ControllerError::Protocol)
    }

    /// # Errors
    /// If any of the listeners have a failure
    #[traced(instrument(level = tracing::Level::TRACE, skip(self, signals)), timing(precision = "s"))]
    pub async fn control(self, signals: Vec<Receiver<Signal>>) -> Result<(), ControllerError> {
        join_all(
            self.listeners
                .iter()
                .map(|l| l.serve(signals[0].resubscribe())),
        )
        .await
        .into_iter()
        .try_for_each(|a| a.map_err(Into::into))
    }
}

#[cfg(test)]
mod tests {
    use anyhow::{Result, anyhow};
    use serde::Deserialize;
    use tokio::net::TcpStream;
    use tokio::time::{Duration, timeout};

    use super::*;
    use crate::error::{ListenerError, ProtocolError};
    use crate::traits::protocol::SessionHandler;

    #[derive(Debug, Clone, Default, Deserialize)]
    struct DummyArgs {
        value: u32,
    }

    #[derive(Debug, Clone, Default, Deserialize)]
    struct DummyContext {
        name: String,
    }

    #[derive(Default, Deserialize)]
    struct DummyProtocol;

    struct DummySession {
        context_name: String,
    }

    impl SessionHandler for DummySession {
        fn run(
            self,
            _signal: tokio::sync::broadcast::Receiver<Signal>,
        ) -> impl std::future::Future<Output = Result<(), crate::error::SessionError>> + Send
        {
            let _ = self.context_name.len();
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
            _address: std::net::SocketAddr,
            context: Self::Context,
            _args: Self::Args,
        ) -> Self::Session {
            DummySession {
                context_name: context.name,
            }
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
    fn map_args_allows_initialization() -> Result<()> {
        let listener: Listener<DummyProtocol> = ron::from_str(
            r#"
{
  "socket": "127.0.0.1:0",
  "value": 0,
  "context": (name: "alpha"),
}
"#,
        )?;
        let mut controller = Controller {
            listeners: vec![listener],
        };
        controller.map_args(|args| DummyArgs {
            value: args.value + 1,
        });
        controller
            .init()
            .map_err(|err| anyhow!("init failed: {err}"))?;
        Ok(())
    }

    #[test]
    fn init_propagates_protocol_error() -> Result<()> {
        let listener: Listener<DummyProtocol> = ron::from_str(
            r#"
{
  "socket": "127.0.0.1:0",
  "value": 0,
  "context": (name: "alpha"),
}
"#,
        )?;
        let mut controller = Controller {
            listeners: vec![listener],
        };
        let err = controller
            .init()
            .err()
            .ok_or_else(|| anyhow!("expected init to fail"))?;
        assert!(matches!(
            err,
            ControllerError::Protocol(ProtocolError::MissingField("value"))
        ));
        Ok(())
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)]
    async fn control_shutdown_completes() -> Result<()> {
        let listener: Listener<DummyProtocol> = ron::from_str(
            r#"
{
  "socket": "127.0.0.1:0",
  "value": 1,
  "context": (name: "alpha"),
}
"#,
        )?;
        let controller = Controller {
            listeners: vec![listener],
        };
        let (tx, rx) = tokio::sync::broadcast::channel(1);
        let handle = tokio::spawn(controller.control(vec![rx]));
        tokio::time::sleep(Duration::from_millis(50)).await;
        tx.send(Signal::Shutdown)?;
        let result = timeout(Duration::from_secs(2), handle)
            .await
            .map_err(|err| anyhow!("control timed out: {err}"))?;
        let result = result.map_err(|err| anyhow!("control join error: {err}"))?;
        if let Err(ControllerError::Listener(ListenerError::BindFailed { ref source, .. })) = result
            && source.kind() == std::io::ErrorKind::PermissionDenied
        {
            return Ok(());
        }
        result.map_err(|err| anyhow!("control error: {err}"))?;
        Ok(())
    }
}
