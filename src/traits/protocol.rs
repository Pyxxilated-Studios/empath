use std::net::SocketAddr;

use tokio::net::TcpStream;

#[async_trait::async_trait]
pub trait SessionHandler {
    async fn run(self) -> anyhow::Result<()>;
}

pub trait Protocol: Default + Send + Sync {
    type Session: SessionHandler + Send + Sync + 'static;

    fn handle(&self, stream: TcpStream, address: SocketAddr) -> Self::Session;
}
