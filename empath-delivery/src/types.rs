//! Type definitions for delivery queue and processor

use std::sync::Arc;

use empath_common::DeliveryStatus;
use empath_spool::SpooledMessageId;
use serde::{Deserialize, Serialize};

use crate::dns::MailServer;

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

/// Information about a message in the delivery queue
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeliveryInfo {
    /// The spooled message identifier
    pub message_id: SpooledMessageId,
    /// Current delivery status
    pub status: DeliveryStatus,
    /// List of delivery attempts
    pub attempts: Vec<empath_common::DeliveryAttempt>,
    /// Recipient domain for this delivery (Arc for cheap cloning)
    pub recipient_domain: Arc<str>,
    /// Resolved mail servers (sorted by priority, Arc for cheap cloning)
    pub mail_servers: Arc<Vec<MailServer>>,
    /// Index of the current mail server being tried
    pub current_server_index: usize,
    /// Unix timestamp when this message was first queued
    pub queued_at: u64,
    /// Unix timestamp when the next retry should be attempted (None for immediate retry)
    pub next_retry_at: Option<u64>,
}

impl DeliveryInfo {
    /// Create a new pending delivery info
    #[must_use]
    pub fn new(message_id: SpooledMessageId, recipient_domain: String) -> Self {
        let queued_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        Self {
            message_id,
            status: DeliveryStatus::Pending,
            attempts: Vec::new(),
            recipient_domain: Arc::from(recipient_domain),
            mail_servers: Arc::new(Vec::new()),
            current_server_index: 0,
            queued_at,
            next_retry_at: None,
        }
    }

    /// Record a delivery attempt
    pub fn record_attempt(&mut self, attempt: empath_common::DeliveryAttempt) {
        self.attempts.push(attempt);
    }

    /// Get the number of attempts made
    pub fn attempt_count(&self) -> u32 {
        u32::try_from(self.attempts.len()).unwrap_or(u32::MAX)
    }

    /// Try the next MX server in the priority list.
    ///
    /// Returns `true` if there is another server to try, `false` if all servers exhausted.
    #[must_use]
    pub fn try_next_server(&mut self) -> bool {
        if self.current_server_index + 1 < self.mail_servers.len() {
            self.current_server_index += 1;
            true
        } else {
            false
        }
    }

    /// Reset to the first MX server (for new delivery cycle).
    pub const fn reset_server_index(&mut self) {
        self.current_server_index = 0;
    }

    /// Get the current mail server being tried.
    #[must_use]
    pub fn current_mail_server(&self) -> Option<&MailServer> {
        self.mail_servers.get(self.current_server_index)
    }
}
