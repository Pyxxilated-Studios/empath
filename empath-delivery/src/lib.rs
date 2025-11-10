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

use std::{collections::HashMap, sync::Arc, time::Duration};

pub use dns::{DnsConfig, DnsError, DnsResolver, MailServer};
pub use domain_config::{DomainConfig, DomainConfigRegistry};
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

/// Status of a message in the delivery queue
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeliveryStatus {
    /// Message is pending delivery
    Pending,
    /// Message delivery is in progress
    InProgress,
    /// Message was successfully delivered
    Completed,
    /// Message delivery failed permanently
    Failed(String),
    /// Message delivery failed temporarily, will retry
    Retry { attempts: u32, last_error: String },
}

/// Represents a single delivery attempt
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeliveryAttempt {
    /// Timestamp of the attempt
    pub timestamp: u64,
    /// Error message if the attempt failed
    pub error: Option<String>,
    /// SMTP server that was contacted
    pub server: String,
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
    pub recipient_domain: String,
    /// Resolved mail servers (sorted by priority)
    pub mail_servers: Vec<MailServer>,
    /// Index of the current mail server being tried
    pub current_server_index: usize,
}

impl DeliveryInfo {
    /// Create a new pending delivery info
    #[must_use]
    pub const fn new(message_id: SpooledMessageId, recipient_domain: String) -> Self {
        Self {
            message_id,
            status: DeliveryStatus::Pending,
            attempts: Vec::new(),
            recipient_domain,
            mail_servers: Vec::new(),
            current_server_index: 0,
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
    pub const fn try_next_server(&mut self) -> bool {
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
    pub async fn set_mail_servers(&self, message_id: &SpooledMessageId, servers: Vec<MailServer>) {
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

    /// Get count of messages by status
    pub async fn count_by_status(&self) -> HashMap<String, usize> {
        let queue = self.queue.read().await;
        let mut counts = HashMap::new();

        for info in queue.values() {
            let status_key = match &info.status {
                DeliveryStatus::Pending => "pending",
                DeliveryStatus::InProgress => "in_progress",
                DeliveryStatus::Completed => "completed",
                DeliveryStatus::Failed(_) => "failed",
                DeliveryStatus::Retry { .. } => "retry",
            };
            *counts.entry(status_key.to_string()).or_insert(0) += 1;
        }

        drop(queue); // Explicit early drop of lock guard

        counts
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

    /// Path to persist queue state (JSON file for CLI access)
    #[serde(skip)]
    queue_state_path: Option<std::path::PathBuf>,

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
            accept_invalid_certs: false,
            dns: DnsConfig::default(),
            domains: DomainConfigRegistry::default(),
            smtp_timeouts: SmtpTimeouts::default(),
            spool: None,
            queue: DeliveryQueue::new(),
            dns_resolver: None,
            queue_state_path: None,
            freeze_marker_path: None,
        }
    }
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
            "DNS resolver initialized with timeout={}s, cache_ttl={}s, cache_size={}",
            self.dns.timeout_secs,
            self.dns.cache_ttl_secs,
            self.dns.cache_size
        );

        // Set up queue state persistence paths based on spool directory
        // If spool_path is provided, derive paths from it, otherwise use /tmp/spool
        let base_path = spool_path.unwrap_or_else(|| std::path::PathBuf::from("/tmp/spool"));
        self.queue_state_path = Some(base_path.join("queue_state.bin"));
        self.freeze_marker_path = Some(base_path.join("queue_frozen"));

        Ok(())
    }

    /// Run the delivery processor
    ///
    /// This method runs continuously until a shutdown signal is received.
    /// It periodically scans the spool for new messages and processes the
    /// delivery queue.
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

                    match self.process_queue_internal(spool).await {
                        Ok(()) => {
                            tracing::debug!("Processed delivery queue");
                        }
                        Err(e) => {
                            tracing::error!("Error processing delivery queue: {e}");
                        }
                    }
                }
                _ = state_save_timer.tick() => {
                    // Periodically persist queue state for CLI access
                    if let Err(e) = self.save_queue_state().await {
                        tracing::warn!("Failed to save queue state: {e}");
                    }
                }
                sig = shutdown.recv() => {
                    match sig {
                        Ok(Signal::Shutdown | Signal::Finalised) => {
                            internal!("Delivery processor shutting down");
                            // Save final queue state before shutdown
                            let _ignore_error = self.save_queue_state().await;
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

    /// Save the current queue state to a bincode file for CLI access
    ///
    /// This allows the `empathctl` CLI tool to inspect queue status without
    /// requiring a running API server or IPC mechanism.
    ///
    /// # Errors
    /// Returns an error if the queue state cannot be serialized or written
    async fn save_queue_state(&self) -> Result<(), DeliveryError> {
        if let Some(ref state_path) = self.queue_state_path {
            let queue_data = self.queue.all_messages().await;

            // Convert to a HashMap keyed by message ID string for easier CLI access
            let state_map: std::collections::HashMap<String, DeliveryInfo> = queue_data
                .into_iter()
                .map(|info| (info.message_id.to_string(), info))
                .collect();

            let encoded = bincode::serialize(&state_map).map_err(|e| {
                SystemError::Internal(format!("Failed to serialize queue state: {e}"))
            })?;

            // Create parent directory if it doesn't exist
            if let Some(parent) = state_path.parent() {
                let _ignore_error = tokio::fs::create_dir_all(parent).await;
            }

            // Write to temporary file first, then rename for atomic update
            let temp_path = state_path.with_extension("tmp");
            tokio::fs::write(&temp_path, encoded)
                .await
                .map_err(|e| SystemError::Internal(format!("Failed to write queue state: {e}")))?;

            tokio::fs::rename(&temp_path, state_path)
                .await
                .map_err(|e| {
                    SystemError::Internal(format!("Failed to rename queue state file: {e}"))
                })?;

            tracing::trace!("Queue state saved to {:?}", state_path);
        }

        Ok(())
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

            // Read the message to get recipient domains
            let message = spool
                .read(&msg_id)
                .await
                .map_err(|e| SystemError::SpoolRead(e.to_string()))?;

            // Group recipients by domain (handle multi-recipient messages)
            let Some(recipients) = message.envelope.recipients() else {
                use tracing::warn;
                warn!("Message {:?} has no recipients, skipping", msg_id);
                continue;
            };

            // Collect unique domains from all recipients
            let mut domains = std::collections::HashMap::new();
            for recipient in recipients.iter() {
                // Extract the actual email address from the MailAddr
                let recipient_str = match &**recipient {
                    mailparse::MailAddr::Single(single) => single.addr.clone(),
                    mailparse::MailAddr::Group(_) => continue, // Skip groups
                };

                match extract_domain(&recipient_str) {
                    Ok(domain) => {
                        domains
                            .entry(domain)
                            .or_insert_with(Vec::new)
                            .push(recipient_str);
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
    async fn prepare_message(
        &self,
        message_id: &SpooledMessageId,
        spool: &Arc<dyn empath_spool::BackingStore>,
    ) -> Result<(), DeliveryError> {
        use empath_common::context::DeliveryContext;

        self.queue
            .update_status(message_id, DeliveryStatus::InProgress)
            .await;

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

            vec![MailServer {
                host,
                port,
                priority: 0,
            }]
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
                return Err(PermanentError::NoMailServers(info.recipient_domain.clone()).into());
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

        self.queue.update_status(message_id, new_status).await;

        context.delivery = Some(DeliveryContext {
            message_id: message_id.to_string(),
            domain: updated_info.recipient_domain.clone(),
            server: Some(server),
            error: Some(error.to_string()),
            attempts: Some(updated_info.attempt_count()),
        });

        modules::dispatch(Event::Event(Ev::DeliveryFailure), context);

        error
    }

    /// Deliver a message via SMTP (complete transaction including DATA)
    ///
    /// This method performs the full SMTP transaction:
    /// 1. Connects to the SMTP server
    /// 2. Optionally upgrades to TLS via STARTTLS if required
    /// 3. Performs EHLO/HELO handshake
    /// 4. Sends MAIL FROM and RCPT TO commands
    /// 5. Sends DATA command and the actual message content
    /// 6. Sends QUIT to cleanly close the connection
    ///
    /// # Errors
    /// Returns an error if any part of the SMTP transaction fails
    async fn deliver_message(
        &self,
        server_address: &str,
        context: &Context,
        delivery_info: &DeliveryInfo,
    ) -> Result<(), DeliveryError> {
        use empath_smtp::client::SmtpClient;

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

        // Log security warning if certificate validation is disabled
        if accept_invalid_certs {
            tracing::warn!(
                domain = %delivery_info.recipient_domain,
                server = %server_address,
                "SECURITY WARNING: TLS certificate validation is disabled for this connection"
            );
        }

        // Connect to the SMTP server
        let mut client = SmtpClient::connect(server_address, server_address.to_string())
            .await
            .map_err(|e| {
                TemporaryError::ConnectionFailed(format!(
                    "Failed to connect to {server_address}: {e}"
                ))
            })?
            .accept_invalid_certs(accept_invalid_certs);

        // Read greeting
        let greeting = client.read_greeting().await.map_err(|e| {
            TemporaryError::ConnectionFailed(format!("Failed to read greeting: {e}"))
        })?;

        if !greeting.is_success() {
            return Err(TemporaryError::ServerBusy(format!(
                "Server rejected connection: {}",
                greeting.message()
            ))
            .into());
        }

        // Perform TLS negotiation if required or available
        self.negotiate_tls(&mut client, &context.id, require_tls)
            .await?;

        // Send MAIL FROM
        self.send_mail_from(&mut client, context).await?;

        // Send RCPT TO for all recipients
        self.send_rcpt_to(&mut client, context).await?;

        // Send DATA command and message content
        self.send_message_data(&mut client, context).await?;

        // Send QUIT to cleanly close the connection
        // Note: We don't fail the delivery if QUIT fails since the message was already delivered
        let quit_timeout = Duration::from_secs(self.smtp_timeouts.quit_secs);
        if let Err(e) = tokio::time::timeout(quit_timeout, client.quit()).await {
            tracing::warn!(
                server = %server_address,
                timeout = ?quit_timeout,
                "QUIT command timed out after successful delivery: {e}"
            );
        }

        Ok(())
    }

    /// Negotiate TLS upgrade via STARTTLS if required or available
    ///
    /// # Errors
    /// Returns an error if TLS is required but fails, or if STARTTLS negotiation fails
    async fn negotiate_tls(
        &self,
        client: &mut empath_smtp::client::SmtpClient,
        helo_domain: &str,
        require_tls: bool,
    ) -> Result<(), DeliveryError> {
        // Send initial EHLO
        let ehlo_timeout = Duration::from_secs(self.smtp_timeouts.ehlo_secs);
        let ehlo_response = tokio::time::timeout(ehlo_timeout, client.ehlo(helo_domain))
            .await
            .map_err(|_| TemporaryError::Timeout(format!("EHLO timed out after {ehlo_timeout:?}")))?
            .map_err(|e| TemporaryError::SmtpTemporary(format!("EHLO failed: {e}")))?;

        if !ehlo_response.is_success() {
            return Err(TemporaryError::SmtpTemporary(format!(
                "Server rejected EHLO: {}",
                ehlo_response.message()
            ))
            .into());
        }

        // Check if server advertises STARTTLS
        let supports_starttls = ehlo_response
            .message()
            .lines()
            .any(|line| line.to_uppercase().contains("STARTTLS"));

        if require_tls || supports_starttls {
            let starttls_timeout = Duration::from_secs(self.smtp_timeouts.starttls_secs);
            let starttls_response = tokio::time::timeout(starttls_timeout, client.starttls())
                .await
                .map_err(|_| {
                    if require_tls {
                        DeliveryError::Permanent(PermanentError::TlsRequired(format!(
                            "STARTTLS timed out after {starttls_timeout:?}"
                        )))
                    } else {
                        DeliveryError::Temporary(TemporaryError::Timeout(format!(
                            "STARTTLS timed out after {starttls_timeout:?}"
                        )))
                    }
                })?
                .map_err(|e| {
                    if require_tls {
                        DeliveryError::Permanent(PermanentError::TlsRequired(format!(
                            "STARTTLS failed: {e}"
                        )))
                    } else {
                        DeliveryError::Temporary(TemporaryError::SmtpTemporary(format!(
                            "STARTTLS failed: {e}"
                        )))
                    }
                })?;

            if !starttls_response.is_success() {
                let message = format!("Server rejected STARTTLS: {}", starttls_response.message());
                return if require_tls {
                    Err(PermanentError::TlsRequired(message).into())
                } else {
                    Err(TemporaryError::SmtpTemporary(message).into())
                };
            }

            // Re-send EHLO after STARTTLS (RFC 3207)
            let ehlo_response = tokio::time::timeout(ehlo_timeout, client.ehlo(helo_domain))
                .await
                .map_err(|_| {
                    TemporaryError::Timeout(format!(
                        "EHLO after STARTTLS timed out after {ehlo_timeout:?}"
                    ))
                })?
                .map_err(|e| {
                    TemporaryError::SmtpTemporary(format!("EHLO after STARTTLS failed: {e}"))
                })?;

            if !ehlo_response.is_success() {
                return Err(TemporaryError::SmtpTemporary(format!(
                    "Server rejected EHLO after STARTTLS: {}",
                    ehlo_response.message()
                ))
                .into());
            }
        }

        Ok(())
    }

    /// Send MAIL FROM command
    ///
    /// # Errors
    /// Returns an error if the MAIL FROM command fails
    async fn send_mail_from(
        &self,
        client: &mut empath_smtp::client::SmtpClient,
        context: &Context,
    ) -> Result<(), DeliveryError> {
        let sender = context
            .envelope
            .sender()
            .and_then(extract_email_address)
            .unwrap_or_default();

        let mail_from_timeout = Duration::from_secs(self.smtp_timeouts.mail_from_secs);
        let mail_response =
            tokio::time::timeout(mail_from_timeout, client.mail_from(&sender, None))
                .await
                .map_err(|_| {
                    TemporaryError::Timeout(format!(
                        "MAIL FROM timed out after {mail_from_timeout:?}"
                    ))
                })?
                .map_err(|e| TemporaryError::SmtpTemporary(format!("MAIL FROM failed: {e}")))?;

        if !mail_response.is_success() {
            let code = mail_response.code;
            let message = format!("Server rejected MAIL FROM: {}", mail_response.message());
            return if (500..600).contains(&code) {
                Err(PermanentError::MessageRejected(message).into())
            } else {
                Err(TemporaryError::SmtpTemporary(message).into())
            };
        }

        Ok(())
    }

    /// Send RCPT TO commands for all recipients
    ///
    /// # Errors
    /// Returns an error if any RCPT TO command fails
    async fn send_rcpt_to(
        &self,
        client: &mut empath_smtp::client::SmtpClient,
        context: &Context,
    ) -> Result<(), DeliveryError> {
        let Some(recipients) = context.envelope.recipients() else {
            return Err(SystemError::Internal("No recipients in message".to_string()).into());
        };

        let rcpt_to_timeout = Duration::from_secs(self.smtp_timeouts.rcpt_to_secs);
        for recipient in recipients.iter() {
            let Some(recipient_addr) = extract_email_address(recipient) else {
                continue; // Skip group addresses
            };

            let rcpt_response =
                tokio::time::timeout(rcpt_to_timeout, client.rcpt_to(&recipient_addr))
                    .await
                    .map_err(|_| {
                        TemporaryError::Timeout(format!(
                            "RCPT TO timed out after {rcpt_to_timeout:?}"
                        ))
                    })?
                    .map_err(|e| TemporaryError::SmtpTemporary(format!("RCPT TO failed: {e}")))?;

            if !rcpt_response.is_success() {
                let code = rcpt_response.code;
                let message = format!(
                    "Server rejected RCPT TO {}: {}",
                    recipient_addr,
                    rcpt_response.message()
                );
                return if (500..600).contains(&code) {
                    Err(PermanentError::InvalidRecipient(message).into())
                } else {
                    Err(TemporaryError::SmtpTemporary(message).into())
                };
            }
        }

        Ok(())
    }

    /// Send DATA command and message content
    ///
    /// # Errors
    /// Returns an error if the DATA command or message sending fails
    async fn send_message_data(
        &self,
        client: &mut empath_smtp::client::SmtpClient,
        context: &Context,
    ) -> Result<(), DeliveryError> {
        let data_timeout = Duration::from_secs(self.smtp_timeouts.data_secs);

        // Send DATA command
        let data_response = tokio::time::timeout(data_timeout, client.data())
            .await
            .map_err(|_| {
                TemporaryError::Timeout(format!("DATA command timed out after {data_timeout:?}"))
            })?
            .map_err(|e| TemporaryError::SmtpTemporary(format!("DATA command failed: {e}")))?;

        if !(300..400).contains(&data_response.code) {
            let code = data_response.code;
            let message = format!("Server rejected DATA: {}", data_response.message());
            return if (500..600).contains(&code) {
                Err(PermanentError::MessageRejected(message).into())
            } else {
                Err(TemporaryError::SmtpTemporary(message).into())
            };
        }

        // Send the actual message data
        let message_data = context
            .data
            .as_ref()
            .ok_or_else(|| SystemError::Internal("No message data to send".to_string()))?;

        let data_str = std::str::from_utf8(message_data.as_ref())
            .map_err(|e| SystemError::Internal(format!("Message data is not valid UTF-8: {e}")))?;

        let send_response = tokio::time::timeout(data_timeout, client.send_data(data_str))
            .await
            .map_err(|_| {
                TemporaryError::Timeout(format!(
                    "Sending message data timed out after {data_timeout:?}"
                ))
            })?
            .map_err(|e| {
                TemporaryError::SmtpTemporary(format!("Failed to send message data: {e}"))
            })?;

        if !send_response.is_success() {
            let code = send_response.code;
            let message = format!("Server rejected message data: {}", send_response.message());
            return if (500..600).contains(&code) {
                Err(PermanentError::MessageRejected(message).into())
            } else {
                Err(TemporaryError::SmtpTemporary(message).into())
            };
        }

        Ok(())
    }

    /// Process all pending messages in the queue
    ///
    /// # Errors
    /// Returns an error if processing fails
    async fn process_queue_internal(
        &self,
        spool: &Arc<dyn empath_spool::BackingStore>,
    ) -> Result<(), DeliveryError> {
        let pending = self.queue.pending_messages().await;

        for info in pending {
            if let Err(e) = self.prepare_message(&info.message_id, spool).await {
                use tracing::error;

                error!(
                    message_id = ?info.message_id,
                    error = %e,
                    "Failed to prepare message for delivery"
                );

                if let Ok(mut context) = spool.read(&info.message_id).await {
                    let server = info
                        .mail_servers
                        .get(info.current_server_index)
                        .map_or_else(|| info.recipient_domain.clone(), MailServer::address);
                    let _error = self
                        .handle_delivery_error(&info.message_id, &mut context, e, server)
                        .await;
                }
            }
        }

        Ok(())
    }
}

/// Extract the email address string from an Address (for SMTP commands)
///
/// For `Single` addresses, returns just the email address.
/// For `Group` addresses, returns `None` as they can't be used in SMTP commands.
fn extract_email_address(address: &empath_common::address::Address) -> Option<String> {
    match &**address {
        mailparse::MailAddr::Single(single_info) => Some(single_info.addr.clone()),
        mailparse::MailAddr::Group(_) => None, // Groups can't be used in SMTP commands
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
        let context = create_test_context("sender@example.org", "recipient@test.example.com");

        // Spool the message
        let msg_id = spool.write(context).await.unwrap();

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
        assert_eq!(info.recipient_domain, "test.example.com");
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

        let msg_id = spool.write(context).await.unwrap();

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
}
