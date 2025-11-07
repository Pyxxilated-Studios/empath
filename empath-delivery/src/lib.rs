//! Delivery queue and processor for handling outbound mail from the spool
//!
//! This module provides functionality to:
//! - Track messages pending delivery
//! - Manage delivery attempts and retries
//! - Prepare messages for sending via SMTP

#![deny(clippy::pedantic, clippy::all, clippy::nursery)]
#![allow(clippy::must_use_candidate)]

use std::{collections::HashMap, sync::Arc, time::Duration};

use empath_common::{Signal, internal, tracing};
use empath_spool::SpooledMessageId;
use empath_tracing::traced;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

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
    /// MX server to use for delivery
    pub mx_server: Option<String>,
}

impl DeliveryInfo {
    /// Create a new pending delivery info
    pub const fn new(message_id: SpooledMessageId, recipient_domain: String) -> Self {
        Self {
            message_id,
            status: DeliveryStatus::Pending,
            attempts: Vec::new(),
            recipient_domain,
            mx_server: None,
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

    /// Set the MX server for a message
    pub async fn set_mx_server(&self, message_id: &SpooledMessageId, mx_server: String) {
        let mut queue = self.queue.write().await;
        if let Some(info) = queue.get_mut(message_id) {
            info.mx_server = Some(mx_server);
        }
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

    /// The spool backing store to read messages from (initialized in `init()`)
    #[serde(skip)]
    spool: Option<Arc<dyn empath_spool::BackingStore>>,

    /// The delivery queue (initialized in `init()`)
    #[serde(skip)]
    queue: DeliveryQueue,
}

impl Default for DeliveryProcessor {
    fn default() -> Self {
        Self {
            scan_interval_secs: default_scan_interval(),
            process_interval_secs: default_process_interval(),
            max_attempts: default_max_attempts(),
            spool: None,
            queue: DeliveryQueue::new(),
        }
    }
}

impl DeliveryProcessor {
    /// Initialize the delivery processor
    ///
    /// # Errors
    ///
    /// Returns an error if the processor cannot be initialized
    pub fn init(&mut self, spool: Arc<dyn empath_spool::BackingStore>) -> anyhow::Result<()> {
        internal!("Initialising Delivery Processor ...");
        self.spool = Some(spool);
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
    ) -> anyhow::Result<()> {
        internal!("Delivery processor starting");

        let Some(spool) = &self.spool else {
            return Err(anyhow::anyhow!(
                "Delivery processor not initialized. Call init() first."
            ));
        };

        let scan_interval = Duration::from_secs(self.scan_interval_secs);
        let process_interval = Duration::from_secs(self.process_interval_secs);

        let mut scan_timer = tokio::time::interval(scan_interval);
        let mut process_timer = tokio::time::interval(process_interval);

        // Skip the first tick to avoid immediate execution
        scan_timer.tick().await;
        process_timer.tick().await;

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
                    match self.process_queue_internal(spool).await {
                        Ok(()) => {
                            tracing::debug!("Processed delivery queue");
                        }
                        Err(e) => {
                            tracing::error!("Error processing delivery queue: {e}");
                        }
                    }
                }
                sig = shutdown.recv() => {
                    match sig {
                        Ok(Signal::Shutdown | Signal::Finalised) => {
                            internal!("Delivery processor shutting down");
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

    /// Scan the spool for new messages and add them to the queue
    ///
    /// # Errors
    /// Returns an error if the spool cannot be read
    async fn scan_spool_internal(
        &self,
        spool: &Arc<dyn empath_spool::BackingStore>,
    ) -> anyhow::Result<usize> {
        let message_ids = spool.list().await?;
        let mut added = 0;

        for msg_id in message_ids {
            // Check if already in queue
            if self.queue.get(&msg_id).await.is_some() {
                continue;
            }

            // Read the message to get recipient domains
            let message = spool.read(&msg_id).await?;

            // Group recipients by domain (handle multi-recipient messages)
            let Some(recipients) = message.envelope.recipients() else {
                use tracing::warn;
                warn!("Message {:?} has no recipients, skipping", msg_id);
                continue;
            };

            // Collect unique domains from all recipients
            let mut domains = std::collections::HashMap::new();
            for recipient in recipients.iter() {
                let recipient_str = recipient.to_string();
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
    /// 2. Determines the recipient domain and MX server
    /// 3. Connects to the MX server via SMTP
    /// 4. Performs EHLO/HELO handshake
    /// 5. Validates MAIL FROM and RCPT TO
    /// 6. Does NOT send DATA (that's for actual delivery)
    ///
    /// # Errors
    /// Returns an error if the message cannot be read or SMTP connection fails
    async fn prepare_message(
        &self,
        message_id: &SpooledMessageId,
        spool: &Arc<dyn empath_spool::BackingStore>,
    ) -> anyhow::Result<()> {
        self.queue
            .update_status(message_id, DeliveryStatus::InProgress)
            .await;

        let _message = spool.read(message_id).await?;
        let info = self
            .queue
            .get(message_id)
            .await
            .ok_or_else(|| anyhow::anyhow!("Message not in queue"))?;

        // Stub MX lookup - in a real implementation, this would do DNS MX queries
        let mx_server = format!("mx.{}", info.recipient_domain);
        let mx_address = format!("{mx_server}:25");
        self.queue
            .set_mx_server(message_id, mx_server.clone())
            .await;

        internal!(
            "Sending message to {:?} with mx host {}",
            message_id,
            mx_address
        );

        // TODO: Actually implement this part at some point
        // let result = self.smtp_handshake(&mx_address, &message).await;

        // match result {
        //     Ok(()) => {
        //         self.queue.update_status(message_id, DeliveryStatus::Pending).await;
        //         Ok(())
        //     }
        //     Err(e) => {
        //         let error = self.handle_delivery_error(message_id, e, mx_server).await;
        //         Err(error)
        //     }
        Ok(())
    }

    /// Handle a failed delivery attempt and update status based on retry policy
    ///
    /// Records the attempt and determines whether to retry or mark as permanently failed.
    ///
    /// # Errors
    /// Returns the original error after recording it
    async fn handle_delivery_error(
        &self,
        message_id: &SpooledMessageId,
        error: anyhow::Error,
        server: String,
    ) -> anyhow::Error {
        use tracing::warn;

        // Record the attempt
        let attempt = DeliveryAttempt {
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            error: Some(error.to_string()),
            server,
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
        error
    }

    /// Perform SMTP handshake and validation (but don't send data)
    ///
    /// # Errors
    /// Returns an error if the SMTP connection or handshake fails
    async fn _smtp_handshake(
        &self,
        server_address: &str,
        message: &empath_spool::Message,
    ) -> anyhow::Result<()> {
        use empath_smtp::client::SmtpClient;

        // Connect to the SMTP server
        let mut client = SmtpClient::connect(server_address, server_address.to_string())
            .await
            .map_err(|e| anyhow::anyhow!("Failed to connect to {server_address}: {e}"))?;

        // Read greeting
        let greeting = client
            .read_greeting()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to read greeting: {e}"))?;

        if !greeting.is_success() {
            return Err(anyhow::anyhow!(
                "Server rejected connection: {}",
                greeting.message()
            ));
        }

        // Send EHLO
        let helo_domain = &message.helo_id;
        let ehlo_response = client
            .ehlo(helo_domain)
            .await
            .map_err(|e| anyhow::anyhow!("EHLO failed: {e}"))?;

        if !ehlo_response.is_success() {
            return Err(anyhow::anyhow!(
                "Server rejected EHLO: {}",
                ehlo_response.message()
            ));
        }

        // Send MAIL FROM
        let sender = message
            .envelope
            .sender()
            .map_or_else(|| "<>".to_string(), std::string::ToString::to_string);

        let mail_response = client
            .mail_from(&sender, None)
            .await
            .map_err(|e| anyhow::anyhow!("MAIL FROM failed: {e}"))?;

        if !mail_response.is_success() {
            return Err(anyhow::anyhow!(
                "Server rejected MAIL FROM: {}",
                mail_response.message()
            ));
        }

        // Send RCPT TO for each recipient
        if let Some(recipients) = message.envelope.recipients() {
            for recipient in recipients.iter() {
                let rcpt_response = client
                    .rcpt_to(&recipient.to_string())
                    .await
                    .map_err(|e| anyhow::anyhow!("RCPT TO failed: {e}"))?;

                if !rcpt_response.is_success() {
                    return Err(anyhow::anyhow!(
                        "Server rejected RCPT TO {}: {}",
                        recipient,
                        rcpt_response.message()
                    ));
                }
            }
        } else {
            return Err(anyhow::anyhow!("No recipients in message"));
        }

        // Send QUIT to cleanly close the connection
        let _ = client.quit().await;

        Ok(())
    }

    /// Process all pending messages in the queue
    ///
    /// # Errors
    /// Returns an error if processing fails
    async fn process_queue_internal(
        &self,
        spool: &Arc<dyn empath_spool::BackingStore>,
    ) -> anyhow::Result<()> {
        let pending = self.queue.pending_messages().await;

        for info in pending {
            if let Err(e) = self.prepare_message(&info.message_id, spool).await {
                use tracing::error;

                error!(
                    message_id = ?info.message_id,
                    error = %e,
                    "Failed to prepare message for delivery"
                );

                // Use extracted method to handle the error
                let server = info.mx_server.clone().unwrap_or_default();
                let _error = self
                    .handle_delivery_error(&info.message_id, e, server)
                    .await;
            }
        }

        Ok(())
    }
}

/// Extract domain from an email address
///
/// # Errors
/// Returns an error if the email address format is invalid or has no domain part
fn extract_domain(email: &str) -> anyhow::Result<String> {
    // Remove angle brackets if present
    let cleaned = email.trim().trim_matches(|c| c == '<' || c == '>');

    // Split on @ and get the domain part
    cleaned
        .split('@')
        .nth(1)
        .map(|domain| domain.trim().to_string())
        .filter(|domain| !domain.is_empty())
        .ok_or_else(|| anyhow::anyhow!("Invalid email address: no domain found in '{email}'"))
}
