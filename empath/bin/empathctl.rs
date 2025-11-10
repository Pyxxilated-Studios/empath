//! Command-line utility for managing the Empath MTA queue
//!
//! This tool provides operational control over the delivery queue, including:
//! - Listing messages by status
//! - Viewing message details
//! - Retrying failed deliveries
//! - Deleting messages
//! - Freezing/unfreezing the queue
//! - Viewing statistics

#![allow(
    clippy::items_after_statements,
    clippy::single_match_else,
    clippy::case_sensitive_file_extension_comparisons
)]

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

/// Command-line utility for managing the Empath MTA delivery queue
#[derive(Parser, Debug)]
#[command(name = "empathctl")]
#[command(about = "Manage the Empath MTA delivery queue", long_about = None)]
#[command(version)]
struct Cli {
    /// Path to the spool directory
    #[arg(short, long, default_value = "/tmp/spool/empath")]
    spool_path: PathBuf,

    /// Path to the queue state file (bincode format)
    #[arg(short, long)]
    queue_state: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Queue management commands
    Queue {
        #[command(subcommand)]
        action: QueueAction,
    },
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
}

static PENDING_STR: &str = "Pending";
static IN_PROGRESS_STR: &str = "In Progress";
static COMPLETED_STR: &str = "Completed";
static FAILED_STR: &str = "Failed";
static RETRY_STR: &str = "Retry";

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
                cmd_retry(&queue_state_path, &message_id, force).await?;
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
                cmd_stats(&queue_state_path, watch, interval).await?;
            }
        },
    }

    Ok(())
}

/// List messages in the queue
async fn cmd_list(
    spool_path: &std::path::Path,
    queue_state_path: &std::path::Path,
    status_filter: Option<StatusFilter>,
    format: &str,
) -> anyhow::Result<()> {
    use empath_spool::BackingStore;

    // Load spool
    let spool = empath_spool::FileBackingStore::builder()
        .path(spool_path.to_path_buf())
        .build()?;
    let message_ids = spool.list().await?;

    // Load queue state if available
    let queue_state = load_queue_state(queue_state_path).await.ok();

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
    queue_state_path: &std::path::Path,
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

    // Load queue state
    let queue_state = load_queue_state(queue_state_path).await.ok();
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
    queue_state_path: &std::path::Path,
    message_id: &str,
    force: bool,
) -> anyhow::Result<()> {
    let id = parse_message_id(message_id)?;

    // Load queue state
    let mut queue_state = load_queue_state(queue_state_path).await?;

    // Find message in queue
    let info = queue_state
        .get_mut(&id.to_string())
        .ok_or_else(|| anyhow::anyhow!("Message {id} not found in queue"))?;

    // Check if message can be retried
    match &info.status {
        empath_delivery::DeliveryStatus::Failed(_)
        | empath_delivery::DeliveryStatus::Retry { .. } => {
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
    info.status = empath_delivery::DeliveryStatus::Pending;
    info.reset_server_index();

    // Save updated state
    save_queue_state(queue_state_path, &queue_state).await?;

    println!("Message {id} marked for retry");

    Ok(())
}

/// Delete a message from queue and spool
async fn cmd_delete(
    spool_path: &std::path::Path,
    queue_state_path: &std::path::Path,
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

    // Delete from spool
    let spool = empath_spool::FileBackingStore::builder()
        .path(spool_path.to_path_buf())
        .build()?;
    spool.delete(&id).await?;

    // Remove from queue state
    if let Ok(mut queue_state) = load_queue_state(queue_state_path).await {
        queue_state.remove(&id.to_string());
        let _ignore_error = save_queue_state(queue_state_path, &queue_state).await;
    }

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
    queue_state_path: &std::path::Path,
    watch: bool,
    interval: u64,
) -> anyhow::Result<()> {
    if watch {
        // Watch mode - continuously update
        loop {
            // Clear screen
            print!("\x1B[2J\x1B[1;1H");

            display_stats(queue_state_path).await?;

            println!("\nPress Ctrl+C to exit");

            tokio::time::sleep(std::time::Duration::from_secs(interval)).await;
        }
    } else {
        // Single display
        display_stats(queue_state_path).await?;
    }

    Ok(())
}

/// Display queue statistics
async fn display_stats(queue_state_path: &std::path::Path) -> anyhow::Result<()> {
    let queue_state = load_queue_state(queue_state_path).await.ok();

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
            };
            *counts.entry(status_key).or_insert(0) += 1;
        }

        println!("Messages by Status:");
        for s in [
            PENDING_STR,
            IN_PROGRESS_STR,
            RETRY_STR,
            FAILED_STR,
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
    path: &std::path::Path,
) -> anyhow::Result<std::collections::HashMap<String, empath_delivery::DeliveryInfo>> {
    let content = tokio::fs::read(path).await?;
    let state = bincode::deserialize(&content)?;
    Ok(state)
}

/// Save queue state to bincode file
async fn save_queue_state(
    path: &std::path::Path,
    state: &std::collections::HashMap<String, empath_delivery::DeliveryInfo>,
) -> anyhow::Result<()> {
    let encoded = bincode::serialize(state)?;
    tokio::fs::write(path, encoded).await?;
    Ok(())
}

/// Check if a status matches the filter
fn status_matches(status: &empath_delivery::DeliveryStatus, filter: StatusFilter) -> bool {
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
