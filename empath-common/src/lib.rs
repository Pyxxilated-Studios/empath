pub mod audit;
pub mod config;
pub mod context;
pub mod controller;
pub mod error;
pub mod listener;
pub mod traits;

#[cfg(feature = "observability")]
pub mod logging;

#[cfg(not(feature = "observability"))]
pub mod logging {
    #[derive(Debug, Default)]
    pub struct LoggingHandle;

    #[derive(Debug, Default)]
    pub struct LoggingConfigBuilder;

    impl LoggingConfigBuilder {
        #[must_use]
        pub const fn new() -> Self {
            Self
        }

        #[must_use]
        pub const fn with_default_level(self, _level: tracing::metadata::LevelFilter) -> Self {
            self
        }

        #[must_use]
        pub fn with_otlp_endpoint(self, _endpoint: impl Into<String>) -> Self {
            self
        }

        pub fn init(self) -> anyhow::Result<LoggingHandle> {
            Ok(LoggingHandle)
        }
    }

    impl LoggingHandle {
        pub fn set_log_level(&self, _level: tracing::metadata::LevelFilter) -> Result<(), String> {
            Ok(())
        }
    }
}

pub use context::{DeliveryAttempt, DeliveryContext, DeliveryStatus};
pub use empath_core::{
    Domain, SpooledMessageId, address, address_parser, domain, email, envelope, message,
    message_api, message_id, mime, spf, status,
};
pub use tracing;

#[derive(Debug, Clone, Copy)]
pub enum Signal {
    Shutdown,
    Finalised,
    Drain(std::net::SocketAddr),
}
