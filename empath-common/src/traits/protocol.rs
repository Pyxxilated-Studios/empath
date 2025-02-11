use std::{collections::HashMap, fmt::Debug, net::SocketAddr};

use serde::Deserialize;
use tokio::net::TcpStream;

pub trait SessionHandler {
    fn run(self) -> impl std::future::Future<Output = anyhow::Result<()>> + Send;
}

pub trait Protocol: Default + Send + Sync {
    type Session: SessionHandler + Send + Sync + 'static;
    type Context: Default + Clone + Debug + Send + Sync + for<'a> Deserialize<'a> =
        HashMap<String, String>;
    type ExtraArgs;

    fn handle(
        &self,
        stream: TcpStream,
        address: SocketAddr,
        context: Self::Context,
        args: Self::ExtraArgs,
    ) -> Self::Session;
}
