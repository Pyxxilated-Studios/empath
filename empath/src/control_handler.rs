//! Control handler implementation for Empath MTA
//!
//! This module implements the `CommandHandler` trait to process control requests
//! for managing the running MTA instance.

use std::{
    collections::HashMap,
    path::PathBuf,
    str::FromStr,
    sync::Arc,
    time::{Duration, Instant, SystemTime},
};

use async_trait::async_trait;
use empath_common::context::Context;
use empath_control::{
    ControlError, DnsCommand, QueueCommand, Request, RequestCommand, Response, SpoolCommand,
    SystemCommand, protocol::ResponseData, server::CommandHandler,
};
use empath_delivery::DeliveryQueryService;
use empath_spool::BackingStore;
use ron::Options;
use tracing_subscriber::filter::LevelFilter;

use crate::controller::Empath;

/// Handler for control commands
pub struct EmpathControlHandler {
    /// Reference to the delivery query service for operations
    delivery: Arc<dyn DeliveryQueryService>,
    /// Server start time for uptime calculation
    start_time: Instant,
}

const DEFAULT_PAGE_SIZE: usize = 100;
const MAX_PAGE_SIZE: usize = 500;

// Emit an audit event with user/uid fields added automatically.
macro_rules! audit {
    ($level:ident, $msg:literal $(, $($field:tt)+ )? $(,)?) => {{
        let identity = crate::control_handler::AuditIdentity::capture();
        tracing::event!(
            tracing::Level::$level,
            user = %identity.user,
            uid = %identity.uid,
            $( $($field)+ ,)?
            $msg
        );
    }};
}

#[derive(Debug)]
struct AuditIdentity {
    user: String,
    uid: String,
}

impl AuditIdentity {
    /// Capture the current user and uid for audit logging.
    fn capture() -> Self {
        let user = std::env::var("USER").unwrap_or_else(|_| "unknown".to_string());

        #[cfg(unix)]
        let uid = unsafe { libc::getuid() }.to_string();
        #[cfg(not(unix))]
        let uid = "N/A".to_string();

        Self { user, uid }
    }
}

fn parse_message_id(message_id: &str) -> empath_control::Result<empath_spool::SpooledMessageId> {
    let mut filename = String::with_capacity(message_id.len() + 4);
    filename.push_str(message_id);
    filename.push_str(".bin");
    empath_spool::SpooledMessageId::from_filename(&filename)
        .ok_or_else(|| ControlError::ServerError(format!("Invalid message ID: {message_id}")))
}

fn reload_config_from_disk(delivery: &Arc<dyn DeliveryQueryService>) -> Result<String, String> {
    let config_path = find_config_file().map_err(|e| e.to_string())?;
    let config_content = std::fs::read_to_string(&config_path)
        .map_err(|e| format!("Failed to read config from {}: {e}", config_path.display()))?;

    let empath: Empath = Options::default()
        .with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME)
        .from_str(&config_content)
        .map_err(|e| format!("Failed to parse config: {e}"))?;

    let (domains, rate_limit) = empath.delivery_reload_settings();

    delivery.replace_domains(domains);
    delivery.replace_rate_limit_config(rate_limit);

    Ok(format!(
        "Configuration reloaded from {}",
        config_path.display()
    ))
}

fn find_config_file() -> anyhow::Result<PathBuf> {
    if let Ok(env_path) = std::env::var("EMPATH_CONFIG") {
        let path = PathBuf::from(env_path);
        if path.exists() {
            return Ok(path);
        }
        anyhow::bail!(
            "EMPATH_CONFIG points to non-existent file: {}",
            path.display()
        );
    }

    let default_paths = vec![
        PathBuf::from("./empath.config.ron"),
        PathBuf::from("/etc/empath/empath.config.ron"),
    ];

    for path in &default_paths {
        if path.exists() {
            return Ok(path.clone());
        }
    }

    let paths_tried = default_paths
        .iter()
        .map(|p| format!("  - {}", p.display()))
        .collect::<Vec<_>>()
        .join("\n");

    anyhow::bail!(
        "No configuration file found. Tried:\n  - EMPATH_CONFIG environment variable\n{paths_tried}"
    )
}

impl EmpathControlHandler {
    /// Create a new control handler
    #[must_use]
    pub fn new(delivery: Arc<dyn DeliveryQueryService>) -> Self {
        Self {
            delivery,
            start_time: Instant::now(),
        }
    }
}

#[async_trait]
impl CommandHandler for EmpathControlHandler {
    async fn handle_request(&self, request: Request) -> empath_control::Result<Response> {
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
            RequestCommand::Spool(spool_cmd) => self.handle_spool_command(spool_cmd).await,
        }
    }
}

impl EmpathControlHandler {
    /// Handle DNS cache management commands
    async fn handle_dns_command(&self, command: DnsCommand) -> empath_control::Result<Response> {
        // Audit log: Record who executed this command
        audit!(INFO, "Control command: DNS", command = ?command);

        let Some(resolver) = self.delivery.dns_resolver() else {
            audit!(
                WARN,
                "DNS command failed: resolver not initialized",
                command = ?command
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
                audit!(INFO, "DNS command completed successfully");
            }
            Err(e) => {
                audit!(WARN, "DNS command failed", error = %e);
            }
        }

        result
    }

    /// Handle system management commands
    fn handle_system_command(&self, command: &SystemCommand) -> empath_control::Result<Response> {
        // Audit log: Record who executed this command
        audit!(INFO, "Control command: System", command = ?command);

        let result = match command {
            SystemCommand::Ping => Ok(Response::ok()),

            SystemCommand::Status => {
                let uptime_secs = self.start_time.elapsed().as_secs();

                let dns_cache_entries = self
                    .delivery
                    .dns_resolver()
                    .as_ref()
                    .map_or(0, |r| r.cache_stats().total_entries);

                let queue_size = self.delivery.queue_len();

                let status = empath_control::protocol::SystemStatus {
                    version: env!("CARGO_PKG_VERSION").to_string(),
                    uptime_secs,
                    queue_size,
                    dns_cache_entries,
                };

                Ok(Response::data(ResponseData::SystemStatus(status)))
            }

            SystemCommand::SetLogLevel { level } => {
                let parsed = LevelFilter::from_str(level.as_str()).map_err(|e| {
                    ControlError::ServerError(format!("Invalid log level '{level}': {e}"))
                })?;
                empath_common::logging::set_log_level(parsed).map_err(|e| {
                    ControlError::ServerError(format!("Failed to set log level: {e}"))
                })?;
                Ok(Response::data(ResponseData::Message(format!(
                    "Log level set to {parsed}"
                ))))
            }

            SystemCommand::ReloadConfig => match reload_config_from_disk(&self.delivery) {
                Ok(message) => Ok(Response::data(ResponseData::Message(message))),
                Err(e) => Err(ControlError::ServerError(e)),
            },

            SystemCommand::ConnectionStats => {
                let stats = self.delivery.connection_stats();
                Ok(Response::data(ResponseData::ConnectionStats(
                    empath_control::protocol::ConnectionStats {
                        queue_size: stats.queue_size,
                        rate_limit_domains: stats.rate_limit_domains,
                    },
                )))
            }

            SystemCommand::SetRateLimit {
                domain,
                messages_per_second,
                burst,
            } => {
                self.delivery
                    .update_rate_limit(domain.clone(), *messages_per_second, *burst)
                    .map_err(ControlError::ServerError)?;
                Ok(Response::data(ResponseData::Message(format!(
                    "Rate limit updated (domain={domain:?}, mps={messages_per_second}, burst={burst})"
                ))))
            }
        };

        // Audit log: Record command result
        match &result {
            Ok(_) => {
                audit!(INFO, "System command completed successfully");
            }
            Err(e) => {
                audit!(WARN, "System command failed", error = %e);
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
        audit!(INFO, "Control command: Queue", command = ?command);

        let Some(spool) = self.delivery.spool() else {
            audit!(
                WARN,
                "Queue command failed: spool not initialized",
                command = ?command
            );
            return Err(ControlError::ServerError(
                "Spool not initialized".to_string(),
            ));
        };

        let result = match command {
            QueueCommand::List {
                status_filter,
                page,
                page_size,
            } => {
                self.handle_list_command(spool, status_filter.as_deref(), page, page_size)
                    .await
            }
            QueueCommand::View {
                message_id,
                include_headers,
                include_body_preview,
            } => {
                self.handle_view_command(
                    spool,
                    message_id.as_ref(),
                    include_headers,
                    include_body_preview,
                )
                .await
            }
            QueueCommand::Retry { message_id, force } => {
                self.handle_retry_command(message_id.as_ref(), force)
            }
            QueueCommand::Delete { message_id } => {
                self.handle_delete_command(spool, message_id.as_ref()).await
            }
            QueueCommand::Stats => Ok(self.handle_stats_command()),
            QueueCommand::ProcessNow => {
                self.delivery.request_process_now();
                audit!(INFO, "Manual queue process trigger received");
                Ok(Response::data(ResponseData::Message(
                    "Triggered immediate queue scan and process".to_string(),
                )))
            }
        };

        // Audit log: Record command result
        match &result {
            Ok(_) => {
                audit!(INFO, "Queue command completed successfully");
            }
            Err(e) => {
                audit!(WARN, "Queue command failed", error = %e);
            }
        }

        result
    }

    /// Handle queue list command
    async fn handle_list_command(
        &self,
        spool: &Arc<dyn BackingStore>,
        status_filter: Option<&str>,
        page: Option<usize>,
        page_size: Option<usize>,
    ) -> empath_control::Result<Response> {
        // Fetch sorted identifiers filtered by status without cloning the full entries
        let ids = self.delivery.list_message_ids(status_filter);
        let total = ids.len();

        let (page, page_size) = Self::resolve_paging(page, page_size);
        let offset = page.saturating_sub(1).saturating_mul(page_size);

        // Convert to protocol types
        let mut messages = Vec::new();
        for message_id in ids.into_iter().skip(offset).take(page_size) {
            let Some(info) = self.delivery.get_message(&message_id) else {
                continue;
            };
            // Read message from spool to get details
            let (context, size) = match spool.read_metadata_with_size(&info.message_id).await {
                Ok(ctx) => ctx,
                Err(e) => {
                    // Log warning but continue - message may have been deleted after queue read
                    tracing::event!(
                        tracing::Level::WARN,
                        message_id = %info.message_id,
                        error = %e,
                        "Failed to read message from spool during queue list - message may have been deleted"
                    );
                    continue;
                }
            };

            let size = size.unwrap_or(0);

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
                next_retry: info.next_retry_at.and_then(|t| {
                    t.duration_since(std::time::UNIX_EPOCH)
                        .ok()
                        .map(|d| d.as_secs())
                }),
                size,
                spooled_at: info.message_id.timestamp_ms() / 1000,
            };
            messages.push(message);
        }

        let returned = messages.len();
        let queue_page = empath_control::protocol::QueueListPage {
            messages,
            total,
            page,
            page_size,
            has_more: offset + returned < total,
        };

        Ok(Response::data(ResponseData::QueueList(queue_page)))
    }

    /// Handle queue view command
    ///
    /// This command can view ANY message in the spool, regardless of whether it's
    /// currently in the delivery queue. If the message is not in the queue, we use
    /// the persisted delivery context from the spool.
    async fn handle_view_command(
        &self,
        spool: &Arc<dyn BackingStore>,
        message_id: &str,
        include_headers: bool,
        include_body_preview: bool,
    ) -> empath_control::Result<Response> {
        // Parse message ID
        let msg_id = parse_message_id(message_id)?;

        // Read message from spool first (this is the source of truth)
        let needs_body = include_headers || include_body_preview;
        let context = if needs_body {
            spool.read(&msg_id).await.map_err(|e| {
                ControlError::ServerError(format!("Message not found in spool: {e}"))
            })?
        } else {
            spool.read_metadata(&msg_id).await.map_err(|e| {
                ControlError::ServerError(format!("Message not found in spool: {e}"))
            })?
        };

        // Try to get delivery info from queue (optional - message may have been delivered already)
        let info = self.delivery.get_message(&msg_id);

        // Extract headers
        let headers = if include_headers {
            Some(Self::extract_headers(&context))
        } else {
            None
        };

        // Extract body preview
        let body_preview = if include_body_preview {
            Some(Self::extract_body_preview(&context))
        } else {
            None
        };

        let size = match spool.message_size(&msg_id).await {
            Ok(Some(size)) => size,
            Ok(None) => 0,
            Err(e) => {
                tracing::event!(
                    tracing::Level::WARN,
                    message_id = %msg_id,
                    error = %e,
                    "Failed to read message size from spool during view"
                );
                0
            }
        };

        // Build details from either queue info or persisted delivery context
        let details = Self::build_message_details(
            message_id.to_string(),
            &msg_id,
            &context,
            size,
            headers,
            body_preview,
            info,
        );

        Ok(Response::data(ResponseData::QueueMessageDetails(details)))
    }

    /// Handle queue retry command
    fn handle_retry_command(
        &self,
        message_id: &str,
        force: bool,
    ) -> empath_control::Result<Response> {
        // Parse message ID
        let msg_id = parse_message_id(message_id)?;

        // Get delivery info from queue
        let info = self.delivery.get_message(&msg_id).ok_or_else(|| {
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
        self.delivery
            .update_status(&msg_id, empath_common::DeliveryStatus::Pending);
        self.delivery.reset_server_index(&msg_id);
        // Set next_retry to now for immediate retry
        self.delivery
            .set_next_retry_at(&msg_id, std::time::SystemTime::UNIX_EPOCH);

        Ok(Response::data(ResponseData::Message(format!(
            "Message {message_id} scheduled for retry"
        ))))
    }

    /// Handle queue delete command
    async fn handle_delete_command(
        &self,
        spool: &Arc<dyn BackingStore>,
        message_id: &str,
    ) -> empath_control::Result<Response> {
        // Parse message ID
        let msg_id = parse_message_id(message_id)?;

        // Remove from queue
        self.delivery.remove(&msg_id).ok_or_else(|| {
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
    fn handle_stats_command(&self) -> Response {
        let snapshot = self.delivery.queue_stats_snapshot();

        let stats = empath_control::protocol::QueueStats {
            total: snapshot.total,
            by_status: snapshot.by_status,
            by_domain: snapshot.by_domain,
            oldest_message_age_secs: snapshot.oldest_message_age_secs,
        };

        Response::data(ResponseData::QueueStats(stats))
    }

    fn resolve_paging(page: Option<usize>, page_size: Option<usize>) -> (usize, usize) {
        let page_size = page_size
            .unwrap_or(DEFAULT_PAGE_SIZE)
            .clamp(1, MAX_PAGE_SIZE);
        let page = page.unwrap_or(1).max(1);
        (page, page_size)
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

    /// Handle spool management commands
    async fn handle_spool_command(
        &self,
        command: SpoolCommand,
    ) -> empath_control::Result<Response> {
        // Audit log: Record who executed this command
        audit!(INFO, "Control command: Spool", command = ?command);

        let Some(spool) = self.delivery.spool() else {
            audit!(
                WARN,
                "Spool command failed: spool not initialized",
                command = ?command
            );
            return Err(ControlError::ServerError(
                "Spool not initialized".to_string(),
            ));
        };

        let result = match command {
            SpoolCommand::List {
                status_filter,
                page,
                page_size,
            } => {
                self.handle_spool_list_command(spool, status_filter.as_deref(), page, page_size)
                    .await
            }
            SpoolCommand::View {
                message_id,
                include_headers,
                include_body_preview,
            } => {
                // Reuse queue view logic but directly from spool
                self.handle_view_command(
                    spool,
                    message_id.as_ref(),
                    include_headers,
                    include_body_preview,
                )
                .await
            }
            SpoolCommand::CleanupCompleted => self.handle_spool_cleanup_command(spool).await,
            SpoolCommand::Stats => self.handle_spool_stats_command(spool).await,
        };

        // Audit log: Record command result
        match &result {
            Ok(_) => {
                audit!(INFO, "Spool command completed successfully");
            }
            Err(e) => {
                audit!(WARN, "Spool command failed", error = %e);
            }
        }

        result
    }

    /// Handle spool list command - list ALL messages in spool
    async fn handle_spool_list_command(
        &self,
        spool: &Arc<dyn BackingStore>,
        status_filter: Option<&str>,
        page: Option<usize>,
        page_size: Option<usize>,
    ) -> empath_control::Result<Response> {
        // List all messages in spool
        let message_ids = spool
            .list()
            .await
            .map_err(|e| ControlError::ServerError(format!("Failed to list spool: {e}")))?;

        let (page, page_size) = Self::resolve_paging(page, page_size);
        let offset = page.saturating_sub(1).saturating_mul(page_size);
        let mut matched = 0usize;
        let mut messages = Vec::new();
        for msg_id in message_ids {
            // Read each message
            let (context, size) = match spool.read_metadata_with_size(&msg_id).await {
                Ok(ctx) => ctx,
                Err(e) => {
                    tracing::event!(
                        tracing::Level::WARN,
                        message_id = %msg_id,
                        error = %e,
                        "Failed to read message from spool during spool list"
                    );
                    continue;
                }
            };
            let size = size.unwrap_or(0);

            // Extract status from delivery context (if present)
            let (status, domain, attempts) =
                context
                    .delivery
                    .as_ref()
                    .map_or((None, None, None), |delivery| {
                        (
                            Some(format!("{:?}", delivery.status)),
                            Some(delivery.domain.to_string()),
                            Some(u32::try_from(delivery.attempt_history.len()).unwrap_or_default()),
                        )
                    });

            // Apply status filter if specified
            if let Some(filter) = status_filter {
                if let Some(ref msg_status) = status {
                    if !msg_status.contains(filter) {
                        continue;
                    }
                } else if filter != "new" && filter != "no_delivery_context" {
                    // Message has no delivery context, only show if filter is "new" or "no_delivery_context"
                    continue;
                }
            }

            if matched >= offset && messages.len() < page_size {
                let message = empath_control::protocol::SpoolMessage {
                    id: msg_id.to_string(),
                    from: context
                        .envelope
                        .sender()
                        .map_or_else(|| "<>".to_string(), ToString::to_string),
                    to: context.envelope.recipients().map_or_else(Vec::new, |list| {
                        list.iter().map(std::string::ToString::to_string).collect()
                    }),
                    status,
                    domain,
                    attempts,
                    size,
                    spooled_at: msg_id.timestamp_ms() / 1000,
                };
                messages.push(message);
            }
            matched += 1;
        }

        let returned = messages.len();
        let spool_page = empath_control::protocol::SpoolListPage {
            messages,
            total: matched,
            page,
            page_size,
            has_more: offset + returned < matched,
        };

        Ok(Response::data(ResponseData::SpoolList(spool_page)))
    }

    /// Handle spool stats command
    async fn handle_spool_stats_command(
        &self,
        spool: &Arc<dyn BackingStore>,
    ) -> empath_control::Result<Response> {
        // List all messages in spool
        let message_ids = spool
            .list()
            .await
            .map_err(|e| ControlError::ServerError(format!("Failed to list spool: {e}")))?;

        let mut total = 0;
        let mut total_size: u64 = 0;
        let mut by_status: HashMap<String, usize> = HashMap::new();
        let mut oldest_age: Option<u64> = None;

        for msg_id in &message_ids {
            total += 1;

            // Read message to get status and size
            let context = match spool.read_metadata(msg_id).await {
                Ok(ctx) => ctx,
                Err(e) => {
                    tracing::event!(
                        tracing::Level::WARN,
                        message_id = %msg_id,
                        error = %e,
                        "Failed to read message from spool during stats"
                    );
                    continue;
                }
            };

            // Count by status
            let status = context.delivery.as_ref().map_or_else(
                || "NoDeliveryContext".to_string(),
                |delivery| format!("{:?}", delivery.status),
            );
            *by_status.entry(status).or_insert(0) += 1;

            // Add to total size
            let size = match spool.message_size(msg_id).await {
                Ok(Some(len)) => len,
                Ok(None) => 0,
                Err(e) => {
                    tracing::event!(
                        tracing::Level::WARN,
                        message_id = %msg_id,
                        error = %e,
                        "Failed to read message size from spool during stats"
                    );
                    0
                }
            };
            total_size = total_size.saturating_add(u64::try_from(size).unwrap_or(u64::MAX));

            // Track oldest message
            let age_secs = Self::age_secs_from_timestamp(msg_id.timestamp_ms());
            oldest_age = Some(oldest_age.map_or(age_secs, |current| current.max(age_secs)));
        }

        let stats = empath_control::protocol::SpoolStats {
            total,
            by_status,
            total_size,
            oldest_message_age_secs: oldest_age,
        };

        Ok(Response::data(ResponseData::SpoolStats(stats)))
    }

    /// Handle spool cleanup command - delete completed/failed messages
    async fn handle_spool_cleanup_command(
        &self,
        spool: &Arc<dyn BackingStore>,
    ) -> empath_control::Result<Response> {
        // List all messages in spool
        let message_ids = spool
            .list()
            .await
            .map_err(|e| ControlError::ServerError(format!("Failed to list spool: {e}")))?;

        let mut deleted_count = 0;
        let mut failed_deletions = Vec::new();

        for msg_id in message_ids {
            // Read message to check status
            let context = match spool.read_metadata(&msg_id).await {
                Ok(ctx) => ctx,
                Err(e) => {
                    tracing::event!(
                        tracing::Level::WARN,
                        message_id = %msg_id,
                        error = %e,
                        "Failed to read message from spool during cleanup"
                    );
                    continue;
                }
            };

            // Delete if in terminal state
            if let Some(ref delivery) = context.delivery
                && matches!(
                    delivery.status,
                    empath_common::DeliveryStatus::Completed
                        | empath_common::DeliveryStatus::Failed(_)
                )
            {
                match spool.delete(&msg_id).await {
                    Ok(()) => {
                        // Also remove from queue if present
                        self.delivery.remove(&msg_id);
                        deleted_count += 1;
                    }
                    Err(e) => {
                        tracing::event!(
                            tracing::Level::WARN,
                            message_id = %msg_id,
                            error = %e,
                            "Failed to delete completed message from spool"
                        );
                        failed_deletions.push(msg_id.to_string());
                    }
                }
            }
        }

        let message = if failed_deletions.is_empty() {
            format!("Cleaned up {deleted_count} completed/failed message(s)")
        } else {
            format!(
                "Cleaned up {deleted_count} message(s), failed to delete {}: {}",
                failed_deletions.len(),
                failed_deletions.join(", ")
            )
        };

        Ok(Response::data(ResponseData::Message(message)))
    }

    /// Calculate age in seconds from a ULID timestamp, saturating at zero on clock skew
    fn age_secs_from_timestamp(timestamp_ms: u64) -> u64 {
        let spooled_at = SystemTime::UNIX_EPOCH + Duration::from_millis(timestamp_ms);
        SystemTime::now()
            .duration_since(spooled_at)
            .unwrap_or(Duration::ZERO)
            .as_secs()
    }

    #[allow(clippy::too_many_arguments)]
    fn build_message_details(
        message_id: String,
        msg_id: &empath_spool::SpooledMessageId,
        context: &Context,
        size: usize,
        headers: Option<HashMap<String, String>>,
        body_preview: Option<String>,
        queue_info: Option<empath_delivery::DeliveryInfo>,
    ) -> empath_control::protocol::QueueMessageDetails {
        let from = context
            .envelope
            .sender()
            .map_or_else(|| "<>".to_string(), ToString::to_string);
        let to = context.envelope.recipients().map_or_else(Vec::new, |list| {
            list.iter().map(std::string::ToString::to_string).collect()
        });
        let spooled_at = msg_id.timestamp_ms() / 1000;

        if let Some(queue_info) = queue_info {
            return empath_control::protocol::QueueMessageDetails {
                id: message_id,
                from,
                to,
                domain: queue_info.recipient_domain.to_string(),
                status: format!("{:?}", queue_info.status),
                attempts: u32::try_from(queue_info.attempts.len()).unwrap_or_default(),
                next_retry: queue_info.next_retry_at.and_then(|t| {
                    t.duration_since(std::time::UNIX_EPOCH)
                        .ok()
                        .map(|d| d.as_secs())
                }),
                last_error: queue_info.attempts.last().and_then(|a| a.error.clone()),
                size,
                spooled_at,
                headers,
                body_preview,
            };
        }

        if let Some(ref delivery_ctx) = context.delivery {
            return empath_control::protocol::QueueMessageDetails {
                id: message_id,
                from,
                to,
                domain: delivery_ctx.domain.to_string(),
                status: format!("{:?}", delivery_ctx.status),
                attempts: u32::try_from(delivery_ctx.attempt_history.len()).unwrap_or_default(),
                next_retry: delivery_ctx.next_retry_at.and_then(|t| {
                    t.duration_since(std::time::UNIX_EPOCH)
                        .ok()
                        .map(|d| d.as_secs())
                }),
                last_error: delivery_ctx
                    .attempt_history
                    .last()
                    .and_then(|a| a.error.clone()),
                size,
                spooled_at,
                headers,
                body_preview,
            };
        }

        empath_control::protocol::QueueMessageDetails {
            id: message_id,
            from,
            to,
            domain: "unknown".to_string(),
            status: "Unknown".to_string(),
            attempts: 0,
            next_retry: None,
            last_error: None,
            size,
            spooled_at,
            headers,
            body_preview,
        }
    }
}
