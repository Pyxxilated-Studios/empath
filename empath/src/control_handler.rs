//! Control handler implementation for Empath MTA
//!
//! This module implements the `CommandHandler` trait to process control requests
//! for managing the running MTA instance.

use std::{collections::HashMap, future::Future, pin::Pin, sync::Arc, time::Instant};

use empath_common::{context::Context, internal};
use empath_control::{
    ControlError, DnsCommand, QueueCommand, Request, RequestCommand, Response, SystemCommand,
    protocol::ResponseData, server::CommandHandler,
};
use empath_delivery::DeliveryProcessor;
use empath_spool::BackingStore;

/// Handler for control commands
pub struct EmpathControlHandler {
    /// Reference to the delivery processor for DNS operations
    delivery: Arc<DeliveryProcessor>,
    /// Server start time for uptime calculation
    start_time: Instant,
}

impl EmpathControlHandler {
    /// Create a new control handler
    #[must_use]
    pub fn new(delivery: Arc<DeliveryProcessor>) -> Self {
        Self {
            delivery,
            start_time: Instant::now(),
        }
    }
}

impl CommandHandler for EmpathControlHandler {
    fn handle_request(
        &self,
        request: Request,
    ) -> Pin<Box<dyn Future<Output = empath_control::Result<Response>> + Send + '_>> {
        Box::pin(async move {
            // Validate protocol version
            if !request.is_version_compatible() {
                return Err(ControlError::ServerError(format!(
                    "Incompatible protocol version: client={}, server={}",
                    request.version,
                    empath_control::PROTOCOL_VERSION
                )));
            }

            match request.command {
                RequestCommand::Dns(dns_cmd) => self.handle_dns_command(dns_cmd).await,
                RequestCommand::System(sys_cmd) => self.handle_system_command(&sys_cmd),
                RequestCommand::Queue(queue_cmd) => self.handle_queue_command(queue_cmd).await,
            }
        })
    }
}

impl EmpathControlHandler {
    /// Handle DNS cache management commands
    async fn handle_dns_command(&self, command: DnsCommand) -> empath_control::Result<Response> {
        // Audit log: Record who executed this command
        #[cfg(unix)]
        let uid = unsafe { libc::getuid() };
        #[cfg(not(unix))]
        let uid = "N/A";

        let user = std::env::var("USER").unwrap_or_else(|_| "unknown".to_string());

        tracing::event!(
            tracing::Level::INFO,
            user = %user,
            uid = %uid,
            command = ?command,
            "Control command: DNS"
        );

        let Some(resolver) = self.delivery.dns_resolver() else {
            tracing::event!(tracing::Level::WARN,
                user = %user,
                uid = %uid,
                command = ?command,
                "DNS command failed: resolver not initialized"
            );
            return Err(ControlError::ServerError(
                "DNS resolver not initialized".to_string(),
            ));
        };

        let result = match command {
            DnsCommand::ListCache => {
                let cache = resolver.list_cache();

                // Convert to control protocol types
                let cache_data: HashMap<String, Vec<empath_control::protocol::CachedMailServer>> =
                    cache
                        .into_iter()
                        .map(|(domain, servers)| {
                            let servers = servers
                                .into_iter()
                                .map(|(server, ttl)| empath_control::protocol::CachedMailServer {
                                    host: server.host,
                                    priority: server.priority,
                                    port: server.port,
                                    ttl_remaining_secs: ttl.as_secs(),
                                })
                                .collect();
                            (domain, servers)
                        })
                        .collect();

                Ok(Response::data(ResponseData::DnsCache(cache_data)))
            }

            DnsCommand::ClearCache => {
                resolver.clear_cache();
                Ok(Response::ok())
            }

            DnsCommand::RefreshDomain(domain) => match resolver.refresh_domain(&domain).await {
                Ok(servers) => {
                    let message = format!(
                        "Refreshed DNS for {domain}: {} mail server(s)",
                        servers.len()
                    );
                    Ok(Response::data(ResponseData::Message(message)))
                }
                Err(e) => Err(ControlError::ServerError(format!(
                    "Failed to refresh domain {domain}: {e}"
                ))),
            },

            DnsCommand::SetOverride { domain, mx_server } => {
                // Update domain config registry
                self.update_mx_override(&domain, Some(&mx_server));

                let message = format!("Set MX override for {domain} -> {mx_server}");
                Ok(Response::data(ResponseData::Message(message)))
            }

            DnsCommand::RemoveOverride(domain) => {
                self.update_mx_override(&domain, None);

                let message = format!("Removed MX override for {domain}");
                Ok(Response::data(ResponseData::Message(message)))
            }

            DnsCommand::ListOverrides => {
                let overrides = self.list_mx_overrides();
                Ok(Response::data(ResponseData::MxOverrides(overrides)))
            }
        };

        // Audit log: Record command result
        match &result {
            Ok(_) => {
                tracing::event!(tracing::Level::INFO,
                    user = %user,
                    uid = %uid,
                    "DNS command completed successfully"
                );
            }
            Err(e) => {
                tracing::event!(tracing::Level::WARN,
                    user = %user,
                    uid = %uid,
                    error = %e,
                    "DNS command failed"
                );
            }
        }

        result
    }

    /// Handle system management commands
    fn handle_system_command(&self, command: &SystemCommand) -> empath_control::Result<Response> {
        // Audit log: Record who executed this command
        #[cfg(unix)]
        let uid = unsafe { libc::getuid() };
        #[cfg(not(unix))]
        let uid = "N/A";

        let user = std::env::var("USER").unwrap_or_else(|_| "unknown".to_string());

        tracing::event!(tracing::Level::INFO,
            user = %user,
            uid = %uid,
            command = ?command,
            "Control command: System"
        );

        let result = match command {
            SystemCommand::Ping => Ok(Response::ok()),

            SystemCommand::Status => {
                let uptime_secs = self.start_time.elapsed().as_secs();

                let dns_cache_entries = self
                    .delivery
                    .dns_resolver()
                    .as_ref()
                    .map_or(0, |r| r.cache_stats().total_entries);

                let queue_size = self.delivery.queue().len();

                let status = empath_control::protocol::SystemStatus {
                    version: env!("CARGO_PKG_VERSION").to_string(),
                    uptime_secs,
                    queue_size,
                    dns_cache_entries,
                };

                Ok(Response::data(ResponseData::SystemStatus(status)))
            }
        };

        // Audit log: Record command result
        match &result {
            Ok(_) => {
                tracing::event!(tracing::Level::INFO,
                    user = %user,
                    uid = %uid,
                    "System command completed successfully"
                );
            }
            Err(e) => {
                tracing::event!(tracing::Level::WARN,
                    user = %user,
                    uid = %uid,
                    error = %e,
                    "System command failed"
                );
            }
        }

        result
    }

    /// Handle queue management commands
    async fn handle_queue_command(
        &self,
        command: QueueCommand,
    ) -> empath_control::Result<Response> {
        // Audit log: Record who executed this command
        #[cfg(unix)]
        let uid = unsafe { libc::getuid() };
        #[cfg(not(unix))]
        let uid = "N/A";

        let user = std::env::var("USER").unwrap_or_else(|_| "unknown".to_string());

        tracing::event!(tracing::Level::INFO,
            user = %user,
            uid = %uid,
            command = ?command,
            "Control command: Queue"
        );

        let Some(spool) = self.delivery.spool() else {
            tracing::event!(tracing::Level::WARN,
                user = %user,
                uid = %uid,
                command = ?command,
                "Queue command failed: spool not initialized"
            );
            return Err(ControlError::ServerError(
                "Spool not initialized".to_string(),
            ));
        };

        let queue = self.delivery.queue();

        let result = match command {
            QueueCommand::List { status_filter } => {
                self.handle_list_command(spool, queue, status_filter).await
            }
            QueueCommand::View { message_id } => {
                self.handle_view_command(spool, queue, message_id).await
            }
            QueueCommand::Retry { message_id, force } => {
                Self::handle_retry_command(queue, &message_id, force)
            }
            QueueCommand::Delete { message_id } => {
                self.handle_delete_command(spool, queue, message_id).await
            }
            QueueCommand::Stats => Ok(Self::handle_stats_command(queue)),
        };

        // Audit log: Record command result
        match &result {
            Ok(_) => {
                tracing::event!(tracing::Level::INFO,
                    user = %user,
                    uid = %uid,
                    "Queue command completed successfully"
                );
            }
            Err(e) => {
                tracing::event!(tracing::Level::WARN,
                    user = %user,
                    uid = %uid,
                    error = %e,
                    "Queue command failed"
                );
            }
        }

        result
    }

    /// Handle queue list command
    async fn handle_list_command(
        &self,
        spool: &Arc<dyn BackingStore>,
        queue: &empath_delivery::DeliveryQueue,
        status_filter: Option<String>,
    ) -> empath_control::Result<Response> {
        // Get all messages from queue
        let all_info = queue.all_messages();

        // Filter by status if requested
        let filtered_info: Vec<_> = if let Some(status) = status_filter {
            all_info
                .into_iter()
                .filter(|info| info.status.matches_filter(&status))
                .collect()
        } else {
            all_info
        };
        internal!(level = TRACE, "{filtered_info:?}");

        // Convert to protocol types
        let mut messages = Vec::new();
        for info in filtered_info {
            // Read message from spool to get details
            let Ok(context) = spool.read(&info.message_id).await else {
                continue; // Skip messages that can't be read
            };

            let message = empath_control::protocol::QueueMessage {
                id: info.message_id.to_string(),
                from: context
                    .envelope
                    .sender()
                    .map_or_else(|| "<>".to_string(), ToString::to_string),
                to: context.envelope.recipients().map_or_else(Vec::new, |list| {
                    list.iter().map(std::string::ToString::to_string).collect()
                }),
                domain: info.recipient_domain.to_string(),
                status: info.status.to_string(),
                attempts: u32::try_from(info.attempts.len()).unwrap_or_default(),
                next_retry: info.next_retry_at,
                size: context.data.as_ref().map_or(0, |d| d.len()),
                spooled_at: info.message_id.timestamp_ms() / 1000,
            };
            messages.push(message);
        }

        Ok(Response::data(ResponseData::QueueList(messages)))
    }

    /// Handle queue view command
    async fn handle_view_command(
        &self,
        spool: &Arc<dyn BackingStore>,
        queue: &empath_delivery::DeliveryQueue,
        message_id: String,
    ) -> empath_control::Result<Response> {
        // Parse message ID
        let msg_id = empath_spool::SpooledMessageId::from_filename(&format!("{message_id}.bin"))
            .ok_or_else(|| {
                ControlError::ServerError(format!("Invalid message ID: {message_id}"))
            })?;

        // Get delivery info from queue
        let info = queue.get(&msg_id).ok_or_else(|| {
            ControlError::ServerError(format!("Message not found in queue: {message_id}"))
        })?;

        // Read message from spool
        let context = spool
            .read(&msg_id)
            .await
            .map_err(|e| ControlError::ServerError(format!("Failed to read message: {e}")))?;

        // Extract headers
        let headers = Self::extract_headers(&context);

        // Extract body preview
        let body_preview = Self::extract_body_preview(&context);

        let details = empath_control::protocol::QueueMessageDetails {
            id: message_id,
            from: context
                .envelope
                .sender()
                .map_or_else(|| "<>".to_string(), ToString::to_string),
            to: context.envelope.recipients().map_or_else(Vec::new, |list| {
                list.iter().map(std::string::ToString::to_string).collect()
            }),
            domain: info.recipient_domain.to_string(),
            status: format!("{:?}", info.status),
            attempts: u32::try_from(info.attempts.len()).unwrap_or_default(),
            next_retry: info.next_retry_at,
            last_error: info.attempts.last().and_then(|a| a.error.clone()),
            size: context.data.as_ref().map_or(0, |d| d.len()),
            spooled_at: msg_id.timestamp_ms() / 1000,
            headers,
            body_preview,
        };

        Ok(Response::data(ResponseData::QueueMessageDetails(details)))
    }

    /// Handle queue retry command
    fn handle_retry_command(
        queue: &empath_delivery::DeliveryQueue,
        message_id: &String,
        force: bool,
    ) -> empath_control::Result<Response> {
        // Parse message ID
        let msg_id = empath_spool::SpooledMessageId::from_filename(&format!("{message_id}.bin"))
            .ok_or_else(|| {
                ControlError::ServerError(format!("Invalid message ID: {message_id}"))
            })?;

        // Get delivery info from queue
        let info = queue.get(&msg_id).ok_or_else(|| {
            ControlError::ServerError(format!("Message not found in queue: {message_id}"))
        })?;

        // Check if message can be retried
        if !force && !matches!(info.status, empath_common::DeliveryStatus::Failed(_)) {
            return Err(ControlError::ServerError(format!(
                "Message is not in failed status (current: {:?}). Use --force to retry anyway.",
                info.status
            )));
        }

        // Reset status to pending
        queue.update_status(&msg_id, empath_common::DeliveryStatus::Pending);
        queue.reset_server_index(&msg_id);
        queue.set_next_retry_at(&msg_id, 0);

        Ok(Response::data(ResponseData::Message(format!(
            "Message {message_id} scheduled for retry"
        ))))
    }

    /// Handle queue delete command
    async fn handle_delete_command(
        &self,
        spool: &Arc<dyn BackingStore>,
        queue: &empath_delivery::DeliveryQueue,
        message_id: String,
    ) -> empath_control::Result<Response> {
        // Parse message ID
        let msg_id = empath_spool::SpooledMessageId::from_filename(&format!("{message_id}.bin"))
            .ok_or_else(|| {
                ControlError::ServerError(format!("Invalid message ID: {message_id}"))
            })?;

        // Remove from queue
        queue.remove(&msg_id).ok_or_else(|| {
            ControlError::ServerError(format!("Message not found in queue: {message_id}"))
        })?;

        // Delete from spool
        spool.delete(&msg_id).await.map_err(|e| {
            ControlError::ServerError(format!("Failed to delete message from spool: {e}"))
        })?;

        Ok(Response::data(ResponseData::Message(format!(
            "Message {message_id} deleted"
        ))))
    }

    /// Handle queue stats command
    fn handle_stats_command(queue: &empath_delivery::DeliveryQueue) -> Response {
        // Get all messages
        let all_info = queue.all_messages();

        // Count by status
        let mut by_status: HashMap<String, usize> = HashMap::new();
        for info in &all_info {
            *by_status.entry(format!("{:?}", info.status)).or_insert(0) += 1;
        }

        // Count by domain
        let mut by_domain: HashMap<String, usize> = HashMap::new();
        for info in &all_info {
            *by_domain
                .entry(info.recipient_domain.to_string())
                .or_insert(0) += 1;
        }

        // Find oldest message
        let oldest_age = all_info
            .iter()
            .map(|info| {
                let now_ms = u64::try_from(
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis(),
                )
                .unwrap_or_default();
                let spooled_ms = info.message_id.timestamp_ms();
                (now_ms - spooled_ms) / 1000
            })
            .max();

        let stats = empath_control::protocol::QueueStats {
            total: all_info.len(),
            by_status,
            by_domain,
            oldest_message_age_secs: oldest_age,
        };

        Response::data(ResponseData::QueueStats(stats))
    }

    /// Extract email headers from message data
    fn extract_headers(context: &Context) -> HashMap<String, String> {
        let mut headers = HashMap::new();
        if let Some(data) = &context.data
            && let Ok(data_str) = std::str::from_utf8(data.as_ref())
        {
            // Parse headers (very basic)
            for line in data_str.lines() {
                if line.is_empty() {
                    break;
                }
                if let Some((key, value)) = line.split_once(':') {
                    headers.insert(key.trim().to_string(), value.trim().to_string());
                }
            }
        }
        headers
    }

    /// Extract body preview from message data
    fn extract_body_preview(context: &Context) -> String {
        context.data.as_ref().map_or_else(
            || "[No data]".to_string(),
            |data| {
                std::str::from_utf8(data.as_ref()).map_or_else(
                    |_| "[Binary data]".to_string(),
                    |data_str| {
                        data_str
                            .find("\r\n\r\n")
                            .or_else(|| data_str.find("\n\n"))
                            .map_or_else(
                                || data_str.chars().take(1024).collect(),
                                |body_start| {
                                    let offset = if data_str[body_start..].starts_with("\r\n\r\n") {
                                        4
                                    } else {
                                        2
                                    };
                                    let body = &data_str[body_start + offset..];
                                    body.chars().take(1024).collect()
                                },
                            )
                    },
                )
            },
        )
    }

    /// Update MX override in domain configuration
    ///
    /// Note: This is a runtime-only change and does not persist across restarts.
    /// To make overrides permanent, update the configuration file.
    fn update_mx_override(&self, domain: &str, mx_override: Option<&String>) {
        // Access the domain registry
        let registry = self.delivery.domains();

        // Get existing configuration or create a default
        let config = registry
            .get(domain)
            .map(|entry| entry.value().clone())
            .unwrap_or_default();

        // Update MX override
        let mut updated_config = config;
        updated_config.mx_override = mx_override.cloned();

        // Insert updated configuration (DomainConfigRegistry now has interior mutability)
        registry.insert(domain.to_string(), updated_config);

        tracing::event!(
            tracing::Level::INFO,
            domain = %domain,
            mx_override = ?mx_override,
            "Updated MX override for domain at runtime"
        );
    }

    /// List all configured MX overrides
    fn list_mx_overrides(&self) -> HashMap<String, String> {
        let registry = self.delivery.domains();
        let mut overrides = HashMap::new();

        for (domain, config) in registry.iter() {
            if let Some(ref mx_override) = config.mx_override {
                overrides.insert(domain.clone(), mx_override.clone());
            }
        }

        overrides
    }
}
