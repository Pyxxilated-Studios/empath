use tracing::{metadata::LevelFilter, trace};
use tracing_subscriber::{
    prelude::__tracing_subscriber_SubscriberExt, util::SubscriberInitExt, Layer,
};

#[derive(Default)]
pub struct Logger;

impl Logger {
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

    ///
    /// Log an incoming request/response -- e.g. from a client to the server
    ///
    #[tracing::instrument(skip(message))]
    pub fn incoming(message: &str) {
        trace!(message);
    }

    ///
    /// Log an outgoing response/request -- e.g. from the server to a client
    ///
    #[tracing::instrument(skip(message))]
    pub fn outgoing(message: &str) {
        trace!(message);
    }

    ///
    /// Log an internal message
    ///
    #[tracing::instrument(skip(message))]
    pub fn internal(message: &str) {
        trace!(message);
    }
}
