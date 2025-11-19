//! Unified timeout configuration for SMTP operations.
//!
//! This module provides a consistent interface for timeout configuration
//! across both server-side (receiving mail via SMTP) and client-side
//! (delivering mail via SMTP) operations.
//!
//! ## Design
//!
//! Different contexts require different timeout values:
//! - **Server-side**: RFC 5321 compliant timeouts for receiving mail
//! - **Client-side**: Optimized timeouts for delivery operations
//!
//! Both implement a common `TimeoutConfig` trait for unified access.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Common interface for timeout configuration.
///
/// This trait provides a unified way to access timeout values regardless
/// of whether they're for server-side or client-side SMTP operations.
pub trait TimeoutConfig {
    /// Timeout for SMTP command processing.
    fn command_timeout(&self) -> Duration;

    /// Timeout for DATA command and message transfer.
    fn data_timeout(&self) -> Duration;

    /// Maximum connection duration.
    fn connection_timeout(&self) -> Duration;
}

/// Server-side SMTP timeout configuration (RFC 5321 compliant).
///
/// These timeouts are used when receiving mail via SMTP. They follow
/// RFC 5321 recommendations for SMTP server implementations.
///
/// # RFC 5321 Recommendations
///
/// - Initial 220 response: 5 minutes
/// - MAIL/RCPT commands: 5 minutes
/// - DATA initiation: 2 minutes
/// - DATA block: 3 minutes
/// - DATA termination: 10 minutes
/// - Overall connection: 30 minutes
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerTimeouts {
    /// Timeout for SMTP commands (HELO, EHLO, MAIL FROM, RCPT TO, etc.)
    ///
    /// Default: 300 seconds (5 minutes, per RFC 5321)
    #[serde(default = "defaults::server_command_timeout_secs")]
    pub command_secs: u64,

    /// Timeout for DATA command initiation.
    ///
    /// Default: 120 seconds (2 minutes, per RFC 5321)
    #[serde(default = "defaults::server_data_init_secs")]
    pub data_init_secs: u64,

    /// Timeout for receiving each block of message data.
    ///
    /// Default: 180 seconds (3 minutes, per RFC 5321)
    #[serde(default = "defaults::server_data_block_secs")]
    pub data_block_secs: u64,

    /// Timeout for the final dot terminating the message.
    ///
    /// Default: 600 seconds (10 minutes, per RFC 5321)
    #[serde(default = "defaults::server_data_termination_secs")]
    pub data_termination_secs: u64,

    /// Maximum total connection duration.
    ///
    /// Default: 1800 seconds (30 minutes)
    #[serde(default = "defaults::server_connection_secs")]
    pub connection_secs: u64,
}

impl Default for ServerTimeouts {
    fn default() -> Self {
        Self {
            command_secs: defaults::server_command_timeout_secs(),
            data_init_secs: defaults::server_data_init_secs(),
            data_block_secs: defaults::server_data_block_secs(),
            data_termination_secs: defaults::server_data_termination_secs(),
            connection_secs: defaults::server_connection_secs(),
        }
    }
}

impl TimeoutConfig for ServerTimeouts {
    fn command_timeout(&self) -> Duration {
        Duration::from_secs(self.command_secs)
    }

    fn data_timeout(&self) -> Duration {
        Duration::from_secs(self.data_init_secs)
    }

    fn connection_timeout(&self) -> Duration {
        Duration::from_secs(self.connection_secs)
    }
}

/// Client-side SMTP timeout configuration (optimized for delivery).
///
/// These timeouts are used when delivering mail via SMTP. They're more
/// aggressive than server-side timeouts to enable faster failure detection
/// and retry logic.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientTimeouts {
    /// Timeout for establishing TCP connection.
    ///
    /// Default: 30 seconds
    #[serde(default = "defaults::client_connect_secs")]
    pub connect_secs: u64,

    /// Timeout for EHLO/HELO command.
    ///
    /// Default: 30 seconds
    #[serde(default = "defaults::client_ehlo_secs")]
    pub ehlo_secs: u64,

    /// Timeout for STARTTLS command.
    ///
    /// Default: 30 seconds
    #[serde(default = "defaults::client_starttls_secs")]
    pub starttls_secs: u64,

    /// Timeout for MAIL FROM command.
    ///
    /// Default: 30 seconds
    #[serde(default = "defaults::client_mail_from_secs")]
    pub mail_from_secs: u64,

    /// Timeout for RCPT TO command.
    ///
    /// Default: 30 seconds
    #[serde(default = "defaults::client_rcpt_to_secs")]
    pub rcpt_to_secs: u64,

    /// Timeout for DATA command and message transfer.
    ///
    /// Default: 120 seconds (2 minutes)
    #[serde(default = "defaults::client_data_secs")]
    pub data_secs: u64,

    /// Timeout for QUIT command.
    ///
    /// Default: 10 seconds (doesn't fail delivery if timeout occurs)
    #[serde(default = "defaults::client_quit_secs")]
    pub quit_secs: u64,
}

impl Default for ClientTimeouts {
    fn default() -> Self {
        Self {
            connect_secs: defaults::client_connect_secs(),
            ehlo_secs: defaults::client_ehlo_secs(),
            starttls_secs: defaults::client_starttls_secs(),
            mail_from_secs: defaults::client_mail_from_secs(),
            rcpt_to_secs: defaults::client_rcpt_to_secs(),
            data_secs: defaults::client_data_secs(),
            quit_secs: defaults::client_quit_secs(),
        }
    }
}

impl TimeoutConfig for ClientTimeouts {
    fn command_timeout(&self) -> Duration {
        Duration::from_secs(self.ehlo_secs)
    }

    fn data_timeout(&self) -> Duration {
        Duration::from_secs(self.data_secs)
    }

    fn connection_timeout(&self) -> Duration {
        // Client doesn't have an overall connection timeout,
        // so we use the sum of typical operation timeouts
        Duration::from_secs(
            self.connect_secs
                + self.ehlo_secs
                + self.mail_from_secs
                + self.rcpt_to_secs
                + self.data_secs
                + self.quit_secs,
        )
    }
}

/// Default timeout values.
mod defaults {
    // Server-side defaults (RFC 5321 compliant)
    pub const fn server_command_timeout_secs() -> u64 {
        300 // 5 minutes
    }
    pub const fn server_data_init_secs() -> u64 {
        120 // 2 minutes
    }
    pub const fn server_data_block_secs() -> u64 {
        180 // 3 minutes
    }
    pub const fn server_data_termination_secs() -> u64 {
        600 // 10 minutes
    }
    pub const fn server_connection_secs() -> u64 {
        1800 // 30 minutes
    }

    // Client-side defaults (optimized for delivery)
    pub const fn client_connect_secs() -> u64 {
        30
    }
    pub const fn client_ehlo_secs() -> u64 {
        30
    }
    pub const fn client_starttls_secs() -> u64 {
        30
    }
    pub const fn client_mail_from_secs() -> u64 {
        30
    }
    pub const fn client_rcpt_to_secs() -> u64 {
        30
    }
    pub const fn client_data_secs() -> u64 {
        120 // 2 minutes
    }
    pub const fn client_quit_secs() -> u64 {
        10
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_server_timeouts_defaults() {
        let timeouts = ServerTimeouts::default();
        assert_eq!(timeouts.command_secs, 300);
        assert_eq!(timeouts.data_init_secs, 120);
        assert_eq!(timeouts.data_block_secs, 180);
        assert_eq!(timeouts.data_termination_secs, 600);
        assert_eq!(timeouts.connection_secs, 1800);
    }

    #[test]
    fn test_client_timeouts_defaults() {
        let timeouts = ClientTimeouts::default();
        assert_eq!(timeouts.connect_secs, 30);
        assert_eq!(timeouts.ehlo_secs, 30);
        assert_eq!(timeouts.starttls_secs, 30);
        assert_eq!(timeouts.mail_from_secs, 30);
        assert_eq!(timeouts.rcpt_to_secs, 30);
        assert_eq!(timeouts.data_secs, 120);
        assert_eq!(timeouts.quit_secs, 10);
    }

    #[test]
    fn test_timeout_config_trait_server() {
        let timeouts = ServerTimeouts::default();
        assert_eq!(timeouts.command_timeout(), Duration::from_secs(300));
        assert_eq!(timeouts.data_timeout(), Duration::from_secs(120));
        assert_eq!(timeouts.connection_timeout(), Duration::from_secs(1800));
    }

    #[test]
    fn test_timeout_config_trait_client() {
        let timeouts = ClientTimeouts::default();
        assert_eq!(timeouts.command_timeout(), Duration::from_secs(30));
        assert_eq!(timeouts.data_timeout(), Duration::from_secs(120));
        // Connection timeout is sum of all operation timeouts
        assert_eq!(
            timeouts.connection_timeout(),
            Duration::from_secs(30 + 30 + 30 + 30 + 120 + 10)
        );
    }

    #[test]
    fn test_server_timeouts_clone() {
        let timeouts = ServerTimeouts::default();
        let cloned = timeouts.clone();
        assert_eq!(timeouts.command_secs, cloned.command_secs);
        assert_eq!(timeouts.connection_secs, cloned.connection_secs);
    }

    #[test]
    fn test_client_timeouts_clone() {
        let timeouts = ClientTimeouts::default();
        let cloned = timeouts.clone();
        assert_eq!(timeouts.connect_secs, cloned.connect_secs);
        assert_eq!(timeouts.data_secs, cloned.data_secs);
    }
}
