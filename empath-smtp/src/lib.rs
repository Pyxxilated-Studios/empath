#![feature(bstr, result_option_map_or_default)]

pub mod command;
pub mod connection;
pub mod extensions;
pub mod session;
pub mod state;

use std::{collections::HashMap, net::SocketAddr, sync::Arc};

use empath_common::{
    Signal,
    traits::protocol::{Protocol, SessionHandler},
};
use empath_tracing::traced;
use serde::Deserialize;
use tokio::net::TcpStream;

use crate::{
    extensions::Extension,
    session::{Session, SessionConfig, TlsContext},
};

const MAX_MESSAGE_SIZE: usize = 100;

#[derive(Default, Deserialize)]
pub struct Smtp;

#[derive(Clone, Debug, Default, Deserialize)]
pub struct SmtpArgs {
    tls: Option<TlsContext>,
    #[serde(default)]
    extensions: Vec<Extension>,
    #[serde(skip)]
    spool: Option<Arc<dyn empath_spool::Spool>>,
}

impl SmtpArgs {
    /// Create a new `SmtpArgs` builder
    #[must_use]
    pub fn builder() -> Self {
        Self::default()
    }

    /// Set the TLS context for STARTTLS support
    #[must_use]
    pub fn with_tls(mut self, tls: Option<TlsContext>) -> Self {
        self.tls = tls;
        self
    }

    /// Set the SMTP extensions supported by this server
    #[must_use]
    pub fn with_extensions(mut self, extensions: Vec<Extension>) -> Self {
        self.extensions = extensions;
        self
    }

    /// Set the spool controller for this SMTP server
    #[must_use]
    pub fn with_spool(mut self, spool: Arc<dyn empath_spool::Spool>) -> Self {
        self.spool = Some(spool);
        self
    }
}

impl Protocol for Smtp {
    type Session = Session<TcpStream>;
    type Args = SmtpArgs;

    fn ty() -> &'static str {
        "SMTP"
    }

    #[traced(instrument(level = tracing::Level::TRACE, skip(self, stream, init_context, args)), timing(precision = "ms"))]
    fn handle(
        &self,
        stream: TcpStream,
        peer: SocketAddr,
        init_context: HashMap<String, String>,
        args: Self::Args,
    ) -> Self::Session {
        Session::create(
            Arc::default(),
            stream,
            peer,
            SessionConfig::builder()
                .with_extensions(args.extensions)
                .with_tls_context(args.tls)
                .with_spool(args.spool)
                .with_init_context(init_context)
                .build(),
        )
    }

    #[traced(instrument(skip(self, args)), timing(precision = "ns"))]
    fn validate(&mut self, args: &mut Self::Args) -> anyhow::Result<()> {
        if let Some(tls) = args.tls.as_ref() {
            if !tls.certificate.try_exists()? {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!(
                        "Unable to find TLS Certificate {}",
                        tls.certificate.display()
                    ),
                )
                .into());
            }

            if !tls.key.try_exists()? {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("Unable to find TLS Key {}", tls.key.display()),
                )
                .into());
            }
        }

        if !args
            .extensions
            .iter()
            .any(|ext| matches!(ext, Extension::Size(_)))
        {
            args.extensions.push(Extension::Size(MAX_MESSAGE_SIZE));
        }

        Ok(())
    }
}

impl SessionHandler for Session<TcpStream> {
    async fn run(self, signal: tokio::sync::broadcast::Receiver<Signal>) -> anyhow::Result<()> {
        Self::run(self, signal).await
    }
}

// Re-export the type-safe State enum from the state module
pub use state::State;
