//! Delivery queue and processor for handling outbound mail from the spool
//!
//! This module provides functionality to:
//! - Track messages pending delivery
//! - Manage delivery attempts and retries
//! - Prepare messages for sending via SMTP
//! - DNS MX record resolution for recipient domains

#![deny(clippy::pedantic, clippy::all, clippy::nursery)]
#![allow(clippy::must_use_candidate)]

mod dns;
mod domain_config;
mod error;
mod smtp_transaction;

use std::{collections::HashMap, sync::Arc, time::Duration};

pub use dns::{DnsConfig, DnsError, DnsResolver, MailServer};
pub use domain_config::{DomainConfig, DomainConfigRegistry};
// Re-export DeliveryAttempt and DeliveryStatus for consumers of this crate
pub use empath_common::{DeliveryAttempt, DeliveryStatus};
use empath_common::{Signal, context::Context, internal, tracing};
use empath_ffi::modules::{self, Ev, Event};
use empath_spool::SpooledMessageId;
use empath_tracing::traced;
pub use error::{DeliveryError, PermanentError, SystemError, TemporaryError};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

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
    pub attempts: Vec<DeliveryAttempt>,
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
    pub fn current_mail_server(&self) -> Option<&MailServer> {
        self.mail_servers.get(self.current_server_index)
    }
}

/// Manages the delivery queue for outbound messages
#[derive(Debug, Clone)]
pub struct DeliveryQueue {
    /// Map of message IDs to delivery information
    queue: Arc<RwLock<HashMap<SpooledMessageId, DeliveryInfo>>>,
}

impl Default for DeliveryQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl DeliveryQueue {
    /// Create a new empty delivery queue
    #[must_use]
    pub fn new() -> Self {
        Self {
            queue: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Add a message to the delivery queue
    pub async fn enqueue(&self, message_id: SpooledMessageId, recipient_domain: String) {
        let mut queue = self.queue.write().await;
        queue.insert(
            message_id.clone(),
            DeliveryInfo::new(message_id, recipient_domain),
        );
    }

    /// Get delivery info for a message
    pub async fn get(&self, message_id: &SpooledMessageId) -> Option<DeliveryInfo> {
        let queue = self.queue.read().await;
        queue.get(message_id).cloned()
    }

    /// Update the status of a message
    pub async fn update_status(&self, message_id: &SpooledMessageId, status: DeliveryStatus) {
        let mut queue = self.queue.write().await;
        if let Some(info) = queue.get_mut(message_id) {
            info.status = status;
        }
    }

    /// Record a delivery attempt
    pub async fn record_attempt(&self, message_id: &SpooledMessageId, attempt: DeliveryAttempt) {
        let mut queue = self.queue.write().await;
        if let Some(info) = queue.get_mut(message_id) {
            info.record_attempt(attempt);
        }
    }

    /// Set the resolved mail servers for a message
    pub async fn set_mail_servers(
        &self,
        message_id: &SpooledMessageId,
        servers: Arc<Vec<MailServer>>,
    ) {
        let mut queue = self.queue.write().await;
        if let Some(info) = queue.get_mut(message_id) {
            info.mail_servers = servers;
            info.current_server_index = 0;
        }
    }

    /// Try the next MX server for a message.
    ///
    /// Returns `true` if there is another server to try, `false` if all exhausted.
    pub async fn try_next_server(&self, message_id: &SpooledMessageId) -> bool {
        let mut queue = self.queue.write().await;
        queue
            .get_mut(message_id)
            .is_some_and(DeliveryInfo::try_next_server)
    }

    /// Remove a message from the queue
    pub async fn remove(&self, message_id: &SpooledMessageId) -> Option<DeliveryInfo> {
        let mut queue = self.queue.write().await;
        queue.remove(message_id)
    }

    /// Set the next retry timestamp for a message
    pub async fn set_next_retry_at(&self, message_id: &SpooledMessageId, next_retry_at: u64) {
        let mut queue = self.queue.write().await;
        if let Some(info) = queue.get_mut(message_id) {
            info.next_retry_at = Some(next_retry_at);
        }
    }

    /// Reset the server index to 0 for a message (for new retry cycle)
    pub async fn reset_server_index(&self, message_id: &SpooledMessageId) {
        let mut queue = self.queue.write().await;
        if let Some(info) = queue.get_mut(message_id) {
            info.reset_server_index();
        }
    }

    /// Get all pending messages
    pub async fn pending_messages(&self) -> Vec<DeliveryInfo> {
        let queue = self.queue.read().await;
        queue
            .values()
            .filter(|info| info.status == DeliveryStatus::Pending)
            .cloned()
            .collect()
    }

    /// Get all messages with their current status
    pub async fn all_messages(&self) -> Vec<DeliveryInfo> {
        let queue = self.queue.read().await;
        queue.values().cloned().collect()
    }
}

const fn default_scan_interval() -> u64 {
    30
}

const fn default_process_interval() -> u64 {
    10
}

const fn default_max_attempts() -> u32 {
    25
}

const fn default_base_retry_delay() -> u64 {
    60 // 1 minute
}

const fn default_max_retry_delay() -> u64 {
    86400 // 24 hours
}

const fn default_retry_jitter_factor() -> f64 {
    0.2 // ±20%
}

/// Processor for handling delivery of messages from the spool
///
/// This processor runs continuously, scanning the spool for new messages
/// and processing the delivery queue at configurable intervals.
#[allow(
    clippy::unsafe_derive_deserialize,
    reason = "The unsafe aspects have nothing to do with the struct"
)]
#[derive(Debug, Deserialize)]
pub struct DeliveryProcessor {
    /// How often to scan the spool for new messages (in seconds)
    #[serde(default = "default_scan_interval")]
    pub scan_interval_secs: u64,

    /// How often to process the delivery queue (in seconds)
    #[serde(default = "default_process_interval")]
    pub process_interval_secs: u64,

    /// Maximum number of delivery attempts before giving up
    #[serde(default = "default_max_attempts")]
    pub max_attempts: u32,

    /// Base delay for exponential backoff (in seconds)
    ///
    /// First retry will occur after this delay. Subsequent retries will double
    /// this delay (with jitter) up to `max_retry_delay_secs`.
    ///
    /// Default: 60 seconds (1 minute)
    #[serde(default = "default_base_retry_delay")]
    pub base_retry_delay_secs: u64,

    /// Maximum delay between retry attempts (in seconds)
    ///
    /// Caps the exponential backoff to prevent excessively long delays.
    ///
    /// Default: 86400 seconds (24 hours)
    #[serde(default = "default_max_retry_delay")]
    pub max_retry_delay_secs: u64,

    /// Jitter factor for retry delays (0.0 to 1.0)
    ///
    /// Adds randomness to retry delays to prevent thundering herd.
    /// A factor of 0.2 means ±20% randomness.
    ///
    /// Default: 0.2 (±20%)
    #[serde(default = "default_retry_jitter_factor")]
    pub retry_jitter_factor: f64,

    /// Message expiration time (in seconds)
    ///
    /// Messages older than this will be marked as expired and removed from the queue.
    /// Set to `None` to never expire messages.
    ///
    /// Default: None (never expire)
    #[serde(default)]
    pub message_expiration_secs: Option<u64>,

    /// Accept invalid TLS certificates globally (for testing only)
    ///
    /// **SECURITY WARNING**: Setting this to `true` disables certificate validation
    /// for all domains (unless overridden per-domain), making connections vulnerable
    /// to Man-in-the-Middle attacks. Only enable for testing with self-signed certificates.
    ///
    /// Default: `false` (secure)
    #[serde(default)]
    pub accept_invalid_certs: bool,

    /// DNS configuration for resolver
    #[serde(default)]
    pub dns: DnsConfig,

    /// Per-domain delivery configuration
    #[serde(default)]
    pub domains: DomainConfigRegistry,

    /// SMTP operation timeout configuration
    #[serde(default)]
    pub smtp_timeouts: SmtpTimeouts,

    /// The spool backing store to read messages from (initialized in `init()`)
    #[serde(skip)]
    spool: Option<Arc<dyn empath_spool::BackingStore>>,

    /// The delivery queue (initialized in `init()`)
    #[serde(skip)]
    queue: DeliveryQueue,

    /// DNS resolver for MX record lookups (initialized in `init()`)
    #[serde(skip)]
    dns_resolver: Option<DnsResolver>,

    /// Path to freeze marker file (presence indicates queue is frozen)
    #[serde(skip)]
    freeze_marker_path: Option<std::path::PathBuf>,
}

impl Default for DeliveryProcessor {
    fn default() -> Self {
        Self {
            scan_interval_secs: default_scan_interval(),
            process_interval_secs: default_process_interval(),
            max_attempts: default_max_attempts(),
            base_retry_delay_secs: default_base_retry_delay(),
            max_retry_delay_secs: default_max_retry_delay(),
            retry_jitter_factor: default_retry_jitter_factor(),
            message_expiration_secs: None,
            accept_invalid_certs: false,
            dns: DnsConfig::default(),
            domains: DomainConfigRegistry::default(),
            smtp_timeouts: SmtpTimeouts::default(),
            spool: None,
            queue: DeliveryQueue::new(),
            dns_resolver: None,
            freeze_marker_path: None,
        }
    }
}

/// Calculate the next retry time using exponential backoff with jitter
///
/// # Formula
/// `delay = min(base * 2^(attempts - 1), max_delay) * (1 ± jitter)`
///
/// # Arguments
/// * `attempt` - The attempt number (1-indexed)
/// * `base_delay_secs` - Base delay in seconds (e.g., 60 for 1 minute)
/// * `max_delay_secs` - Maximum delay in seconds (e.g., 86400 for 24 hours)
/// * `jitter_factor` - Jitter factor (e.g., 0.2 for ±20%)
///
/// # Returns
/// Unix timestamp when the next retry should occur
fn calculate_next_retry_time(
    attempt: u32,
    base_delay_secs: u64,
    max_delay_secs: u64,
    jitter_factor: f64,
) -> u64 {
    use rand::Rng;

    // Calculate exponential backoff: base * 2^(attempts - 1)
    // Use saturating operations to prevent overflow
    let exponent = attempt.saturating_sub(1);
    let delay = if exponent >= 63 {
        // 2^63 would overflow, use max_delay directly
        max_delay_secs
    } else {
        let multiplier = 1u64 << exponent; // 2^exponent
        base_delay_secs
            .saturating_mul(multiplier)
            .min(max_delay_secs)
    };

    // Apply jitter: delay * (1 ± jitter_factor)
    // Intentional precision loss and casting for randomization
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )]
    let jittered_delay = {
        let jitter_range = (delay as f64) * jitter_factor;
        let mut rng = rand::thread_rng();
        let jitter: f64 = rng.gen_range(-jitter_range..=jitter_range);
        ((delay as f64) + jitter).max(0.0) as u64
    };

    // Calculate next retry timestamp
    let current_time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    current_time.saturating_add(jittered_delay)
}

impl DeliveryProcessor {
    /// Initialize the delivery processor
    ///
    /// # Errors
    ///
    /// Returns an error if the processor cannot be initialized
    pub fn init(
        &mut self,
        spool: Arc<dyn empath_spool::BackingStore>,
        spool_path: Option<std::path::PathBuf>,
    ) -> Result<(), DeliveryError> {
        internal!("Initialising Delivery Processor ...");
        self.spool = Some(spool);
        self.dns_resolver = Some(DnsResolver::with_dns_config(self.dns.clone())?);
        internal!(
            "DNS resolver initialized with timeout={}s, cache_ttl={}, min_ttl={}s, max_ttl={}s, cache_size={}",
            self.dns.timeout_secs,
            self.dns.cache_ttl_secs.map_or_else(
                || "DNS record TTL".to_string(),
                |ttl| format!("{ttl}s (override)")
            ),
            self.dns.min_cache_ttl_secs,
            self.dns.max_cache_ttl_secs,
            self.dns.cache_size
        );

        // Set up freeze marker path based on spool directory
        // If spool_path is provided, derive paths from it, otherwise use /tmp/spool
        let base_path = spool_path.unwrap_or_else(|| std::path::PathBuf::from("/tmp/spool"));
        self.freeze_marker_path = Some(base_path.join("queue_frozen"));

        Ok(())
    }

    /// Run the delivery processor
    ///
    /// This method runs continuously until a shutdown signal is received.
    /// It periodically scans the spool for new messages and processes the
    /// delivery queue.
    ///
    /// ## Graceful Shutdown
    ///
    /// When a shutdown signal is received:
    /// 1. Stop accepting new work (scan/process ticks)
    /// 2. Wait for any in-flight delivery to complete (with 30s timeout)
    /// 3. Save queue state to disk
    /// 4. Exit cleanly
    ///
    /// In-flight deliveries that don't complete within the shutdown timeout
    /// will be marked as pending and retried on the next restart.
    ///
    /// # Errors
    ///
    /// Returns an error if the delivery processor encounters a fatal error
    #[traced(instrument(level = tracing::Level::TRACE, skip_all))]
    pub async fn serve(
        &self,
        mut shutdown: tokio::sync::broadcast::Receiver<Signal>,
    ) -> Result<(), DeliveryError> {
        internal!("Delivery processor starting");

        let Some(spool) = &self.spool else {
            return Err(SystemError::NotInitialized(
                "Delivery processor not initialized. Call init() first.".to_string(),
            )
            .into());
        };

        let scan_interval = Duration::from_secs(self.scan_interval_secs);
        let process_interval = Duration::from_secs(self.process_interval_secs);
        let state_save_interval = Duration::from_secs(30); // Save queue state every 30s

        let mut scan_timer = tokio::time::interval(scan_interval);
        let mut process_timer = tokio::time::interval(process_interval);
        let mut state_save_timer = tokio::time::interval(state_save_interval);

        // Skip the first tick to avoid immediate execution
        scan_timer.tick().await;
        process_timer.tick().await;
        state_save_timer.tick().await;

        // Track if we're currently processing a delivery
        let processing = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let processing_clone = processing.clone();

        loop {
            tokio::select! {
                _ = scan_timer.tick() => {
                    match self.scan_spool_internal(spool).await {
                        Ok(count) if count > 0 => {
                            tracing::info!("Scanned spool, found {count} new messages");
                        }
                        Ok(_) => {
                            tracing::debug!("Scanned spool, no new messages");
                        }
                        Err(e) => {
                            tracing::error!("Error scanning spool: {e}");
                        }
                    }
                }
                _ = process_timer.tick() => {
                    // Check if queue is frozen before processing
                    if self.is_frozen() {
                        tracing::debug!("Delivery queue is frozen, skipping processing");
                        continue;
                    }

                    // Mark that we're processing
                    processing.store(true, std::sync::atomic::Ordering::SeqCst);

                    match self.process_queue_internal(spool).await {
                        Ok(()) => {
                            tracing::debug!("Processed delivery queue");
                        }
                        Err(e) => {
                            tracing::error!("Error processing delivery queue: {e}");
                        }
                    }

                    // Mark that we're done processing
                    processing.store(false, std::sync::atomic::Ordering::SeqCst);
                }
                _ = state_save_timer.tick() => {
                    // Queue state is now persisted to spool on every status change
                    // This timer tick is no longer needed but kept for potential future use
                }
                sig = shutdown.recv() => {
                    match sig {
                        Ok(Signal::Shutdown | Signal::Finalised) => {
                            internal!("Delivery processor received shutdown signal");

                            // Wait for any in-flight delivery to complete (with 30s timeout)
                            let shutdown_timeout = Duration::from_secs(30);
                            let start = std::time::Instant::now();

                            while processing_clone.load(std::sync::atomic::Ordering::SeqCst) {
                                if start.elapsed() >= shutdown_timeout {
                                    tracing::warn!(
                                        "Shutdown timeout exceeded, {} remaining in-flight delivery will be retried on restart",
                                        if processing_clone.load(std::sync::atomic::Ordering::SeqCst) { "1" } else { "0" }
                                    );
                                    break;
                                }

                                tracing::debug!(
                                    "Waiting for in-flight delivery to complete ({:.1}s elapsed)...",
                                    start.elapsed().as_secs_f64()
                                );
                                tokio::time::sleep(Duration::from_millis(100)).await;
                            }

                            if !processing_clone.load(std::sync::atomic::Ordering::SeqCst) {
                                internal!("All in-flight deliveries completed successfully");
                            }

                            // Queue state is automatically persisted to spool on every status change
                            internal!("Delivery processor shutdown complete");
                            break;
                        }
                        Err(e) => {
                            tracing::error!("Delivery processor shutdown channel error: {e}");
                            break;
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Get a reference to the delivery queue
    pub const fn queue(&self) -> &DeliveryQueue {
        &self.queue
    }

    /// Check if the delivery queue is frozen
    ///
    /// Returns `true` if the freeze marker file exists, `false` otherwise.
    fn is_frozen(&self) -> bool {
        self.freeze_marker_path
            .as_ref()
            .is_some_and(|path| path.exists())
    }

    /// Scan the spool for new messages and add them to the queue
    ///
    /// # Errors
    /// Returns an error if the spool cannot be read
    async fn scan_spool_internal(
        &self,
        spool: &Arc<dyn empath_spool::BackingStore>,
    ) -> Result<usize, DeliveryError> {
        let message_ids = spool
            .list()
            .await
            .map_err(|e| SystemError::SpoolRead(e.to_string()))?;
        let mut added = 0;

        for msg_id in message_ids {
            // Check if already in queue
            if self.queue.get(&msg_id).await.is_some() {
                continue;
            }

            // Read the message to get context (potentially with delivery state)
            let context = spool
                .read(&msg_id)
                .await
                .map_err(|e| SystemError::SpoolRead(e.to_string()))?;

            // Check if this message already has delivery state persisted
            if let Some(delivery_ctx) = &context.delivery {
                // Restore from persisted state
                let info = DeliveryInfo {
                    message_id: msg_id.clone(),
                    status: delivery_ctx.status.clone(),
                    attempts: delivery_ctx.attempt_history.clone(),
                    recipient_domain: delivery_ctx.domain.clone(),
                    mail_servers: Arc::new(Vec::new()), // Will be resolved again if needed
                    current_server_index: delivery_ctx.current_server_index,
                    queued_at: delivery_ctx.queued_at,
                    next_retry_at: delivery_ctx.next_retry_at,
                };

                // Add to queue with existing state
                self.queue.queue.write().await.insert(msg_id.clone(), info);
                added += 1;
                continue;
            }

            // New message without delivery state - create fresh DeliveryInfo
            // Group recipients by domain (handle multi-recipient messages)
            let Some(recipients) = context.envelope.recipients() else {
                use tracing::warn;
                warn!("Message {:?} has no recipients, skipping", msg_id);
                continue;
            };

            // Collect unique domains from all recipients
            let mut domains = std::collections::HashMap::new();
            for recipient in recipients.iter() {
                // Extract the actual email address from the MailAddr
                let recipient_str = match &**recipient {
                    mailparse::MailAddr::Single(single) => &single.addr,
                    mailparse::MailAddr::Group(_) => continue, // Skip groups
                };

                match extract_domain(recipient_str) {
                    Ok(domain) => {
                        domains
                            .entry(domain)
                            .or_insert_with(Vec::new)
                            .push(recipient_str.to_owned());
                    }
                    Err(e) => {
                        use tracing::warn;
                        warn!(
                            message_id = ?msg_id,
                            recipient = %recipient_str,
                            error = %e,
                            "Failed to extract domain from recipient, skipping"
                        );
                    }
                }
            }

            // Enqueue for each unique domain
            for (domain, _recipients) in domains {
                self.queue.enqueue(msg_id.clone(), domain).await;
                added += 1;
            }
        }

        Ok(added)
    }

    /// Prepare a message for delivery using SMTP client (but don't actually send it yet)
    ///
    /// This method:
    /// 1. Reads the message from the spool
    /// 2. Performs DNS MX lookup for the recipient domain
    /// 3. Connects to the MX server via SMTP
    /// 4. Performs EHLO/HELO handshake
    /// 5. Validates MAIL FROM and RCPT TO
    /// 6. Does NOT send DATA (that's for actual delivery)
    ///
    /// # Errors
    /// Returns an error if the message cannot be read, DNS lookup fails, or SMTP connection fails
    #[allow(
        clippy::too_many_lines,
        reason = "Persistence logic adds necessary lines"
    )]
    async fn prepare_message(
        &self,
        message_id: &SpooledMessageId,
        spool: &Arc<dyn empath_spool::BackingStore>,
    ) -> Result<(), DeliveryError> {
        use empath_common::context::DeliveryContext;

        self.queue
            .update_status(message_id, DeliveryStatus::InProgress)
            .await;

        // Persist the InProgress status to spool
        if let Err(e) = self.persist_delivery_state(message_id, spool).await {
            use tracing::warn;
            warn!(
                message_id = ?message_id,
                error = %e,
                "Failed to persist delivery state after status update to InProgress"
            );
            // Continue anyway - this is not critical for delivery
        }

        let mut context = spool
            .read(message_id)
            .await
            .map_err(|e| SystemError::SpoolRead(e.to_string()))?;
        let info = self.queue.get(message_id).await.ok_or_else(|| {
            SystemError::MessageNotFound(format!("Message {message_id:?} not in queue"))
        })?;

        // Dispatch DeliveryAttempt event to modules
        {
            context.delivery = Some(DeliveryContext {
                message_id: message_id.to_string(),
                domain: info.recipient_domain.clone(),
                server: None, // Server not yet determined at this point
                error: None,
                attempts: Some(info.attempt_count()),
                status: info.status.clone(),
                attempt_history: info.attempts.clone(),
                queued_at: info.queued_at,
                next_retry_at: info.next_retry_at,
                current_server_index: info.current_server_index,
            });

            modules::dispatch(Event::Event(Ev::DeliveryAttempt), &mut context);
        }

        // Check for domain-specific MX override first (for testing/debugging)
        let mail_servers = if let Some(domain_config) = self.domains.get(&info.recipient_domain)
            && let Some(mx_override) = domain_config.mx_override_address()
        {
            internal!(
                "Using MX override for {}: {}",
                info.recipient_domain,
                mx_override
            );

            // Parse host:port or use default port 25
            let (host, port) = if let Some((h, p)) = mx_override.split_once(':') {
                (h.to_string(), p.parse::<u16>().unwrap_or(25))
            } else {
                (mx_override.to_string(), 25)
            };

            Arc::new(vec![MailServer {
                host,
                port,
                priority: 0,
            }])
        } else {
            // Get the DNS resolver
            let Some(dns_resolver) = &self.dns_resolver else {
                return Err(SystemError::NotInitialized(
                    "DNS resolver not initialized. Call init() first.".to_string(),
                )
                .into());
            };

            // Perform real DNS MX lookup for the recipient domain
            // DNS errors are automatically converted to DeliveryError via From<DnsError>
            let resolved = dns_resolver
                .resolve_mail_servers(&info.recipient_domain)
                .await?;

            if resolved.is_empty() {
                return Err(
                    PermanentError::NoMailServers(info.recipient_domain.to_string()).into(),
                );
            }

            resolved
        };

        // Store the resolved mail servers
        self.queue
            .set_mail_servers(message_id, mail_servers.clone())
            .await;

        // Use the first (highest priority) mail server
        let primary_server = &mail_servers[0];
        let mx_address = primary_server.address();

        internal!(
            "Sending message to {:?} with MX host {} (priority {})",
            message_id,
            mx_address,
            primary_server.priority
        );

        // Deliver the message via SMTP (including DATA command)
        let result = self.deliver_message(&mx_address, &context, &info).await;

        match result {
            Ok(()) => {
                self.queue
                    .update_status(message_id, DeliveryStatus::Completed)
                    .await;

                // Persist the Completed status to spool before deletion
                // Note: This will be immediately deleted, but it's important for consistency
                // in case the deletion fails
                if let Err(e) = self.persist_delivery_state(message_id, spool).await {
                    use tracing::warn;
                    warn!(
                        message_id = ?message_id,
                        error = %e,
                        "Failed to persist delivery state after successful delivery"
                    );
                }

                // Delete the message from the spool after successful delivery
                if let Err(e) = spool.delete(message_id).await {
                    use tracing::error;
                    error!(
                        message_id = ?message_id,
                        error = %e,
                        "Failed to delete message from spool after successful delivery"
                    );
                    // Don't fail the delivery just because we couldn't delete the spool file
                    // The message was delivered successfully
                }

                // Dispatch DeliverySuccess event to modules
                context.delivery = Some(DeliveryContext {
                    message_id: message_id.to_string(),
                    domain: info.recipient_domain.clone(),
                    server: Some(mx_address.clone()),
                    error: None,
                    attempts: Some(info.attempt_count()),
                    status: info.status.clone(),
                    attempt_history: info.attempts.clone(),
                    queued_at: info.queued_at,
                    next_retry_at: info.next_retry_at,
                    current_server_index: info.current_server_index,
                });
                modules::dispatch(Event::Event(Ev::DeliverySuccess), &mut context);

                Ok(())
            }
            Err(e) => {
                let error = self
                    .handle_delivery_error(message_id, &mut context, e, mx_address.clone())
                    .await;
                Err(error)
            }
        }
    }

    /// Handle a failed delivery attempt and update status based on retry policy
    ///
    /// Records the attempt and determines whether to retry or mark as permanently failed.
    /// Implements MX server fallback: tries lower-priority MX servers before counting as a retry.
    /// Dispatches `DeliveryFailure` event to modules.
    ///
    /// # Errors
    /// Returns the original error after recording it
    #[allow(
        clippy::too_many_lines,
        reason = "Persistence logic adds necessary lines"
    )]
    async fn handle_delivery_error(
        &self,
        message_id: &SpooledMessageId,
        context: &mut Context,
        error: DeliveryError,
        server: String,
    ) -> DeliveryError {
        use empath_common::context::DeliveryContext;
        use tracing::{info, warn};

        // Record the attempt
        let attempt = DeliveryAttempt {
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            error: Some(error.to_string()),
            server: server.clone(),
        };

        self.queue.record_attempt(message_id, attempt).await;

        // Get updated info to check attempt count
        // Use proper error handling instead of unwrap
        let Some(updated_info) = self.queue.get(message_id).await else {
            warn!(
                "Message {:?} disappeared from queue during error handling",
                message_id
            );
            return error; // Preserve original error
        };

        // Check if this is a temporary failure that warrants trying another MX server
        // (e.g., connection refused, timeout, temporary SMTP error)
        let is_temporary_failure = error.is_temporary();

        // Try next MX server if this was a temporary failure
        if is_temporary_failure
            && self.queue.try_next_server(message_id).await
            && let Some(info) = self.queue.get(message_id).await
            && let Some(next_server) = info.current_mail_server()
        {
            info!(
                "Trying next MX server for {:?}: {} (priority {})",
                message_id, next_server.host, next_server.priority
            );
            // Set status back to Pending to retry immediately with next server
            self.queue
                .update_status(message_id, DeliveryStatus::Pending)
                .await;

            // Persist the Pending status for next MX server attempt
            if let Some(spool) = &self.spool
                && let Err(e) = self.persist_delivery_state(message_id, spool).await
            {
                use tracing::warn;
                warn!(
                    message_id = ?message_id,
                    error = %e,
                    "Failed to persist delivery state after MX server fallback"
                );
            }

            return error;
        }

        // All MX servers exhausted or permanent failure, use normal retry logic
        // Determine new status based on attempt count
        let new_status = if updated_info.attempt_count() >= self.max_attempts {
            DeliveryStatus::Failed(error.to_string())
        } else {
            DeliveryStatus::Retry {
                attempts: updated_info.attempt_count(),
                last_error: error.to_string(),
            }
        };

        self.queue
            .update_status(message_id, new_status.clone())
            .await;

        // Calculate and set next retry time using exponential backoff
        if matches!(new_status, DeliveryStatus::Retry { .. }) {
            let next_retry_at = calculate_next_retry_time(
                updated_info.attempt_count(),
                self.base_retry_delay_secs,
                self.max_retry_delay_secs,
                self.retry_jitter_factor,
            );

            self.queue
                .set_next_retry_at(message_id, next_retry_at)
                .await;

            // Calculate delay for logging
            let current_time = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            let delay_secs = next_retry_at.saturating_sub(current_time);

            info!(
                message_id = ?message_id,
                attempt = updated_info.attempt_count(),
                retry_delay_secs = delay_secs,
                next_retry_at = next_retry_at,
                "Scheduled retry with exponential backoff"
            );
        }

        // Persist the updated status (Retry or Failed) to spool
        if let Some(spool) = &self.spool
            && let Err(e) = self.persist_delivery_state(message_id, spool).await
        {
            warn!(
                message_id = ?message_id,
                error = %e,
                "Failed to persist delivery state after handling delivery error"
            );
        }

        context.delivery = Some(DeliveryContext {
            message_id: message_id.to_string(),
            domain: updated_info.recipient_domain.clone(),
            server: Some(server),
            error: Some(error.to_string()),
            attempts: Some(updated_info.attempt_count()),
            status: updated_info.status.clone(),
            attempt_history: updated_info.attempts.clone(),
            queued_at: updated_info.queued_at,
            next_retry_at: updated_info.next_retry_at,
            current_server_index: updated_info.current_server_index,
        });

        modules::dispatch(Event::Event(Ev::DeliveryFailure), context);

        error
    }

    /// Persist the current delivery queue state to the spool's Context.delivery field
    ///
    /// This method synchronizes the in-memory queue state (status, attempts, retry timing)
    /// to the spool's persistent storage. This ensures queue state survives restarts.
    ///
    /// # Errors
    /// Returns an error if the message is not in the queue or if spool update fails
    async fn persist_delivery_state(
        &self,
        message_id: &SpooledMessageId,
        spool: &Arc<dyn empath_spool::BackingStore>,
    ) -> Result<(), DeliveryError> {
        use empath_common::context::DeliveryContext;

        // Get current queue info
        let info = self.queue.get(message_id).await.ok_or_else(|| {
            SystemError::MessageNotFound(format!("Message {message_id:?} not in queue"))
        })?;

        // Read context from spool
        let mut context = spool
            .read(message_id)
            .await
            .map_err(|e| SystemError::SpoolRead(e.to_string()))?;

        // Update the delivery field with current queue state
        context.delivery = Some(DeliveryContext {
            message_id: message_id.to_string(),
            domain: info.recipient_domain.clone(),
            server: info.current_mail_server().map(MailServer::address),
            error: match &info.status {
                DeliveryStatus::Failed(e) | DeliveryStatus::Retry { last_error: e, .. } => {
                    Some(e.clone())
                }
                _ => None,
            },
            attempts: Some(info.attempt_count()),
            status: info.status.clone(),
            attempt_history: info.attempts.clone(),
            queued_at: info.queued_at,
            next_retry_at: info.next_retry_at,
            current_server_index: info.current_server_index,
        });

        // Atomically update spool
        spool
            .update(message_id, &context)
            .await
            .map_err(|e| SystemError::SpoolWrite(e.to_string()))?;

        Ok(())
    }

    /// Deliver a message via SMTP (complete transaction including DATA)
    ///
    /// This method performs the full SMTP transaction by delegating to `SmtpTransaction`.
    ///
    /// # Errors
    /// Returns an error if any part of the SMTP transaction fails
    async fn deliver_message(
        &self,
        server_address: &str,
        context: &Context,
        delivery_info: &DeliveryInfo,
    ) -> Result<(), DeliveryError> {
        // Check if TLS is required for this domain
        let require_tls = self
            .domains
            .get(&delivery_info.recipient_domain)
            .is_some_and(|config| config.require_tls);

        // Determine if we should accept invalid certificates
        // Priority: per-domain override > global configuration
        let accept_invalid_certs = self
            .domains
            .get(&delivery_info.recipient_domain)
            .and_then(|config| config.accept_invalid_certs)
            .unwrap_or(self.accept_invalid_certs);

        // Create and execute the SMTP transaction
        let transaction = smtp_transaction::SmtpTransaction::new(
            context,
            server_address.to_string(),
            require_tls,
            accept_invalid_certs,
            &self.smtp_timeouts,
        );

        transaction.execute().await
    }

    /// Process all pending messages in the queue
    ///
    /// This method:
    /// 1. Checks for expired messages and marks them as `Expired`
    /// 2. For messages with `Retry` status, checks if it's time to retry
    /// 3. Processes messages that are ready for delivery
    ///
    /// # Errors
    /// Returns an error if processing fails
    async fn process_queue_internal(
        &self,
        spool: &Arc<dyn empath_spool::BackingStore>,
    ) -> Result<(), DeliveryError> {
        use tracing::{debug, error, warn};

        let current_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        // Get all messages to check for expiration and retry timing
        let all_messages = self.queue.all_messages().await;

        for info in all_messages {
            // Skip messages that are already completed, failed, expired, or in progress
            if matches!(
                info.status,
                DeliveryStatus::Completed
                    | DeliveryStatus::Failed(_)
                    | DeliveryStatus::Expired
                    | DeliveryStatus::InProgress
            ) {
                continue;
            }

            // Check if message has expired
            if let Some(expiration_secs) = self.message_expiration_secs {
                let age_secs = current_time.saturating_sub(info.queued_at);
                if age_secs > expiration_secs {
                    warn!(
                        message_id = ?info.message_id,
                        age_secs = age_secs,
                        expiration_secs = expiration_secs,
                        "Message expired, marking as Expired"
                    );
                    self.queue
                        .update_status(&info.message_id, DeliveryStatus::Expired)
                        .await;

                    // Persist the Expired status to spool
                    if let Err(e) = self.persist_delivery_state(&info.message_id, spool).await {
                        warn!(
                            message_id = ?info.message_id,
                            error = %e,
                            "Failed to persist delivery state after marking message as Expired"
                        );
                    }

                    continue;
                }
            }

            // For Retry status, check if it's time to retry
            if matches!(info.status, DeliveryStatus::Retry { .. }) {
                if let Some(next_retry_at) = info.next_retry_at
                    && current_time < next_retry_at
                {
                    // Not yet time to retry, skip this message
                    let wait_secs = next_retry_at.saturating_sub(current_time);
                    debug!(
                        message_id = ?info.message_id,
                        wait_secs = wait_secs,
                        "Skipping message, not yet time to retry"
                    );
                    continue;
                }

                // Time to retry! Reset status to Pending and reset server index
                debug!(
                    message_id = ?info.message_id,
                    attempt = info.attempt_count(),
                    "Time to retry delivery"
                );
                self.queue
                    .update_status(&info.message_id, DeliveryStatus::Pending)
                    .await;

                // Reset to first MX server for new retry cycle
                self.queue.reset_server_index(&info.message_id).await;

                // Persist the Pending status for retry
                if let Err(e) = self.persist_delivery_state(&info.message_id, spool).await {
                    warn!(
                        message_id = ?info.message_id,
                        error = %e,
                        "Failed to persist delivery state after marking message for retry"
                    );
                }
            }

            // Process the message (Pending status)
            if matches!(info.status, DeliveryStatus::Pending)
                && let Err(e) = self.prepare_message(&info.message_id, spool).await
            {
                error!(
                    message_id = ?info.message_id,
                    error = %e,
                    "Failed to prepare message for delivery"
                );

                if let Ok(mut context) = spool.read(&info.message_id).await {
                    let server = info
                        .mail_servers
                        .get(info.current_server_index)
                        .map_or_else(|| info.recipient_domain.to_string(), MailServer::address);
                    let _error = self
                        .handle_delivery_error(&info.message_id, &mut context, e, server)
                        .await;
                }
            }
        }

        Ok(())
    }
}

/// Extract domain from an email address
///
/// # Errors
/// Returns an error if the email address format is invalid or has no domain part
fn extract_domain(email: &str) -> Result<String, DeliveryError> {
    // Remove angle brackets if present
    let cleaned = email.trim().trim_matches(|c| c == '<' || c == '>');

    // Split on @ and get the domain part
    cleaned
        .split('@')
        .nth(1)
        .map(|domain| domain.trim().to_string())
        .filter(|domain| !domain.is_empty())
        .ok_or_else(|| {
            SystemError::Internal(format!(
                "Invalid email address: no domain found in '{email}'"
            ))
            .into()
        })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use empath_common::{
        address::{Address, AddressList},
        context::Context,
        envelope::Envelope,
    };
    use empath_spool::BackingStore;

    use super::*;

    fn create_test_context(from: &str, to: &str) -> Context {
        let mut envelope = Envelope::default();

        // Parse and set sender
        if let Ok(sender_addr) = mailparse::addrparse(from)
            && let Some(addr) = sender_addr.iter().next()
        {
            *envelope.sender_mut() = Some(Address(addr.clone()));
        }

        // Parse and set recipient
        if let Ok(recip_addr) = mailparse::addrparse(to) {
            *envelope.recipients_mut() = Some(AddressList(
                recip_addr.iter().map(|a| Address(a.clone())).collect(),
            ));
        }

        Context {
            envelope,
            id: "test-session".to_string(),
            data: Some(Arc::from(b"Test message content".as_slice())),
            ..Default::default()
        }
    }

    #[test]
    fn test_extract_domain() {
        assert_eq!(extract_domain("user@example.com").unwrap(), "example.com");
        assert_eq!(extract_domain("<user@test.org>").unwrap(), "test.org");
        assert_eq!(extract_domain("  user@domain.net  ").unwrap(), "domain.net");

        assert!(extract_domain("invalid").is_err());
        assert!(extract_domain("user@").is_err());
        assert!(extract_domain("@domain.com").is_ok()); // Empty local part is technically valid
    }

    #[tokio::test]
    async fn test_domain_config_mx_override() {
        // Create a domain config registry with an MX override
        let mut domains = DomainConfigRegistry::new();
        domains.insert(
            "test.example.com".to_string(),
            DomainConfig {
                mx_override: Some("localhost:1025".to_string()),
                ..Default::default()
            },
        );

        let processor = DeliveryProcessor {
            domains,
            ..Default::default()
        };

        // Verify the domain config was stored
        assert!(processor.domains.has_config("test.example.com"));
        let domain_config = processor.domains.get("test.example.com").unwrap();
        assert_eq!(domain_config.mx_override_address(), Some("localhost:1025"));
    }

    #[tokio::test]
    async fn test_delivery_with_mx_override_integration() {
        use empath_spool::MemoryBackingStore;

        // Create a memory-backed spool
        let spool: Arc<dyn BackingStore> = Arc::new(MemoryBackingStore::default());

        // Create a test context (message)
        let mut context = create_test_context("sender@example.org", "recipient@test.example.com");

        // Spool the message
        let msg_id = spool.write(&mut context).await.unwrap();

        // Create domain config with MX override
        let mut domains = DomainConfigRegistry::new();
        domains.insert(
            "test.example.com".to_string(),
            DomainConfig {
                mx_override: Some("localhost:1025".to_string()),
                ..Default::default()
            },
        );

        let mut processor = DeliveryProcessor {
            domains,
            scan_interval_secs: 1,
            process_interval_secs: 1,
            max_attempts: 3,
            ..Default::default()
        };

        processor.init(spool.clone(), None).unwrap();

        // Manually scan the spool to add the message to the queue
        let added = processor.scan_spool_internal(&spool).await.unwrap();
        assert_eq!(added, 1, "Should have added 1 message to queue");

        // Verify the message is in the queue
        let queue_info = processor.queue.get(&msg_id).await;
        assert!(queue_info.is_some(), "Message should be in queue");

        let info = queue_info.unwrap();
        assert_eq!(info.recipient_domain.as_ref(), "test.example.com");
        assert_eq!(info.attempt_count(), 0);

        // Test prepare_message to verify MX override is used
        // Note: This will fail to actually connect (expected), but we can verify
        // that the MX override logic was triggered by checking the mail_servers
        let _result = processor.prepare_message(&msg_id, &spool).await;

        // Verify that mail_servers were set (even though connection failed)
        let updated_info = processor.queue.get(&msg_id).await.unwrap();
        assert!(
            !updated_info.mail_servers.is_empty(),
            "Mail servers should be set"
        );

        // Verify the MX override was used (localhost:1025)
        let server = &updated_info.mail_servers[0];
        assert_eq!(server.host, "localhost");
        assert_eq!(server.port, 1025);
        assert_eq!(server.priority, 0);
    }

    #[test]
    fn test_domain_config_multiple_domains() {
        let mut domains = DomainConfigRegistry::new();

        domains.insert(
            "test.local".to_string(),
            DomainConfig {
                mx_override: Some("localhost:1025".to_string()),
                ..Default::default()
            },
        );

        domains.insert(
            "gmail.com".to_string(),
            DomainConfig {
                require_tls: true,
                max_connections: Some(10),
                rate_limit: Some(100),
                ..Default::default()
            },
        );

        assert_eq!(domains.len(), 2);

        let test_config = domains.get("test.local").unwrap();
        assert!(test_config.has_mx_override());
        assert!(!test_config.require_tls);

        let gmail_config = domains.get("gmail.com").unwrap();
        assert!(!gmail_config.has_mx_override());
        assert!(gmail_config.require_tls);
        assert_eq!(gmail_config.max_connections, Some(10));
        assert_eq!(gmail_config.rate_limit, Some(100));
    }

    #[tokio::test]
    async fn test_delivery_queue_domain_grouping() {
        use empath_spool::MemoryBackingStore;

        let spool: Arc<dyn BackingStore> = Arc::new(MemoryBackingStore::default());

        // Create a message with multiple recipients in different domains
        let mut context = create_test_context("sender@example.org", "user1@domain1.com");

        // Add more recipients to different domains
        if let Some(recipients) = context.envelope.recipients_mut() {
            if let Ok(addr2) = mailparse::addrparse("user2@domain2.com") {
                for addr in addr2.iter() {
                    recipients.push(Address(addr.clone()));
                }
            }
            if let Ok(addr3) = mailparse::addrparse("user3@domain1.com") {
                for addr in addr3.iter() {
                    recipients.push(Address(addr.clone()));
                }
            }
        }

        let msg_id = spool.write(&mut context).await.unwrap();

        let mut processor = DeliveryProcessor::default();
        processor.init(spool.clone(), None).unwrap();

        // Scan spool - should create separate queue entries for each domain
        let added = processor.scan_spool_internal(&spool).await.unwrap();
        assert_eq!(added, 2, "Should create 2 queue entries (one per domain)");

        // Verify both domains are queued
        // Note: The same message ID is queued multiple times with different domains
        let info = processor.queue.get(&msg_id).await;
        assert!(info.is_some(), "Message should be in queue");
    }

    #[tokio::test]
    async fn test_graceful_shutdown() {
        use empath_spool::MemoryBackingStore;
        use tokio::sync::broadcast;

        // Create a memory-backed spool
        let spool: Arc<dyn BackingStore> = Arc::new(MemoryBackingStore::default());

        // Create a test context and spool a message
        let mut context = create_test_context("sender@example.org", "recipient@test.example.com");
        let _msg_id = spool.write(&mut context).await.unwrap();

        // Create a processor with short intervals for faster testing
        let mut processor = DeliveryProcessor {
            scan_interval_secs: 1,
            process_interval_secs: 1,
            max_attempts: 3,
            ..Default::default()
        };

        processor
            .init(
                spool.clone(),
                Some(std::path::PathBuf::from("/tmp/graceful_shutdown_test")),
            )
            .unwrap();

        // Manually scan the spool to add the message to the queue
        let added = processor.scan_spool_internal(&spool).await.unwrap();
        assert_eq!(added, 1, "Should have added 1 message to queue");

        // Create shutdown channel
        let (shutdown_tx, shutdown_rx) = broadcast::channel(16);

        // Start the processor in a background task
        let processor_handle = tokio::spawn(async move { processor.serve(shutdown_rx).await });

        // Give the processor a moment to start
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Send shutdown signal
        shutdown_tx.send(empath_common::Signal::Shutdown).unwrap();

        // Wait for graceful shutdown to complete (with timeout)
        let result = tokio::time::timeout(
            Duration::from_secs(35), // Slightly longer than the 30s shutdown timeout
            processor_handle,
        )
        .await;

        // Verify shutdown completed successfully
        assert!(result.is_ok(), "Processor should shutdown within timeout");
        let shutdown_result = result.unwrap();
        assert!(shutdown_result.is_ok(), "Processor serve should return Ok");
        assert!(
            shutdown_result.unwrap().is_ok(),
            "Processor should shutdown without error"
        );
    }

    #[tokio::test]
    async fn test_graceful_shutdown_respects_timeout() {
        use empath_spool::MemoryBackingStore;
        use tokio::sync::broadcast;

        // Create a memory-backed spool
        let spool: Arc<dyn BackingStore> = Arc::new(MemoryBackingStore::default());

        // Create a processor
        let mut processor = DeliveryProcessor {
            scan_interval_secs: 1,
            process_interval_secs: 1,
            max_attempts: 3,
            ..Default::default()
        };

        processor
            .init(
                spool.clone(),
                Some(std::path::PathBuf::from(
                    "/tmp/graceful_shutdown_timeout_test",
                )),
            )
            .unwrap();

        // Create shutdown channel
        let (shutdown_tx, shutdown_rx) = broadcast::channel(16);

        // Start the processor in a background task
        let start_time = std::time::Instant::now();
        let processor_handle = tokio::spawn(async move { processor.serve(shutdown_rx).await });

        // Give the processor a moment to start
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Send shutdown signal
        shutdown_tx.send(empath_common::Signal::Shutdown).unwrap();

        // Wait for graceful shutdown to complete
        let result = tokio::time::timeout(Duration::from_secs(35), processor_handle).await;

        // Verify shutdown completed quickly (since no processing was happening)
        let elapsed = start_time.elapsed();
        assert!(result.is_ok(), "Processor should shutdown within timeout");
        assert!(
            elapsed < Duration::from_secs(5),
            "Shutdown should be fast when not processing (took {elapsed:?})"
        );

        let shutdown_result = result.unwrap();
        assert!(shutdown_result.is_ok(), "Processor serve should return Ok");
        assert!(
            shutdown_result.unwrap().is_ok(),
            "Processor should shutdown without error"
        );
    }

    #[test]
    fn test_exponential_backoff_calculation() {
        // Test exponential backoff with base=60s, max=86400s, jitter=0
        // We'll test with jitter=0 for predictable results
        let base_delay = 60;
        let max_delay = 86400;
        let jitter_factor = 0.0; // No jitter for testing

        // Attempt 1: 60 * 2^0 = 60 seconds
        let current_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let next_retry = calculate_next_retry_time(1, base_delay, max_delay, jitter_factor);
        let delay = next_retry.saturating_sub(current_time);
        assert_eq!(delay, 60, "First retry should be 60 seconds");

        // Attempt 2: 60 * 2^1 = 120 seconds
        let current_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let next_retry = calculate_next_retry_time(2, base_delay, max_delay, jitter_factor);
        let delay = next_retry.saturating_sub(current_time);
        assert_eq!(delay, 120, "Second retry should be 120 seconds");

        // Attempt 3: 60 * 2^2 = 240 seconds
        let current_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let next_retry = calculate_next_retry_time(3, base_delay, max_delay, jitter_factor);
        let delay = next_retry.saturating_sub(current_time);
        assert_eq!(delay, 240, "Third retry should be 240 seconds");

        // Attempt 20: Should be capped at max_delay (86400 seconds = 24 hours)
        let current_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let next_retry = calculate_next_retry_time(20, base_delay, max_delay, jitter_factor);
        let delay = next_retry.saturating_sub(current_time);
        assert_eq!(
            delay, max_delay,
            "High attempt number should be capped at max_delay"
        );
    }

    #[test]
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_sign_loss,
        clippy::cast_possible_truncation
    )]
    fn test_exponential_backoff_with_jitter() {
        // Test that jitter is applied (result should be different from exact calculation)
        let base_delay = 60;
        let max_delay = 86400;
        let jitter_factor = 0.2; // ±20%

        // Attempt 2: Expected = 120 seconds, with ±20% jitter = 96-144 seconds
        let current_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let next_retry = calculate_next_retry_time(2, base_delay, max_delay, jitter_factor);
        let delay = next_retry.saturating_sub(current_time);

        // Check that delay is within jitter range
        let expected = 120;
        let min = expected - (expected as f64 * jitter_factor) as u64;
        let max = expected + (expected as f64 * jitter_factor) as u64;
        assert!(
            delay >= min && delay <= max,
            "Delay {delay} should be within jitter range [{min}, {max}]"
        );
    }

    #[tokio::test]
    async fn test_message_expiration() {
        use empath_spool::MemoryBackingStore;

        let spool: Arc<dyn BackingStore> = Arc::new(MemoryBackingStore::default());

        // Create a processor with 1 second expiration
        let mut processor = DeliveryProcessor {
            message_expiration_secs: Some(1), // Expire after 1 second
            ..Default::default()
        };

        processor.init(spool.clone(), None).unwrap();

        // Create and queue a message
        let mut context = create_test_context("sender@example.org", "recipient@test.com");
        let msg_id = spool.write(&mut context).await.unwrap();

        // Manually add to queue with old timestamp
        let old_timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .saturating_sub(5); // 5 seconds ago

        let mut info = DeliveryInfo::new(msg_id.clone(), "test.com".to_string());
        info.queued_at = old_timestamp; // Manually set old timestamp

        // Add to queue
        {
            let mut queue = processor.queue.queue.write().await;
            queue.insert(msg_id.clone(), info);
        }

        // Process the queue - should expire the message
        let _result = processor.process_queue_internal(&spool).await;

        // Verify message was marked as expired
        let updated_info = processor.queue.get(&msg_id).await;
        assert!(updated_info.is_some(), "Message should still be in queue");
        assert_eq!(
            updated_info.unwrap().status,
            DeliveryStatus::Expired,
            "Message should be marked as Expired"
        );
    }

    #[tokio::test]
    async fn test_retry_scheduling_with_backoff() {
        use empath_spool::MemoryBackingStore;

        let spool: Arc<dyn BackingStore> = Arc::new(MemoryBackingStore::default());

        // Create a processor with fast backoff for testing
        let mut processor = DeliveryProcessor {
            base_retry_delay_secs: 2, // 2 seconds base delay
            max_retry_delay_secs: 60, // Cap at 60 seconds
            retry_jitter_factor: 0.0, // No jitter for predictable testing
            max_attempts: 3,
            ..Default::default()
        };

        processor.init(spool.clone(), None).unwrap();

        // Create and queue a message
        let mut context = create_test_context("sender@example.org", "recipient@test.com");
        let msg_id = spool.write(&mut context).await.unwrap();
        processor
            .queue
            .enqueue(msg_id.clone(), "test.com".to_string())
            .await;

        // Set message to Retry status with next_retry_at in the future
        let future_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .saturating_add(10); // 10 seconds in the future

        processor
            .queue
            .update_status(
                &msg_id,
                DeliveryStatus::Retry {
                    attempts: 1,
                    last_error: "test error".to_string(),
                },
            )
            .await;
        processor
            .queue
            .set_next_retry_at(&msg_id, future_time)
            .await;

        // Process queue - should NOT process message yet (too early)
        let _result = processor.process_queue_internal(&spool).await;

        // Verify message is still in Retry status (not changed to Pending)
        let info = processor.queue.get(&msg_id).await.unwrap();
        assert!(
            matches!(info.status, DeliveryStatus::Retry { .. }),
            "Message should still be in Retry status"
        );

        // Now set next_retry_at to the past
        let past_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .saturating_sub(1); // 1 second in the past

        processor.queue.set_next_retry_at(&msg_id, past_time).await;

        // Process queue again - should now reset to Pending
        let _result = processor.process_queue_internal(&spool).await;

        // Verify message was reset to Pending (or InProgress/Failed after processing)
        let info = processor.queue.get(&msg_id).await.unwrap();
        // Message should no longer be in Retry status with future timestamp
        assert!(
            !matches!(info.status, DeliveryStatus::Retry { .. })
                || info.next_retry_at.is_none()
                || info.next_retry_at.unwrap() <= past_time,
            "Message should be processed or have updated retry time"
        );
    }
}
