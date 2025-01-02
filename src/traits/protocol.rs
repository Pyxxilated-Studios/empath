use std::{collections::HashMap, fmt::Debug, net::SocketAddr};

use serde::Deserialize;
use tokio::net::TcpStream;

use crate::smtp::{extensions::Extension, session::TlsContext};

pub trait SessionHandler {
    fn run(self) -> impl std::future::Future<Output = anyhow::Result<()>> + Send;
}

pub trait Protocol: Default + Send + Sync {
    type Session: SessionHandler + Send + Sync + 'static;
    type Context: Default + Clone + Debug + Send + Sync + for<'a> Deserialize<'a> =
        HashMap<String, String>;

    fn handle(
        &self,
        stream: TcpStream,
        address: SocketAddr,
        extensions: &[Extension],
        tls_context: Option<TlsContext>,
        context: Self::Context,
    ) -> Self::Session;
}
