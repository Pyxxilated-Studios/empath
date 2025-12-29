//! Type definitions for delivery queue and processor

use std::sync::Arc;
use std::time::SystemTime;

use empath_common::context::DeliveryPriorityTier;
use empath_common::{DeliveryAttempt, DeliveryStatus, Domain};
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

/// SMTP client pool configuration
///
/// Controls connection reuse for outbound SMTP deliveries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmtpPoolConfig {
    /// Enable connection pooling for outbound SMTP clients.
    #[serde(default = "default_smtp_pool_enabled")]
    pub enabled: bool,
    /// Maximum idle connections to keep per domain key.
    #[serde(default = "default_smtp_pool_max_per_domain")]
    pub max_per_domain: usize,
    /// Idle time-to-live before pooled connections are closed.
    #[serde(default = "default_smtp_pool_idle_ttl")]
    pub idle_ttl_secs: u64,
    /// Run a NOOP health check before reusing a pooled connection.
    #[serde(default = "default_smtp_pool_health_check")]
    pub health_check_on_acquire: bool,
    /// How often to sweep idle connections from the pool.
    #[serde(default = "default_smtp_pool_cleanup_interval")]
    pub cleanup_interval_secs: u64,
}

impl Default for SmtpPoolConfig {
    fn default() -> Self {
        Self {
            enabled: default_smtp_pool_enabled(),
            max_per_domain: default_smtp_pool_max_per_domain(),
            idle_ttl_secs: default_smtp_pool_idle_ttl(),
            health_check_on_acquire: default_smtp_pool_health_check(),
            cleanup_interval_secs: default_smtp_pool_cleanup_interval(),
        }
    }
}

impl SmtpPoolConfig {
    /// Create a builder for ergonomic SMTP pool configuration.
    #[must_use]
    pub fn builder() -> SmtpPoolConfigBuilder {
        SmtpPoolConfigBuilder::default()
    }
}

#[derive(Debug, Default)]
pub struct SmtpPoolConfigBuilder {
    config: SmtpPoolConfig,
}

impl SmtpPoolConfigBuilder {
    #[must_use]
    pub const fn enabled(mut self, enabled: bool) -> Self {
        self.config.enabled = enabled;
        self
    }

    #[must_use]
    pub const fn max_per_domain(mut self, max_per_domain: usize) -> Self {
        self.config.max_per_domain = max_per_domain;
        self
    }

    #[must_use]
    pub const fn idle_ttl_secs(mut self, idle_ttl_secs: u64) -> Self {
        self.config.idle_ttl_secs = idle_ttl_secs;
        self
    }

    #[must_use]
    pub const fn health_check_on_acquire(mut self, health_check_on_acquire: bool) -> Self {
        self.config.health_check_on_acquire = health_check_on_acquire;
        self
    }

    #[must_use]
    pub const fn cleanup_interval_secs(mut self, cleanup_interval_secs: u64) -> Self {
        self.config.cleanup_interval_secs = cleanup_interval_secs;
        self
    }

    #[must_use]
    pub const fn build(self) -> SmtpPoolConfig {
        self.config
    }
}

const fn default_smtp_pool_enabled() -> bool {
    true
}

const fn default_smtp_pool_max_per_domain() -> usize {
    4
}

const fn default_smtp_pool_idle_ttl() -> u64 {
    60
}

const fn default_smtp_pool_health_check() -> bool {
    true
}

const fn default_smtp_pool_cleanup_interval() -> u64 {
    30
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

/// Snapshot of connection/rate-limit stats for control endpoints
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ConnectionStats {
    /// Messages currently in the delivery queue
    pub queue_size: usize,
    /// Number of active rate limiter buckets
    pub rate_limit_domains: usize,
}

/// Information about a message in the delivery queue
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeliveryInfo {
    /// The spooled message identifier
    pub message_id: SpooledMessageId,
    /// Current delivery status
    pub status: DeliveryStatus,
    /// List of delivery attempts
    pub attempts: Vec<DeliveryAttempt>,
    /// Recipient domain for this delivery
    pub recipient_domain: Domain,
    /// Resolved mail servers (sorted by priority, Arc for cheap cloning)
    pub mail_servers: Arc<Vec<MailServer>>,
    /// Index of the current mail server being tried
    pub current_server_index: usize,
    /// Delivery priority tier for scheduling.
    pub priority_tier: DeliveryPriorityTier,
    /// Selected outbound binding name (if any)
    pub binding: Option<String>,
    /// When this message was first queued for delivery
    pub queued_at: SystemTime,
    /// When the next retry should be attempted (None for immediate retry)
    pub next_retry_at: Option<SystemTime>,
}

impl DeliveryInfo {
    /// Create a new pending delivery info
    #[must_use]
    pub fn new(message_id: SpooledMessageId, recipient_domain: String) -> Self {
        Self {
            message_id,
            status: DeliveryStatus::Pending,
            attempts: Vec::new(),
            recipient_domain: Domain::new(recipient_domain),
            mail_servers: Arc::new(Vec::new()),
            current_server_index: 0,
            priority_tier: DeliveryPriorityTier::default(),
            binding: None,
            queued_at: SystemTime::now(),
            next_retry_at: None,
        }
    }

    /// Create a new pending delivery info with a specific priority tier.
    #[must_use]
    pub fn new_with_priority(
        message_id: SpooledMessageId,
        recipient_domain: String,
        priority_tier: DeliveryPriorityTier,
    ) -> Self {
        Self {
            priority_tier,
            ..Self::new(message_id, recipient_domain)
        }
    }

    /// Create a builder for ergonomic delivery info setup.
    #[must_use]
    pub fn builder(message_id: SpooledMessageId, recipient_domain: String) -> DeliveryInfoBuilder {
        DeliveryInfoBuilder {
            info: Self::new(message_id, recipient_domain),
        }
    }

    /// Record a delivery attempt
    pub fn record_attempt(&mut self, attempt: DeliveryAttempt) {
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

#[derive(Debug)]
pub struct DeliveryInfoBuilder {
    info: DeliveryInfo,
}

impl DeliveryInfoBuilder {
    #[must_use]
    pub fn with_status(mut self, status: DeliveryStatus) -> Self {
        self.info.status = status;
        self
    }

    #[must_use]
    pub fn with_attempts(mut self, attempts: Vec<DeliveryAttempt>) -> Self {
        self.info.attempts = attempts;
        self
    }

    #[must_use]
    pub fn with_mail_servers(mut self, mail_servers: Arc<Vec<MailServer>>) -> Self {
        self.info.mail_servers = mail_servers;
        self
    }

    #[must_use]
    pub const fn with_current_server_index(mut self, current_server_index: usize) -> Self {
        self.info.current_server_index = current_server_index;
        self
    }

    #[must_use]
    pub const fn with_priority_tier(mut self, priority_tier: DeliveryPriorityTier) -> Self {
        self.info.priority_tier = priority_tier;
        self
    }

    #[must_use]
    pub fn with_binding(mut self, binding: Option<String>) -> Self {
        self.info.binding = binding;
        self
    }

    #[must_use]
    pub const fn with_queued_at(mut self, queued_at: SystemTime) -> Self {
        self.info.queued_at = queued_at;
        self
    }

    #[must_use]
    pub const fn with_next_retry_at(mut self, next_retry_at: Option<SystemTime>) -> Self {
        self.info.next_retry_at = next_retry_at;
        self
    }

    #[must_use]
    pub fn build(self) -> DeliveryInfo {
        self.info
    }
}
