use std::net::SocketAddr;

use tokio::net::TcpStream;

use crate::smtp::{extensions::Extension, session::TlsContext};

#[async_trait::async_trait]
pub trait SessionHandler {
    async fn run(self) -> anyhow::Result<()>;
}

pub trait Protocol: Default + Send + Sync {
    type Session: SessionHandler + Send + Sync + 'static;

    fn handle(
        &self,
        stream: TcpStream,
        address: SocketAddr,
        extensions: &[Extension],
        tls_context: Option<TlsContext>,
    ) -> Self::Session;
}
