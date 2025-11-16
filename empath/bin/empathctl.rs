//! Command-line utility for managing the Empath MTA
//!
//! This tool provides operational control over the MTA, including:
//! - Queue management (list, view, retry, delete messages)
//! - DNS cache management (list, clear, refresh)
//! - System status and health checks
//! - Viewing statistics

use std::{
    io::{Write, stdin, stdout},
    path::PathBuf,
};

use chrono::{TimeZone, Utc, offset::LocalResult};
use clap::{Parser, Subcommand, ValueEnum};
use empath_control::{
    ControlClient, DEFAULT_CONTROL_SOCKET, DnsCommand, Request, RequestCommand, ResponsePayload,
    SystemCommand, protocol::ResponseData,
};

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
    /// Show queue statistics
    Stats {
        /// Watch mode - continuously update statistics
        #[arg(long)]
        watch: bool,

        /// Update interval in seconds (for watch mode)
        #[arg(long, default_value = "2")]
        interval: u64,
    },
    /// Trigger immediate queue processing
    ProcessNow,
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

    match cli.command {
        Commands::Queue { action } => {
            handle_queue_command_direct(&cli.control_socket, action).await?;
        }
        Commands::Dns { action } => {
            handle_dns_command_direct(&cli.control_socket, action).await?;
        }
        Commands::System { action } => {
            handle_system_command_direct(&cli.control_socket, action).await?;
        }
    }

    Ok(())
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
async fn handle_dns_command_direct(socket_path: &str, action: DnsAction) -> anyhow::Result<()> {
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

/// Handle queue commands directly
async fn handle_queue_command_direct(socket_path: &str, action: QueueAction) -> anyhow::Result<()> {
    // Enable persistent connections for watch mode to reduce socket overhead
    let client = if matches!(action, QueueAction::Stats { watch: true, .. }) {
        check_control_socket(socket_path)?.with_persistent_connection()
    } else {
        check_control_socket(socket_path)?
    };
    handle_queue_command(&client, action).await
}

/// Handle DNS cache management commands
async fn handle_dns_command(client: &ControlClient, action: DnsAction) -> anyhow::Result<()> {
    let request = match action {
        DnsAction::ListCache => Request::new(RequestCommand::Dns(DnsCommand::ListCache)),
        DnsAction::ClearCache => Request::new(RequestCommand::Dns(DnsCommand::ClearCache)),
        DnsAction::Refresh { domain } => {
            Request::new(RequestCommand::Dns(DnsCommand::RefreshDomain(domain)))
        }
        DnsAction::ListOverrides => Request::new(RequestCommand::Dns(DnsCommand::ListOverrides)),
        DnsAction::SetOverride { domain, server } => {
            Request::new(RequestCommand::Dns(DnsCommand::SetOverride {
                domain,
                mx_server: server,
            }))
        }
        DnsAction::RemoveOverride { domain } => {
            Request::new(RequestCommand::Dns(DnsCommand::RemoveOverride(domain)))
        }
    };

    let response = client.send_request(request).await?;

    match response.payload {
        ResponsePayload::Ok => {
            println!("✓ Command completed successfully");
        }
        ResponsePayload::Data(data) => match *data {
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
                                server.host,
                                server.port,
                                server.priority,
                                server.ttl_remaining_secs
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
            ResponseData::SystemStatus(_)
            | ResponseData::QueueList(_)
            | ResponseData::QueueMessageDetails(_)
            | ResponseData::QueueStats(_) => {
                println!("Unexpected response for DNS command: {data:?}");
            }
        },
        ResponsePayload::Error(err) => {
            anyhow::bail!("Server error: {err}");
        }
    }

    Ok(())
}

/// Handle system management commands
async fn handle_system_command(client: &ControlClient, action: SystemAction) -> anyhow::Result<()> {
    let request = match action {
        SystemAction::Ping => Request::new(RequestCommand::System(SystemCommand::Ping)),
        SystemAction::Status => Request::new(RequestCommand::System(SystemCommand::Status)),
    };

    let response = client.send_request(request).await?;

    match response.payload {
        ResponsePayload::Ok => {
            println!("✓ Pong! MTA is responding");
        }
        ResponsePayload::Data(data) => match *data {
            ResponseData::SystemStatus(status) => {
                println!("=== Empath MTA Status ===\n");
                println!("Version:            {}", status.version);
                println!(
                    "Uptime:             {}",
                    format_duration(status.uptime_secs)
                );
                println!("Queue size:         {} message(s)", status.queue_size);
                println!("DNS cache entries:  {}", status.dns_cache_entries);
            }
            ResponseData::DnsCache(_)
            | ResponseData::MxOverrides(_)
            | ResponseData::Message(_)
            | ResponseData::QueueList(_)
            | ResponseData::QueueMessageDetails(_)
            | ResponseData::QueueStats(_) => {
                println!("Unexpected response for system command: {data:?}");
            }
        },
        ResponsePayload::Error(err) => {
            anyhow::bail!("Server error: {err}");
        }
    }

    Ok(())
}

/// Handle queue management commands
#[allow(clippy::too_many_lines)]
async fn handle_queue_command(client: &ControlClient, action: QueueAction) -> anyhow::Result<()> {
    use empath_control::{QueueCommand, protocol::ResponseData};

    let request = match action {
        QueueAction::List { status, format: _ } => {
            let status_filter = status.map(|s| {
                match s {
                    StatusFilter::Pending => PENDING_STR,
                    StatusFilter::InProgress => IN_PROGRESS_STR,
                    StatusFilter::Completed => COMPLETED_STR,
                    StatusFilter::Failed => FAILED_STR,
                    StatusFilter::Retry => RETRY_STR,
                    StatusFilter::Expired => EXPIRED_STR,
                }
                .to_string()
            });
            Request::new(RequestCommand::Queue(QueueCommand::List { status_filter }))
        }
        QueueAction::View { message_id } => {
            Request::new(RequestCommand::Queue(QueueCommand::View { message_id }))
        }
        QueueAction::Retry { message_id, force } => {
            Request::new(RequestCommand::Queue(QueueCommand::Retry {
                message_id,
                force,
            }))
        }
        QueueAction::Delete { message_id, yes } => {
            // Confirmation prompt if not --yes
            if !yes {
                print!("Are you sure you want to delete message {message_id}? (y/N): ");
                stdout().flush()?;

                let mut input = String::new();
                stdin().read_line(&mut input)?;

                if !input.trim().eq_ignore_ascii_case("y") {
                    println!("Aborted.");
                    return Ok(());
                }
            }
            Request::new(RequestCommand::Queue(QueueCommand::Delete { message_id }))
        }
        QueueAction::Stats { watch, interval } => {
            if watch {
                // Watch mode - continuously update
                loop {
                    // Clear screen
                    print!("\x1B[2J\x1B[1;1H");

                    let response = client
                        .send_request(Request::new(RequestCommand::Queue(QueueCommand::Stats)))
                        .await?;

                    match response.payload {
                        ResponsePayload::Data(d) if matches!(*d, ResponseData::QueueStats(_)) => {
                            match *d {
                                ResponseData::QueueStats(stats) => {
                                    display_queue_stats(&stats);
                                }
                                _ => unreachable!(),
                            }
                        }
                        ResponsePayload::Error(err) => {
                            anyhow::bail!("Server error: {err}");
                        }
                        _ => {
                            anyhow::bail!("Unexpected response for stats command");
                        }
                    }

                    tokio::time::sleep(tokio::time::Duration::from_secs(interval)).await;
                }
            } else {
                Request::new(RequestCommand::Queue(QueueCommand::Stats))
            }
        }
        QueueAction::ProcessNow => {
            Request::new(RequestCommand::Queue(QueueCommand::ProcessNow))
        }
    };

    let response = client.send_request(request).await?;

    match response.payload {
        ResponsePayload::Ok => {
            println!("✓ Command completed successfully");
        }
        ResponsePayload::Data(data) => match *data {
            ResponseData::QueueList(messages) => {
                if messages.is_empty() {
                    println!("No messages in queue");
                } else {
                    println!("=== Queue Messages ({}) ===\n", messages.len());
                    for msg in messages {
                        println!("ID:        {}", msg.id);
                        println!("From:      {}", msg.from);
                        println!("To:        {}", msg.to.join(", "));
                        println!("Domain:    {}", msg.domain);
                        println!("Status:    {}", msg.status);
                        println!("Attempts:  {}", msg.attempts);
                        if let Some(next_retry) = msg.next_retry {
                            println!("Next retry: {}", format_timestamp(next_retry * 1000));
                        }
                        println!("Size:      {} bytes", msg.size);
                        println!("Spooled:   {}", format_timestamp(msg.spooled_at * 1000));
                        println!();
                    }
                }
            }
            ResponseData::QueueMessageDetails(details) => {
                println!("=== Message Details ===\n");
                println!("ID:        {}", details.id);
                println!("From:      {}", details.from);
                println!("To:        {}", details.to.join(", "));
                println!("Domain:    {}", details.domain);
                println!("Status:    {}", details.status);
                println!("Attempts:  {}", details.attempts);
                if let Some(next_retry) = details.next_retry {
                    println!("Next retry: {}", format_timestamp(next_retry * 1000));
                }
                if let Some(ref error) = details.last_error {
                    println!("Last error: {error}");
                }
                println!("Size:      {} bytes", details.size);
                println!("Spooled:   {}", format_timestamp(details.spooled_at * 1000));

                if !details.headers.is_empty() {
                    println!("\n--- Headers ---");
                    for (key, value) in &details.headers {
                        println!("{key}: {value}");
                    }
                }

                println!("\n--- Body Preview ---");
                println!("{}", details.body_preview);
            }
            ResponseData::QueueStats(stats) => {
                display_queue_stats(&stats);
            }
            ResponseData::Message(msg) => {
                println!("✓ {msg}");
            }
            ResponseData::DnsCache(_)
            | ResponseData::MxOverrides(_)
            | ResponseData::SystemStatus(_) => {
                println!("Unexpected response for queue command: {data:?}");
            }
        },
        ResponsePayload::Error(err) => {
            anyhow::bail!("Server error: {err}");
        }
    }

    Ok(())
}

/// Display queue statistics
fn display_queue_stats(stats: &empath_control::protocol::QueueStats) {
    println!("=== Queue Statistics ===\n");
    println!("Total messages: {}", stats.total);

    if !stats.by_status.is_empty() {
        println!("\nBy Status:");
        for (status, count) in &stats.by_status {
            println!("  {status:12} {count}");
        }
    }

    if !stats.by_domain.is_empty() {
        println!("\nBy Domain:");
        for (domain, count) in &stats.by_domain {
            println!("  {domain:30} {count}");
        }
    }

    if let Some(age) = stats.oldest_message_age_secs {
        println!("\nOldest message age: {}", format_duration(age));
    }
}

/// Format timestamp (milliseconds since epoch) as human-readable
fn format_timestamp(timestamp_ms: u64) -> String {
    let datetime = Utc.timestamp_millis_opt(i64::try_from(timestamp_ms).unwrap_or(0));
    if let LocalResult::Single(dt) = datetime {
        dt.format("%Y-%m-%d %H:%M:%S UTC").to_string()
    } else {
        "unknown".to_string()
    }
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
