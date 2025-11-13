//! Command-line utility for managing the Empath MTA
//!
//! This tool provides operational control over the MTA, including:
//! - Queue management (list, view, retry, delete messages)
//! - DNS cache management (list, clear, refresh)
//! - System status and health checks
//! - Freezing/unfreezing the queue
//! - Viewing statistics

#![allow(
    clippy::items_after_statements,
    clippy::single_match_else,
    clippy::case_sensitive_file_extension_comparisons
)]

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};
use empath_control::{DEFAULT_CONTROL_SOCKET, ControlClient, Request, DnsCommand, SystemCommand};

/// Command-line utility for managing the Empath MTA
#[derive(Parser, Debug)]
#[command(name = "empathctl")]
#[command(about = "Manage the Empath MTA", long_about = None)]
#[command(version)]
struct Cli {
    /// Path to the spool directory (for queue commands)
    #[arg(short, long, default_value = "/tmp/spool/empath")]
    spool_path: PathBuf,

    /// Path to the queue state file (bincode format)
    #[arg(short, long)]
    queue_state: Option<PathBuf>,

    /// Path to the control socket (for control commands)
    #[arg(short = 'c', long, default_value = DEFAULT_CONTROL_SOCKET)]
    control_socket: String,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Queue management commands (file-based)
    Queue {
        #[command(subcommand)]
        action: QueueAction,
    },
    /// DNS cache management (runtime control via socket)
    Dns {
        #[command(subcommand)]
        action: DnsAction,
    },
    /// System status and health (runtime control via socket)
    System {
        #[command(subcommand)]
        action: SystemAction,
    },
}

#[derive(Subcommand, Debug)]
enum DnsAction {
    /// List all cached DNS entries
    ListCache,
    /// Clear the entire DNS cache
    ClearCache,
    /// Refresh DNS records for a specific domain
    Refresh {
        /// Domain to refresh
        domain: String,
    },
    /// List configured MX overrides
    ListOverrides,
    /// Set MX override for a domain (runtime only, not persisted)
    SetOverride {
        /// Domain to override
        domain: String,
        /// Mail server (host:port) to use
        server: String,
    },
    /// Remove MX override for a domain
    RemoveOverride {
        /// Domain to remove override for
        domain: String,
    },
}

#[derive(Subcommand, Debug)]
enum SystemAction {
    /// Check if the MTA is responding
    Ping,
    /// Get system status and statistics
    Status,
}

#[derive(Subcommand, Debug)]
enum QueueAction {
    /// List messages in the queue
    List {
        /// Filter by status
        #[arg(long, value_enum)]
        status: Option<StatusFilter>,

        /// Output format (text, json)
        #[arg(long, default_value = "text")]
        format: String,
    },
    /// View detailed information about a specific message
    View {
        /// Message ID to view
        message_id: String,
    },
    /// Retry delivery of a message
    Retry {
        /// Message ID to retry
        message_id: String,

        /// Force retry even if not failed
        #[arg(long)]
        force: bool,
    },
    /// Delete a message from the queue and spool
    Delete {
        /// Message ID to delete
        message_id: String,

        /// Skip confirmation prompt
        #[arg(long)]
        yes: bool,
    },
    /// Freeze the delivery queue (pause processing)
    Freeze,
    /// Unfreeze the delivery queue (resume processing)
    Unfreeze,
    /// Show queue statistics
    Stats {
        /// Watch mode - continuously update statistics
        #[arg(long)]
        watch: bool,

        /// Update interval in seconds (for watch mode)
        #[arg(long, default_value = "2")]
        interval: u64,
    },
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
enum StatusFilter {
    Pending,
    InProgress,
    Completed,
    Failed,
    Retry,
    Expired,
}

static PENDING_STR: &str = "Pending";
static IN_PROGRESS_STR: &str = "In Progress";
static COMPLETED_STR: &str = "Completed";
static FAILED_STR: &str = "Failed";
static RETRY_STR: &str = "Retry";
static EXPIRED_STR: &str = "Expired";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing/logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();

    // Derive queue state path if not provided
    // Place it in the same directory as the spool
    let queue_state_path = cli
        .queue_state
        .unwrap_or_else(|| cli.spool_path.join("queue_state.bin"));

    match cli.command {
        Commands::Queue { action } => match action {
            QueueAction::List { status, format } => {
                cmd_list(&cli.spool_path, &queue_state_path, status, &format).await?;
            }
            QueueAction::View { message_id } => {
                cmd_view(&cli.spool_path, &queue_state_path, &message_id).await?;
            }
            QueueAction::Retry { message_id, force } => {
                cmd_retry(&cli.spool_path, &queue_state_path, &message_id, force).await?;
            }
            QueueAction::Delete { message_id, yes } => {
                cmd_delete(&cli.spool_path, &queue_state_path, &message_id, yes).await?;
            }
            QueueAction::Freeze => {
                cmd_freeze(&queue_state_path).await?;
            }
            QueueAction::Unfreeze => {
                cmd_unfreeze(&queue_state_path).await?;
            }
            QueueAction::Stats { watch, interval } => {
                cmd_stats(&cli.spool_path, &queue_state_path, watch, interval).await?;
            }
        },
        Commands::Dns { action } => {
            handle_dns_command_direct(&cli.control_socket, action).await?;
        }
        Commands::System { action } => {
            handle_system_command_direct(&cli.control_socket, action).await?;
        }
    }

    Ok(())
}

/// List messages in the queue
async fn cmd_list(
    spool_path: &std::path::Path,
    _queue_state_path: &std::path::Path,
    status_filter: Option<StatusFilter>,
    format: &str,
) -> anyhow::Result<()> {
    use empath_spool::BackingStore;

    // Load spool
    let spool = empath_spool::FileBackingStore::builder()
        .path(spool_path.to_path_buf())
        .build()?;
    let message_ids = spool.list().await?;

    // Load queue state from spool
    let queue_state = load_queue_state(&spool).await.ok();

    // Filter messages by status if requested
    let filtered: Vec<_> = status_filter.map_or_else(
        || message_ids.iter().collect(),
        |filter_status| {
            message_ids
                .iter()
                .filter(|id| {
                    if let Some(ref state) = queue_state
                        && let Some(info) = state.get(&id.to_string())
                    {
                        return status_matches(&info.status, filter_status);
                    }

                    // If no queue state, assume pending
                    filter_status == StatusFilter::Pending
                })
                .collect()
        },
    );

    // Output results
    match format {
        "json" => {
            // For JSON output, we'll manually construct it to avoid pulling in serde_json
            println!("[");
            for (i, id) in filtered.iter().enumerate() {
                let status = queue_state
                    .as_ref()
                    .and_then(|s| s.get(&id.to_string()))
                    .map_or_else(
                        || PENDING_STR.to_string(),
                        |info| format_status(&info.status),
                    );

                let comma = if i < filtered.len() - 1 { "," } else { "" };
                println!(
                    r#"  {{"id": "{}", "status": "{}", "timestamp": {}}}{}"#,
                    id,
                    status,
                    id.timestamp_ms(),
                    comma
                );
            }
            println!("]");
        }
        _ => {
            // Text format
            println!("{:<28} {:<15} {:<20}", "MESSAGE ID", "STATUS", "AGE");
            println!("{}", "-".repeat(65));

            for id in &filtered {
                let status = queue_state
                    .as_ref()
                    .and_then(|s| s.get(&id.to_string()))
                    .map_or_else(
                        || PENDING_STR.to_string(),
                        |info| format_status(&info.status),
                    );

                let age = format_age(id.timestamp_ms());

                println!("{id:<28} {status:<15} {age:<20}");
            }

            println!("\nTotal: {} message(s)", filtered.len());
        }
    }

    Ok(())
}

/// View detailed information about a message
async fn cmd_view(
    spool_path: &std::path::Path,
    _queue_state_path: &std::path::Path,
    message_id: &str,
) -> anyhow::Result<()> {
    use empath_spool::BackingStore;

    let id = parse_message_id(message_id)?;

    // Load spool
    let spool = empath_spool::FileBackingStore::builder()
        .path(spool_path.to_path_buf())
        .build()?;

    // Read message
    let context = spool.read(&id).await?;

    // Load queue state from spool
    let queue_state = load_queue_state(&spool).await.ok();
    let delivery_info = queue_state.as_ref().and_then(|s| s.get(&id.to_string()));

    // Display message details
    println!("Message ID: {id}");
    println!("Timestamp: {}", format_timestamp(id.timestamp_ms()));
    println!("Age: {}", format_age(id.timestamp_ms()));
    println!();

    // Envelope information
    println!("Envelope:");
    if let Some(sender) = context.envelope.sender() {
        println!("  From: {sender}");
    }
    if let Some(recipients) = context.envelope.recipients() {
        println!("  To: {}", recipients.len());
        for recipient in recipients.iter() {
            println!("    - {recipient}");
        }
    }
    println!();

    // Session information
    println!("Session:");
    println!("  ID: {}", context.id);
    println!(
        "  HELO/EHLO: {}",
        if context.extended { "EHLO" } else { "HELO" }
    );
    println!();

    // Delivery status
    if let Some(info) = delivery_info {
        println!("Delivery Status:");
        println!("  Status: {}", format_status(&info.status));
        println!("  Domain: {}", info.recipient_domain);
        println!("  Attempts: {}", info.attempts.len());

        if !info.attempts.is_empty() {
            println!();
            println!("  Attempt History:");
            for (i, attempt) in info.attempts.iter().enumerate() {
                println!(
                    "    {}. {} - {}",
                    i + 1,
                    format_timestamp(attempt.timestamp * 1000),
                    attempt.server
                );
                if let Some(ref error) = attempt.error {
                    println!("       Error: {error}");
                }
            }
        }

        if !info.mail_servers.is_empty() {
            println!();
            println!("  Mail Servers:");
            for server in info.mail_servers.iter() {
                let marker = if info.current_server_index
                    == info
                        .mail_servers
                        .iter()
                        .position(|s| s.host == server.host && s.port == server.port)
                        .unwrap_or(usize::MAX)
                {
                    "→"
                } else {
                    " "
                };
                println!(
                    "    {} {}:{} (priority: {})",
                    marker, server.host, server.port, server.priority
                );
            }
        }
    } else {
        println!("Delivery Status: Not yet queued");
    }

    // Message data size
    if let Some(data) = &context.data {
        println!();
        println!("Message Data:");
        println!("  Size: {} bytes", data.len());
    }

    Ok(())
}

/// Retry delivery of a message
async fn cmd_retry(
    spool_path: &std::path::Path,
    _queue_state_path: &std::path::Path,
    message_id: &str,
    force: bool,
) -> anyhow::Result<()> {
    use empath_spool::BackingStore;

    let id = parse_message_id(message_id)?;

    // Load spool
    let spool = empath_spool::FileBackingStore::builder()
        .path(spool_path.to_path_buf())
        .build()?;

    // Read context from spool
    let mut context = spool.read(&id).await?;

    // Get delivery info from context
    let Some(delivery_ctx) = &mut context.delivery else {
        anyhow::bail!("Message {id} has no delivery information");
    };

    // Check if message can be retried
    match &delivery_ctx.status {
        empath_delivery::DeliveryStatus::Failed(_)
        | empath_delivery::DeliveryStatus::Retry { .. }
        | empath_delivery::DeliveryStatus::Expired => {
            // OK to retry
        }
        empath_delivery::DeliveryStatus::Completed => {
            if !force {
                anyhow::bail!("Message already delivered. Use --force to retry anyway.");
            }
        }
        empath_delivery::DeliveryStatus::InProgress => {
            anyhow::bail!("Message is currently being delivered. Cannot retry.");
        }
        empath_delivery::DeliveryStatus::Pending => {
            println!("Message is already pending delivery.");
            return Ok(());
        }
    }

    // Reset status to pending
    delivery_ctx.status = empath_delivery::DeliveryStatus::Pending;
    delivery_ctx.current_server_index = 0;

    // Save updated context back to spool
    spool.update(&id, &context).await?;

    println!("Message {id} marked for retry");

    Ok(())
}

/// Delete a message from queue and spool
async fn cmd_delete(
    spool_path: &std::path::Path,
    _queue_state_path: &std::path::Path,
    message_id: &str,
    skip_confirm: bool,
) -> anyhow::Result<()> {
    use empath_spool::BackingStore;

    let id = parse_message_id(message_id)?;

    // Confirmation prompt
    if !skip_confirm {
        print!("Delete message {id}? [y/N] ");
        use std::io::{self, Write};
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;

        if !matches!(input.trim().to_lowercase().as_str(), "y" | "yes") {
            println!("Cancelled.");
            return Ok(());
        }
    }

    // Delete from spool (this also removes the delivery context)
    let spool = empath_spool::FileBackingStore::builder()
        .path(spool_path.to_path_buf())
        .build()?;
    spool.delete(&id).await?;

    println!("Message {id} deleted");

    Ok(())
}

/// Freeze the delivery queue
async fn cmd_freeze(queue_state_path: &std::path::Path) -> anyhow::Result<()> {
    // Create or update freeze marker file
    let freeze_path = queue_state_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join("queue_frozen");

    tokio::fs::write(&freeze_path, b"frozen").await?;

    println!("Delivery queue frozen");
    println!("Run 'empathctl queue unfreeze' to resume delivery");

    Ok(())
}

/// Unfreeze the delivery queue
async fn cmd_unfreeze(queue_state_path: &std::path::Path) -> anyhow::Result<()> {
    let freeze_path = queue_state_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join("queue_frozen");

    if freeze_path.exists() {
        tokio::fs::remove_file(&freeze_path).await?;
        println!("Delivery queue unfrozen");
    } else {
        println!("Delivery queue is not frozen");
    }

    Ok(())
}

/// Show queue statistics
async fn cmd_stats(
    spool_path: &std::path::Path,
    queue_state_path: &std::path::Path,
    watch: bool,
    interval: u64,
) -> anyhow::Result<()> {
    if watch {
        // Watch mode - continuously update
        loop {
            // Clear screen
            print!("\x1B[2J\x1B[1;1H");

            display_stats(spool_path, queue_state_path).await?;

            println!("\nPress Ctrl+C to exit");

            tokio::time::sleep(std::time::Duration::from_secs(interval)).await;
        }
    } else {
        // Single display
        display_stats(spool_path, queue_state_path).await?;
    }

    Ok(())
}

/// Display queue statistics
async fn display_stats(
    spool_path: &std::path::Path,
    queue_state_path: &std::path::Path,
) -> anyhow::Result<()> {
    // Load spool
    let spool = empath_spool::FileBackingStore::builder()
        .path(spool_path.to_path_buf())
        .build()?;

    let queue_state = load_queue_state(&spool).await.ok();

    let freeze_path = queue_state_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join("queue_frozen");
    let is_frozen = freeze_path.exists();

    println!("=== Empath Queue Statistics ===");
    println!();
    println!(
        "Queue Status: {}",
        if is_frozen { "FROZEN" } else { "Active" }
    );
    println!();

    if let Some(state) = queue_state {
        // Count by status
        let mut counts = std::collections::HashMap::new();
        for info in state.values() {
            let status_key = match &info.status {
                empath_delivery::DeliveryStatus::Pending => PENDING_STR,
                empath_delivery::DeliveryStatus::InProgress => IN_PROGRESS_STR,
                empath_delivery::DeliveryStatus::Completed => COMPLETED_STR,
                empath_delivery::DeliveryStatus::Failed(_) => FAILED_STR,
                empath_delivery::DeliveryStatus::Retry { .. } => RETRY_STR,
                empath_delivery::DeliveryStatus::Expired => EXPIRED_STR,
            };
            *counts.entry(status_key).or_insert(0) += 1;
        }

        println!("Messages by Status:");
        for s in [
            PENDING_STR,
            IN_PROGRESS_STR,
            RETRY_STR,
            FAILED_STR,
            EXPIRED_STR,
            COMPLETED_STR,
        ] {
            println!("{s}: {}", counts.get(s).unwrap_or(&0));
        }

        println!("Total: {}", state.len());

        // Domain statistics
        let mut domain_counts: std::collections::HashMap<std::sync::Arc<str>, usize> =
            std::collections::HashMap::new();
        for info in state.values() {
            *domain_counts
                .entry(info.recipient_domain.clone())
                .or_insert(0) += 1;
        }

        if !domain_counts.is_empty() {
            println!();
            println!("Top Domains:");
            let mut domains: Vec<_> = domain_counts.iter().collect();
            domains.sort_by(|a, b| b.1.cmp(a.1));
            for (domain, count) in domains.iter().take(10) {
                println!("  {domain:<30} {count:>6}");
            }
        }
    } else {
        println!("No queue state file found");
        println!("Queue state will be available once delivery processor starts");
    }

    Ok(())
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Parse a message ID from string
fn parse_message_id(s: &str) -> anyhow::Result<empath_spool::SpooledMessageId> {
    use empath_spool::SpooledMessageId;

    let filename = if s.ends_with(".bin") || s.ends_with(".eml") {
        s.to_string()
    } else {
        format!("{s}.bin")
    };

    SpooledMessageId::from_filename(&filename)
        .ok_or_else(|| anyhow::anyhow!("Invalid message ID: {s}"))
}

/// Load queue state from bincode file
async fn load_queue_state(
    spool: &empath_spool::FileBackingStore,
) -> anyhow::Result<std::collections::HashMap<String, empath_delivery::DeliveryInfo>> {
    use std::sync::Arc;

    use empath_spool::BackingStore;

    let message_ids = spool.list().await?;
    let mut state = std::collections::HashMap::new();

    for msg_id in message_ids {
        // Read context from spool
        let context = spool.read(&msg_id).await?;

        // Extract delivery info from context.delivery if it exists
        if let Some(delivery_ctx) = context.delivery {
            let info = empath_delivery::DeliveryInfo {
                message_id: msg_id.clone(),
                status: delivery_ctx.status,
                attempts: delivery_ctx.attempt_history,
                recipient_domain: delivery_ctx.domain,
                mail_servers: Arc::new(Vec::new()), // Not stored, will be resolved if needed
                current_server_index: delivery_ctx.current_server_index,
                queued_at: delivery_ctx.queued_at,
                next_retry_at: delivery_ctx.next_retry_at,
            };
            state.insert(msg_id.to_string(), info);
        }
    }

    Ok(state)
}

/// Check if a status matches the filter
const fn status_matches(status: &empath_delivery::DeliveryStatus, filter: StatusFilter) -> bool {
    matches!(
        (status, filter),
        (
            empath_delivery::DeliveryStatus::Pending,
            StatusFilter::Pending
        ) | (
            empath_delivery::DeliveryStatus::InProgress,
            StatusFilter::InProgress
        ) | (
            empath_delivery::DeliveryStatus::Completed,
            StatusFilter::Completed
        ) | (
            empath_delivery::DeliveryStatus::Failed(_),
            StatusFilter::Failed
        ) | (
            empath_delivery::DeliveryStatus::Retry { .. },
            StatusFilter::Retry
        ) | (
            empath_delivery::DeliveryStatus::Expired,
            StatusFilter::Expired
        )
    )
}

/// Format delivery status for display
fn format_status(status: &empath_delivery::DeliveryStatus) -> String {
    match status {
        empath_delivery::DeliveryStatus::Pending => PENDING_STR.to_string(),
        empath_delivery::DeliveryStatus::InProgress => IN_PROGRESS_STR.to_string(),
        empath_delivery::DeliveryStatus::Completed => COMPLETED_STR.to_string(),
        empath_delivery::DeliveryStatus::Failed(_) => FAILED_STR.to_string(),
        empath_delivery::DeliveryStatus::Retry { attempts, .. } => {
            format!("{RETRY_STR} ({attempts})")
        }
        empath_delivery::DeliveryStatus::Expired => EXPIRED_STR.to_string(),
    }
}

/// Format timestamp (milliseconds since epoch) as human-readable
fn format_timestamp(timestamp_ms: u64) -> String {
    use chrono::{TimeZone, Utc};

    let datetime = Utc.timestamp_millis_opt(i64::try_from(timestamp_ms).unwrap_or(0));
    if let chrono::offset::LocalResult::Single(dt) = datetime {
        dt.format("%Y-%m-%d %H:%M:%S UTC").to_string()
    } else {
        "unknown".to_string()
    }
}

/// Format age (time since timestamp) as human-readable
fn format_age(timestamp_ms: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();

    let age_ms = now.saturating_sub(u128::from(timestamp_ms));
    let age_secs = age_ms / 1000;

    if age_secs < 60 {
        format!("{age_secs}s")
    } else if age_secs < 3600 {
        let mins = age_secs / 60;
        format!("{mins}m")
    } else if age_secs < 86400 {
        let hours = age_secs / 3600;
        format!("{hours}h")
    } else {
        let days = age_secs / 86400;
        format!("{days}d")
    }
}

// ============================================================================
// Control Commands (via Unix socket IPC)
// ============================================================================

/// Check control socket connectivity and return client
fn check_control_socket(socket_path: &str) -> anyhow::Result<ControlClient> {
    let client = ControlClient::new(socket_path);

    // Check if socket exists first for better error messages
    if let Err(e) = client.check_socket_exists() {
        anyhow::bail!(
            "Cannot connect to Empath MTA control socket at {socket_path}.\n\
             Error: {e}\n\
             \n\
             Is the Empath MTA running?\n\
             You can configure the socket path with --control-socket or in empath.config.ron"
        );
    }

    Ok(client)
}

/// Handle DNS commands directly
async fn handle_dns_command_direct(
    socket_path: &str,
    action: DnsAction,
) -> anyhow::Result<()> {
    let client = check_control_socket(socket_path)?;
    handle_dns_command(&client, action).await
}

/// Handle system commands directly
async fn handle_system_command_direct(
    socket_path: &str,
    action: SystemAction,
) -> anyhow::Result<()> {
    let client = check_control_socket(socket_path)?;
    handle_system_command(&client, action).await
}

/// Handle DNS cache management commands
async fn handle_dns_command(
    client: &ControlClient,
    action: DnsAction,
) -> anyhow::Result<()> {
    use empath_control::{Response, protocol::ResponseData};

    let request = match action {
        DnsAction::ListCache => Request::Dns(DnsCommand::ListCache),
        DnsAction::ClearCache => Request::Dns(DnsCommand::ClearCache),
        DnsAction::Refresh { domain } => Request::Dns(DnsCommand::RefreshDomain(domain)),
        DnsAction::ListOverrides => Request::Dns(DnsCommand::ListOverrides),
        DnsAction::SetOverride { domain, server } => {
            Request::Dns(DnsCommand::SetOverride {
                domain,
                mx_server: server,
            })
        }
        DnsAction::RemoveOverride { domain } => {
            Request::Dns(DnsCommand::RemoveOverride(domain))
        }
    };

    let response = client.send_request(request).await?;

    match response {
        Response::Ok => {
            println!("✓ Command completed successfully");
        }
        Response::Data(data) => match data {
            ResponseData::DnsCache(cache) => {
                if cache.is_empty() {
                    println!("DNS cache is empty");
                } else {
                    println!("=== DNS Cache ({} entries) ===\n", cache.len());
                    let mut domains: Vec<_> = cache.keys().collect();
                    domains.sort();

                    for domain in domains {
                        let servers = &cache[domain];
                        println!("Domain: {domain}");
                        for server in servers {
                            println!(
                                "  → {}:{} (priority: {}, TTL: {}s)",
                                server.host, server.port, server.priority, server.ttl_remaining_secs
                            );
                        }
                        println!();
                    }
                }
            }
            ResponseData::MxOverrides(overrides) => {
                if overrides.is_empty() {
                    println!("No MX overrides configured");
                } else {
                    println!("=== MX Overrides ({}) ===\n", overrides.len());
                    let mut domains: Vec<_> = overrides.keys().collect();
                    domains.sort();

                    for domain in domains {
                        println!("{domain:<40} → {}", overrides[domain]);
                    }
                }
            }
            ResponseData::Message(msg) => {
                println!("✓ {msg}");
            }
            ResponseData::SystemStatus(_) => {
                println!("Unexpected response for DNS command: {data:?}");
            }
        },
        Response::Error(err) => {
            anyhow::bail!("Server error: {err}");
        }
    }

    Ok(())
}

/// Handle system management commands
async fn handle_system_command(
    client: &ControlClient,
    action: SystemAction,
) -> anyhow::Result<()> {
    use empath_control::{Response, protocol::ResponseData};

    let request = match action {
        SystemAction::Ping => Request::System(SystemCommand::Ping),
        SystemAction::Status => Request::System(SystemCommand::Status),
    };

    let response = client.send_request(request).await?;

    match response {
        Response::Ok => {
            println!("✓ Pong! MTA is responding");
        }
        Response::Data(data) => match data {
            ResponseData::SystemStatus(status) => {
                println!("=== Empath MTA Status ===\n");
                println!("Version:            {}", status.version);
                println!("Uptime:             {}", format_duration(status.uptime_secs));
                println!("Queue size:         {} message(s)", status.queue_size);
                println!("DNS cache entries:  {}", status.dns_cache_entries);
            }
            ResponseData::DnsCache(_) | ResponseData::MxOverrides(_) | ResponseData::Message(_) => {
                println!("Unexpected response for system command: {data:?}");
            }
        },
        Response::Error(err) => {
            anyhow::bail!("Server error: {err}");
        }
    }

    Ok(())
}

/// Format duration in human-readable form
fn format_duration(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        let mins = secs / 60;
        let rem_secs = secs % 60;
        format!("{mins}m {rem_secs}s")
    } else if secs < 86400 {
        let hours = secs / 3600;
        let rem_mins = (secs % 3600) / 60;
        format!("{hours}h {rem_mins}m")
    } else {
        let days = secs / 86400;
        let rem_hours = (secs % 86400) / 3600;
        format!("{days}d {rem_hours}h")
    }
}
