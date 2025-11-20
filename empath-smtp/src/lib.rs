#![feature(bstr, result_option_map_or_default)]

pub mod client;
pub mod command;
pub mod connection;
pub mod error;
pub mod extensions;
pub mod fsm;
pub mod session;
pub mod session_state;
pub mod state;
pub mod transaction_handler;

// Re-export commonly used types
use std::{borrow::Cow, collections::HashMap, net::SocketAddr, sync::Arc};

pub use command::MailParameters;
use empath_common::{
    Signal,
    error::{ProtocolError, SessionError},
    traits::protocol::{Protocol, SessionHandler},
};
use empath_tracing::traced;
use serde::Deserialize;
// Re-export the type-safe state machine from the state module
pub use state::State;
use tokio::net::TcpStream;

use crate::{
    extensions::Extension,
    session::{Session, SessionConfig},
};

const MAX_MESSAGE_SIZE: usize = 100;

/// SMTP server-side timeout configuration
///
/// These timeouts prevent resource exhaustion from slow or malicious clients
/// and follow RFC 5321 Section 4.5.3.2 recommendations.
#[derive(Clone, Debug, Deserialize)]
pub struct SmtpServerTimeouts {
    /// Timeout for regular SMTP commands (EHLO, MAIL FROM, RCPT TO, etc.)
    ///
    /// RFC 5321 recommends: 5 minutes
    /// Default: 300 seconds (5 minutes)
    #[serde(default = "default_command_timeout")]
    pub command_secs: u64,

    /// Timeout for DATA command response
    ///
    /// RFC 5321 recommends: 2 minutes
    /// Default: 120 seconds (2 minutes)
    #[serde(default = "default_data_init_timeout")]
    pub data_init_secs: u64,

    /// Timeout between data chunks while receiving message body
    ///
    /// RFC 5321 recommends: 3 minutes
    /// Default: 180 seconds (3 minutes)
    #[serde(default = "default_data_block_timeout")]
    pub data_block_secs: u64,

    /// Timeout for processing after final dot terminator
    ///
    /// RFC 5321 recommends: 10 minutes
    /// Default: 600 seconds (10 minutes)
    #[serde(default = "default_data_termination_timeout")]
    pub data_termination_secs: u64,

    /// Maximum total session duration
    ///
    /// Prevents sessions from living indefinitely.
    /// Default: 1800 seconds (30 minutes)
    #[serde(default = "default_connection_timeout")]
    pub connection_secs: u64,
}

impl Default for SmtpServerTimeouts {
    fn default() -> Self {
        Self {
            command_secs: default_command_timeout(),
            data_init_secs: default_data_init_timeout(),
            data_block_secs: default_data_block_timeout(),
            data_termination_secs: default_data_termination_timeout(),
            connection_secs: default_connection_timeout(),
        }
    }
}

const fn default_command_timeout() -> u64 {
    300 // 5 minutes per RFC 5321
}

const fn default_data_init_timeout() -> u64 {
    120 // 2 minutes per RFC 5321
}

const fn default_data_block_timeout() -> u64 {
    180 // 3 minutes per RFC 5321
}

const fn default_data_termination_timeout() -> u64 {
    600 // 10 minutes per RFC 5321
}

const fn default_connection_timeout() -> u64 {
    1800 // 30 minutes
}

#[derive(Default, Deserialize)]
pub struct Smtp;

#[derive(Clone, Debug, Default, Deserialize)]
pub struct SmtpArgs {
    #[serde(default)]
    extensions: Vec<Extension>,
    #[serde(skip)]
    spool: Option<Arc<dyn empath_spool::BackingStore>>,
    #[serde(default)]
    pub timeouts: SmtpServerTimeouts,
}

impl SmtpArgs {
    /// Create a new `SmtpArgs` builder
    #[must_use]
    pub fn builder() -> Self {
        Self::default()
    }

    /// Set the SMTP extensions supported by this server
    #[must_use]
    pub fn with_extensions(mut self, extensions: Vec<Extension>) -> Self {
        self.extensions = extensions;
        self
    }

    /// Set the spool controller for this SMTP server
    #[must_use]
    pub fn with_spool(mut self, spool: Arc<dyn empath_spool::BackingStore>) -> Self {
        self.spool = Some(spool);
        self
    }

    /// Set the timeout configuration for this SMTP server
    #[must_use]
    pub const fn with_timeouts(mut self, timeouts: SmtpServerTimeouts) -> Self {
        self.timeouts = timeouts;
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
            stream,
            peer,
            SessionConfig::builder()
                .with_extensions(args.extensions)
                .with_spool(args.spool)
                .with_timeouts(args.timeouts)
                .with_init_context(
                    init_context
                        .into_iter()
                        .map(|(k, v)| (Cow::Owned(k), v))
                        .collect(),
                )
                .build(),
        )
    }

    #[traced(instrument(skip(self, args)), timing(precision = "ns"))]
    fn validate(&mut self, args: &mut Self::Args) -> Result<(), ProtocolError> {
        if let Some(Extension::Starttls(tls)) = args
            .extensions
            .iter()
            .find(|arg| matches!(arg, Extension::Starttls(_)))
        {
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
    async fn run(
        self,
        signal: tokio::sync::broadcast::Receiver<Signal>,
    ) -> Result<(), SessionError> {
        Self::run(self, signal).await
    }
}
