//! SMTP timeout configuration shared by outbound delivery.

use serde::{Deserialize, Serialize};

/// SMTP operation timeout configuration
///
/// Configures timeout durations for various SMTP operations to prevent
/// hung connections and ensure timely failure detection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmtpTimeouts {
    /// Timeout for initial connection establishment
    ///
    /// Default: 30 seconds
    #[serde(default = "default_connect_timeout")]
    pub connect_secs: u64,

    /// Timeout for EHLO/HELO commands
    ///
    /// Default: 30 seconds
    #[serde(default = "default_ehlo_timeout")]
    pub ehlo_secs: u64,

    /// Timeout for STARTTLS command and TLS upgrade
    ///
    /// Default: 30 seconds
    #[serde(default = "default_starttls_timeout")]
    pub starttls_secs: u64,

    /// Timeout for MAIL FROM command
    ///
    /// Default: 30 seconds
    #[serde(default = "default_mail_from_timeout")]
    pub mail_from_secs: u64,

    /// Timeout for RCPT TO command
    ///
    /// Default: 30 seconds
    #[serde(default = "default_rcpt_to_timeout")]
    pub rcpt_to_secs: u64,

    /// Timeout for DATA command and message transmission
    ///
    /// This is longer than other timeouts to accommodate large messages.
    /// Default: 120 seconds (2 minutes)
    #[serde(default = "default_data_timeout")]
    pub data_secs: u64,

    /// Timeout for QUIT command
    ///
    /// Default: 10 seconds
    #[serde(default = "default_quit_timeout")]
    pub quit_secs: u64,
}

impl Default for SmtpTimeouts {
    fn default() -> Self {
        Self {
            connect_secs: default_connect_timeout(),
            ehlo_secs: default_ehlo_timeout(),
            starttls_secs: default_starttls_timeout(),
            mail_from_secs: default_mail_from_timeout(),
            rcpt_to_secs: default_rcpt_to_timeout(),
            data_secs: default_data_timeout(),
            quit_secs: default_quit_timeout(),
        }
    }
}

const fn default_connect_timeout() -> u64 {
    30
}

const fn default_ehlo_timeout() -> u64 {
    30
}

const fn default_starttls_timeout() -> u64 {
    30
}

const fn default_mail_from_timeout() -> u64 {
    30
}

const fn default_rcpt_to_timeout() -> u64 {
    30
}

const fn default_data_timeout() -> u64 {
    120
}

const fn default_quit_timeout() -> u64 {
    10
}
