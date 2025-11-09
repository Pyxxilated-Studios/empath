use std::{collections::HashMap, fmt::Debug, net::SocketAddr};

use serde::Deserialize;
use tokio::net::TcpStream;

use crate::{
    Signal,
    error::{ProtocolError, SessionError},
};

pub trait SessionHandler {
    fn run(
        self,
        signal: tokio::sync::broadcast::Receiver<Signal>,
    ) -> impl std::future::Future<Output = Result<(), SessionError>> + Send;
}

pub trait Protocol: Default + Send + Sync {
    type Session: SessionHandler + Send + Sync + 'static;
    type Context: Default + Clone + Debug + Send + Sync + for<'a> Deserialize<'a> =
        HashMap<String, String>;
    type Args: Default + Clone + Debug + Send + Sync + for<'a> Deserialize<'a>;

    fn handle(
        &self,
        stream: TcpStream,
        address: SocketAddr,
        context: Self::Context,
        args: Self::Args,
    ) -> Self::Session;

    ///
    /// Validate the arguments being provided to the protocol
    ///
    /// # Errors
    /// This really depends on what needs to be done in order to validate the protocols arguments.
    ///
    /// For example, when providing TLS certificates/keys it may be necessary to check that the
    /// paths provided actually exist
    ///
    fn validate(&mut self, args: &mut Self::Args) -> Result<(), ProtocolError>;

    fn ty() -> &'static str;
}
