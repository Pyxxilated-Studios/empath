#![feature(bstr, result_option_map_or_default)]

pub mod command;
pub mod connection;
pub mod extensions;
pub mod session;
pub mod state;

use std::{
    collections::HashMap,
    fmt::{self, Display, Formatter},
    net::SocketAddr,
    sync::Arc,
};

use empath_common::{
    Signal,
    context::Context,
    traits::{
        FiniteStateMachine,
        protocol::{Protocol, SessionHandler},
    },
};
use empath_tracing::traced;
use serde::Deserialize;
use tokio::net::TcpStream;

use crate::{
    command::{Command, HeloVariant},
    extensions::Extension,
    session::{Session, SessionConfig},
};

const MAX_MESSAGE_SIZE: usize = 100;

#[derive(Default, Deserialize)]
pub struct Smtp;

#[derive(Clone, Debug, Default, Deserialize)]
pub struct SmtpArgs {
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
                .with_spool(args.spool)
                .with_init_context(init_context)
                .build(),
        )
    }

    #[traced(instrument(skip(self, args)), timing(precision = "ns"))]
    fn validate(&mut self, args: &mut Self::Args) -> anyhow::Result<()> {
        if let Some(ext) = args
            .extensions
            .iter()
            .find(|arg| matches!(arg, Extension::Starttls(_)))
        {
            match ext {
                Extension::Starttls(tls) => {
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
                _ => unreachable!(),
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

#[repr(C)]
#[derive(PartialEq, PartialOrd, Eq, Hash, Debug, Clone, Copy, Deserialize, Default)]
pub enum State {
    #[default]
    Connect,
    Ehlo,
    Helo,
    Help,
    StartTLS,
    MailFrom,
    RcptTo,
    Data,
    Reading,
    PostDot,
    Quit,
    InvalidCommandSequence,
    Invalid,
    Reject,
    Close,
}

impl Display for State {
    fn fmt(&self, fmt: &mut Formatter<'_>) -> Result<(), fmt::Error> {
        fmt.write_str(match self {
            Self::Reading | Self::PostDot => "",
            Self::Connect => "Connect",
            Self::Close => "Close",
            Self::Ehlo => "EHLO",
            Self::Helo => "HELO",
            Self::Help => "HELP",
            Self::StartTLS => "STARTTLS",
            Self::MailFrom => "MAIL",
            Self::RcptTo => "RCPT",
            Self::Data => "DATA",
            Self::Quit => "QUIT",
            Self::Invalid => "INVALID",
            Self::InvalidCommandSequence => "Invalid Command Sequence",
            Self::Reject => "Rejected",
        })
    }
}

impl TryFrom<&str> for State {
    type Error = Self;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value.to_ascii_uppercase().trim() {
            "EHLO" => Ok(Self::Ehlo),
            "HELO" => Ok(Self::Helo),
            "HELP" => Ok(Self::Help),
            "STARTTLS" => Ok(Self::StartTLS),
            "MAIL" => Ok(Self::MailFrom),
            "RCPT" => Ok(Self::RcptTo),
            "DATA" => Ok(Self::Data),
            "QUIT" => Ok(Self::Quit),
            _ => Err(Self::Invalid),
        }
    }
}

impl FiniteStateMachine for State {
    type Input = Command;
    type Context = Context;

    fn transition(self, input: Self::Input, validate_context: &mut Self::Context) -> Self {
        match (self, input) {
            (Self::Connect, Command::Helo(HeloVariant::Ehlo(id))) => {
                validate_context.id = id;
                validate_context.extended = true;
                Self::Ehlo
            }
            (Self::Connect, Command::Helo(HeloVariant::Helo(id))) => {
                validate_context.id = id;
                Self::Helo
            }
            (Self::Ehlo | Self::Helo, Command::StartTLS) if validate_context.extended => {
                Self::StartTLS
            }
            (Self::Ehlo | Self::Helo, Command::Help) => Self::Help,
            (
                Self::Ehlo | Self::Helo | Self::StartTLS | Self::PostDot | Self::Help,
                Command::MailFrom(from, size),
            ) => {
                validate_context.envelope.sender_mut().clone_from(&from);
                // Store the declared size in the context for validation
                if let Some(size_val) = size {
                    validate_context
                        .context
                        .insert(String::from("declared_size"), size_val.to_string());
                }
                Self::MailFrom
            }
            (Self::RcptTo | Self::MailFrom, Command::RcptTo(to)) => {
                if let Some(rcpts) = validate_context.envelope.recipients_mut() {
                    rcpts.extend_from_slice(&to[..]);
                } else {
                    *validate_context.envelope.recipients_mut() = Some(to);
                }
                Self::RcptTo
            }
            (Self::RcptTo, Command::Data) => Self::Data,
            (Self::Data, comm) if comm != Command::Quit => Self::Connect,
            (_, Command::Rset) => {
                // RSET clears transaction state including declared size
                validate_context.context.remove("declared_size");
                *validate_context.envelope.sender_mut() = None;
                *validate_context.envelope.recipients_mut() = None;
                Self::Helo
            }
            (_, Command::Quit) => Self::Quit,
            (Self::Invalid, _) => Self::Invalid,
            _ => Self::InvalidCommandSequence,
        }
    }
}
