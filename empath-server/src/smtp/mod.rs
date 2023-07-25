use std::{
    net::{IpAddr, Ipv6Addr, SocketAddr},
    sync::{atomic::AtomicU64, Arc},
};

use empath_common::{internal, listener::Listener};
use empath_smtp_proto::extensions::Extension;
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;

use crate::smtp::session::Session;

use self::session::TlsContext;

mod connection;
mod session;

#[derive(Serialize, Deserialize, Clone)]
pub struct Smtp {
    address: IpAddr,
    port: u16,
    #[serde(skip)]
    extensions: Vec<Extension>,
    #[serde(default)]
    banner: String,
    #[serde(default)]
    tls_context: TlsContext,
}

impl Default for Smtp {
    fn default() -> Self {
        Self {
            address: IpAddr::V6(Ipv6Addr::UNSPECIFIED),
            port: 1025,
            extensions: Vec::default(),
            banner: String::default(),
            tls_context: TlsContext::default(),
        }
    }
}

#[typetag::serde]
#[async_trait::async_trait]
impl Listener for Smtp {
    async fn spawn(&self) {
        internal!(
            level = INFO,
            "Starting SMTP Listener on: {}:{}",
            self.address,
            self.port
        );

        let smtplistener = self.clone();
        let listener = TcpListener::bind(SocketAddr::new(smtplistener.address, smtplistener.port))
            .await
            .expect("Unable to start smtp session");
        let queue = Arc::new(AtomicU64::default());

        loop {
            let (stream, address) = listener
                .accept()
                .await
                .expect("Unable to accept connection");

            tokio::spawn(
                Session::create(
                    Arc::clone(&queue),
                    stream,
                    address,
                    self.extensions.clone(),
                    self.tls_context.clone(),
                    self.banner.clone(),
                )
                .run(),
            );
        }
    }
}
