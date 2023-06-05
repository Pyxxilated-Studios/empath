use tracing::{metadata::LevelFilter, trace};
use tracing_subscriber::{
    prelude::__tracing_subscriber_SubscriberExt, util::SubscriberInitExt, Layer,
};

#[derive(Default)]
pub struct Logger<'a> {
    id: &'a str,
}

impl<'a> Logger<'a> {
    pub fn init() {
        let level = if let Ok(level) = std::env::var("LOG_LEVEL") {
            match level.to_ascii_lowercase().as_str() {
                "warn" => LevelFilter::WARN,
                "info" => LevelFilter::INFO,
                "trace" => LevelFilter::TRACE,
                _ => LevelFilter::ERROR,
            }
        } else if cfg!(debug_assertions) {
            LevelFilter::TRACE
        } else {
            LevelFilter::INFO
        };

        tracing_subscriber::Registry::default()
            .with(
                (if cfg!(debug_assertions) {
                    tracing_subscriber::fmt::layer()
                } else {
                    tracing_subscriber::fmt::layer()
                        .with_file(false)
                        .with_line_number(false)
                })
                .compact()
                .with_ansi(true)
                .with_filter(level),
            )
            .init();
    }

    /// Create a logger with an id
    pub fn with_id(id: &'a str) -> Logger {
        Logger { id }
    }

    ///
    /// Log an incoming request/response -- e.g. from a client to the server
    ///
    #[tracing::instrument(skip(self, message), fields(id = self.id))]
    pub fn incoming(&self, message: &str) {
        trace!(message);
    }

    ///
    /// Log an outgoing response/request -- e.g. from the server to a client
    ///
    #[tracing::instrument(skip(self, message), fields(id = self.id))]
    pub fn outgoing(&self, message: &str) {
        trace!(message);
    }

    ///
    /// Log an internal message
    ///
    #[tracing::instrument(skip(self, message), fields(id = self.id))]
    pub fn internal(&self, message: &str) {
        trace!(message);
    }
}
