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

use chrono::{TimeZone, Utc};
use clap::{Parser, Subcommand, ValueEnum};
use empath_common::context::{
    COMPLETED_STR, EXPIRED_STR, FAILED_STR, IN_PROGRESS_STR, PENDING_STR, RETRY_STR,
};
use empath_control::{
    ControlClient, DEFAULT_CONTROL_SOCKET, DnsCommand, QueueCommand, Request, RequestCommand,
    ResponsePayload, SpoolCommand, SystemCommand, protocol::ResponseData,
};

/// Command-line utility for managing the Empath MTA
#[derive(Parser, Debug)]
#[command(
    name = "empathctl",
    about = "Manage the Empath MTA",
    long_about = None,
    version,
    arg_required_else_help = true,
    subcommand_required = true
)]
struct Cli {
    /// Path to the spool directory (for queue commands)
    #[arg(short, long, default_value = "/tmp/spool/empath")]
    spool_path: PathBuf,

    /// Path to the queue state file (postcard format)
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
    /// Queue management commands (runtime control via socket - active delivery queue)
    Queue {
        #[command(subcommand)]
        action: QueueAction,
    },
    /// Spool management commands (runtime control via socket - persistent storage)
    Spool {
        #[command(subcommand)]
        action: SpoolAction,
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
    /// Reload configuration from disk (delivery domains/rate limits only)
    ReloadConfig,
    /// Reload TLS certificate/key pairs without restarting the daemon
    ReloadTls,
    /// List active TLS contexts and certificate metadata
    Tls,
    /// Set runtime log level (e.g., info, debug, trace)
    SetLogLevel {
        /// New log level
        level: String,
    },
    /// Show connection and rate-limit statistics
    ConnectionStats,
    /// List configured outbound bindings
    Bindings,
    /// Enable an outbound binding
    EnableBinding {
        /// Binding name
        name: String,
    },
    /// Disable an outbound binding
    DisableBinding {
        /// Binding name
        name: String,
    },
    /// Update rate limits (global or per-domain)
    SetRateLimit {
        /// Target domain (omit for global defaults)
        #[arg(long)]
        domain: Option<String>,
        /// Messages per second
        #[arg(long)]
        mps: f64,
        /// Burst size (tokens)
        #[arg(long, default_value = "20")]
        burst: u32,
    },
    /// List advertised SMTP capabilities
    Capabilities,
    /// Enable an advertised SMTP capability
    EnableCapability {
        /// Capability name (e.g., AUTH)
        name: String,
    },
    /// Disable an advertised SMTP capability
    DisableCapability {
        /// Capability name (e.g., AUTH)
        name: String,
    },
}

#[derive(Subcommand, Debug)]
enum QueueAction {
    /// List messages in the queue
    List {
        /// Filter by status
        #[arg(long, value_enum)]
        status: Option<StatusFilter>,

        /// Page number (1-based)
        #[arg(long, default_value = "1")]
        page: usize,

        /// Page size (max enforced by server)
        #[arg(long, default_value = "100")]
        page_size: usize,

        /// Output format (text, json)
        #[arg(long, default_value = "text")]
        format: String,
    },
    /// View detailed information about a specific message
    View {
        /// Message ID to view
        message_id: String,

        /// Skip reading message headers (faster, less I/O)
        #[arg(long)]
        no_headers: bool,

        /// Skip reading message body preview (faster, less I/O)
        #[arg(long)]
        no_body: bool,
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

#[derive(Subcommand, Debug)]
enum SpoolAction {
    /// List ALL messages in the spool (including delivered/failed)
    List {
        /// Filter by status
        #[arg(long, value_enum)]
        status: Option<StatusFilter>,

        /// Page number (1-based)
        #[arg(long, default_value = "1")]
        page: usize,

        /// Page size (max enforced by server)
        #[arg(long, default_value = "100")]
        page_size: usize,

        /// Output format (text, json)
        #[arg(long, default_value = "text")]
        format: String,
    },
    /// View detailed information about a specific message in spool
    View {
        /// Message ID to view
        message_id: String,

        /// Skip reading message headers (faster, less I/O)
        #[arg(long)]
        no_headers: bool,

        /// Skip reading message body preview (faster, less I/O)
        #[arg(long)]
        no_body: bool,
    },
    /// Clean up completed/failed messages from spool
    CleanupCompleted,
    /// Show spool statistics
    Stats,
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
        Commands::Spool { action } => {
            handle_spool_command_direct(&cli.control_socket, action).await?;
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
            | ResponseData::ConnectionStats(_)
            | ResponseData::OutboundBindings(_)
            | ResponseData::TlsContexts(_)
            | ResponseData::SmtpCapabilities(_)
            | ResponseData::QueueList(_)
            | ResponseData::QueueMessageDetails(_)
            | ResponseData::QueueStats(_)
            | ResponseData::SpoolList(_)
            | ResponseData::SpoolStats(_) => {
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
#[allow(
    clippy::too_many_lines,
    reason = "CLI command dispatch keeps output formatting in one place."
)]
async fn handle_system_command(client: &ControlClient, action: SystemAction) -> anyhow::Result<()> {
    let request = build_system_request(action);

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
            ResponseData::ConnectionStats(stats) => {
                println!("=== Connection Stats ===\n");
                println!("Queue size:             {}", stats.queue_size);
                println!("Rate limit buckets:     {}", stats.rate_limit_domains);
            }
            ResponseData::OutboundBindings(bindings) => {
                println!("=== Outbound Bindings ===\n");
                if bindings.is_empty() {
                    println!("No outbound bindings configured");
                } else {
                    for binding in bindings {
                        println!("- {}", binding.name);
                        if let Some(source_ip) = binding.source_ip {
                            println!("  source_ip: {source_ip}");
                        }
                        if let Some(source_interface) = binding.source_interface {
                            println!("  source_interface: {source_interface}");
                        }
                        if let Some(helo_domain) = binding.helo_domain {
                            println!("  helo_domain: {helo_domain}");
                        }
                        println!("  attempt_starttls: {}", binding.attempt_starttls);
                        println!("  enabled: {}", binding.enabled);
                        if let Some(health_state) = binding.health_state {
                            println!("  health_state: {health_state}");
                        }
                        if let Some(consecutive_failures) = binding.consecutive_failures {
                            println!("  consecutive_failures: {consecutive_failures}");
                        }
                        if let Some(open_until) = binding.open_until {
                            println!("  open_until: {open_until}");
                        }
                    }
                }
            }
            ResponseData::TlsContexts(contexts) => {
                println!("=== TLS Contexts ===\n");
                if contexts.is_empty() {
                    println!("No TLS contexts configured");
                } else {
                    for (idx, ctx) in contexts.iter().enumerate() {
                        println!("Context {}:", idx + 1);
                        println!("  certificate_path: {}", ctx.certificate_path);
                        println!("  key_path: {}", ctx.key_path);
                        if let Some(fingerprint) = &ctx.fingerprint_sha256 {
                            println!("  fingerprint_sha256: {fingerprint}");
                        }
                        if let Some(subject) = &ctx.subject {
                            println!("  subject: {subject}");
                        }
                        if let Some(issuer) = &ctx.issuer {
                            println!("  issuer: {issuer}");
                        }
                        if let Some(not_before) = ctx.not_before {
                            println!("  not_before: {}", format_timestamp_secs(not_before));
                        } else {
                            println!("  not_before: unknown");
                        }
                        if let Some(not_after) = ctx.not_after {
                            println!("  not_after: {}", format_timestamp_secs(not_after));
                        } else {
                            println!("  not_after: unknown");
                        }
                        if !ctx.subject_alt_names.is_empty() {
                            println!("  subject_alt_names:");
                            for name in &ctx.subject_alt_names {
                                println!("    - {name}");
                            }
                        }
                        if let Some(error) = &ctx.parse_error {
                            println!("  parse_error: {error}");
                        }
                        println!();
                    }
                }
            }
            ResponseData::SmtpCapabilities(capabilities) => {
                println!("=== SMTP Capabilities ===\n");
                if capabilities.is_empty() {
                    println!("No SMTP capabilities enabled");
                } else {
                    for capability in capabilities {
                        println!("- {capability}");
                    }
                }
            }
            ResponseData::Message(msg) => {
                println!("✓ {msg}");
            }
            ResponseData::DnsCache(_)
            | ResponseData::MxOverrides(_)
            | ResponseData::QueueList(_)
            | ResponseData::QueueMessageDetails(_)
            | ResponseData::QueueStats(_)
            | ResponseData::SpoolList(_)
            | ResponseData::SpoolStats(_) => {
                println!("Unexpected response for system command: {data:?}");
            }
        },
        ResponsePayload::Error(err) => {
            anyhow::bail!("Server error: {err}");
        }
    }

    Ok(())
}

fn build_system_request(action: SystemAction) -> Request {
    let command = match action {
        SystemAction::Ping => SystemCommand::Ping,
        SystemAction::Status => SystemCommand::Status,
        SystemAction::ReloadConfig => SystemCommand::ReloadConfig,
        SystemAction::ReloadTls => SystemCommand::ReloadTls,
        SystemAction::Tls => SystemCommand::ListTls,
        SystemAction::SetLogLevel { level } => SystemCommand::SetLogLevel { level },
        SystemAction::ConnectionStats => SystemCommand::ConnectionStats,
        SystemAction::Bindings => SystemCommand::ListBindings,
        SystemAction::EnableBinding { name } => SystemCommand::SetBindingState {
            name,
            enabled: true,
        },
        SystemAction::DisableBinding { name } => SystemCommand::SetBindingState {
            name,
            enabled: false,
        },
        SystemAction::SetRateLimit { domain, mps, burst } => SystemCommand::SetRateLimit {
            domain,
            messages_per_second: mps,
            burst,
        },
        SystemAction::Capabilities => SystemCommand::ListCapabilities,
        SystemAction::EnableCapability { name } => SystemCommand::SetCapabilityState {
            capability: name,
            enabled: true,
        },
        SystemAction::DisableCapability { name } => SystemCommand::SetCapabilityState {
            capability: name,
            enabled: false,
        },
    };

    Request::new(RequestCommand::System(command))
}

/// Handle queue stats watch mode
async fn handle_queue_stats_watch(client: &ControlClient, interval: u64) -> anyhow::Result<()> {
    use empath_control::protocol::ResponseData;

    loop {
        // Clear screen
        print!("\x1B[2J\x1B[1;1H");

        let response = client
            .send_request(Request::new(RequestCommand::Queue(QueueCommand::Stats)))
            .await?;

        match response.payload {
            ResponsePayload::Data(d) => match *d {
                ResponseData::QueueStats(stats) => {
                    display_queue_stats(&stats);
                }
                _ => {
                    anyhow::bail!("Expected QueueStats response, got different data type");
                }
            },
            ResponsePayload::Error(err) => {
                anyhow::bail!("Server error: {err}");
            }
            ResponsePayload::Ok => {
                anyhow::bail!("Unexpected Ok response for stats command");
            }
        }

        tokio::time::sleep(tokio::time::Duration::from_secs(interval)).await;
    }
}

/// Build queue command request from action
fn build_queue_request(action: &QueueAction) -> anyhow::Result<Request> {
    let command = match action {
        QueueAction::List {
            status,
            page,
            page_size,
            format: _,
        } => {
            let status_filter = status.as_ref().map(|s| {
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
            QueueCommand::List {
                status_filter,
                page: Some((*page).max(1)),
                page_size: Some((*page_size).max(1)),
            }
        }
        QueueAction::View {
            message_id,
            no_headers,
            no_body,
        } => QueueCommand::View {
            message_id: message_id.clone(),
            include_headers: !no_headers,
            include_body_preview: !no_body,
        },
        QueueAction::Retry { message_id, force } => QueueCommand::Retry {
            message_id: message_id.clone(),
            force: *force,
        },
        QueueAction::Delete { message_id, yes } => {
            // Confirmation prompt if not --yes
            if !yes {
                print!("Are you sure you want to delete message {message_id}? (y/N): ");
                stdout().flush()?;

                let mut input = String::new();
                stdin().read_line(&mut input)?;

                if !input.trim().eq_ignore_ascii_case("y") {
                    println!("Aborted.");
                    anyhow::bail!("Delete cancelled by user");
                }
            }
            QueueCommand::Delete {
                message_id: message_id.clone(),
            }
        }
        QueueAction::Stats { .. } => QueueCommand::Stats,
        QueueAction::ProcessNow => QueueCommand::ProcessNow,
    };

    Ok(Request::new(RequestCommand::Queue(command)))
}

/// Handle queue command response
fn handle_queue_response(payload: ResponsePayload) -> anyhow::Result<()> {
    use empath_control::protocol::ResponseData;

    match payload {
        ResponsePayload::Ok => {
            println!("✓ Command completed successfully");
        }
        ResponsePayload::Data(data) => match *data {
            ResponseData::QueueList(page) => {
                if page.messages.is_empty() {
                    if page.total == 0 {
                        println!("No messages in queue");
                    } else {
                        let page_count = total_pages(page.total, page.page_size).max(1);
                        println!(
                            "No messages on this page (page {} of {}, {} total matching)",
                            page.page, page_count, page.total
                        );
                    }
                } else {
                    let page_count = total_pages(page.total, page.page_size).max(1);
                    println!(
                        "=== Queue Messages (page {} of {}, showing {} of {} total) ===\n",
                        page.page,
                        page_count,
                        page.messages.len(),
                        page.total
                    );
                    for msg in page.messages {
                        println!("{msg}");
                    }
                    if page.has_more {
                        println!("\n… more results available; try --page {}.", page.page + 1);
                    }
                }
            }
            ResponseData::QueueMessageDetails(details) => {
                println!("=== Message Details ===\n");
                print!("{details}");
            }
            ResponseData::QueueStats(stats) => {
                display_queue_stats(&stats);
            }
            ResponseData::Message(msg) => {
                println!("✓ {msg}");
            }
            ResponseData::DnsCache(_)
            | ResponseData::MxOverrides(_)
            | ResponseData::SystemStatus(_)
            | ResponseData::ConnectionStats(_)
            | ResponseData::OutboundBindings(_)
            | ResponseData::TlsContexts(_)
            | ResponseData::SmtpCapabilities(_)
            | ResponseData::SpoolList(_)
            | ResponseData::SpoolStats(_) => {
                println!("Unexpected response for queue command: {data:?}");
            }
        },
        ResponsePayload::Error(err) => {
            anyhow::bail!("Server error: {err}");
        }
    }

    Ok(())
}

/// Handle queue management commands
async fn handle_queue_command(client: &ControlClient, action: QueueAction) -> anyhow::Result<()> {
    // Handle watch mode separately
    if let QueueAction::Stats {
        watch: true,
        interval,
    } = action
    {
        return handle_queue_stats_watch(client, interval).await;
    }

    let request = build_queue_request(&action)?;
    let response = client.send_request(request).await?;
    handle_queue_response(response.payload)
}

/// Handle spool commands directly
async fn handle_spool_command_direct(socket_path: &str, action: SpoolAction) -> anyhow::Result<()> {
    let client = check_control_socket(socket_path)?;
    handle_spool_command(&client, action).await
}

/// Build spool command request from action
fn build_spool_request(action: &SpoolAction) -> Request {
    let command = match action {
        SpoolAction::List {
            status,
            page,
            page_size,
            format: _,
        } => {
            let status_filter = status.as_ref().map(|s| {
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
            SpoolCommand::List {
                status_filter,
                page: Some((*page).max(1)),
                page_size: Some((*page_size).max(1)),
            }
        }
        SpoolAction::View {
            message_id,
            no_headers,
            no_body,
        } => SpoolCommand::View {
            message_id: message_id.clone(),
            include_headers: !no_headers,
            include_body_preview: !no_body,
        },
        SpoolAction::CleanupCompleted => SpoolCommand::CleanupCompleted,
        SpoolAction::Stats => SpoolCommand::Stats,
    };

    Request::new(RequestCommand::Spool(command))
}

/// Handle spool command response
fn handle_spool_response(payload: ResponsePayload) -> anyhow::Result<()> {
    use empath_control::protocol::ResponseData;

    match payload {
        ResponsePayload::Ok => {
            println!("✓ Command completed successfully");
        }
        ResponsePayload::Data(data) => match *data {
            ResponseData::SpoolList(page) => {
                if page.messages.is_empty() {
                    if page.total == 0 {
                        println!("No messages in spool");
                    } else {
                        let page_count = total_pages(page.total, page.page_size).max(1);
                        println!(
                            "No messages on this page (page {} of {}, {} total matching)",
                            page.page, page_count, page.total
                        );
                    }
                } else {
                    let page_count = total_pages(page.total, page.page_size).max(1);
                    println!(
                        "=== Spool Messages (page {} of {}, showing {} of {} total) ===\n",
                        page.page,
                        page_count,
                        page.messages.len(),
                        page.total
                    );
                    for msg in page.messages {
                        println!("{msg}");
                    }
                    if page.has_more {
                        println!("\n… more results available; try --page {}.", page.page + 1);
                    }
                }
            }
            ResponseData::SpoolStats(stats) => {
                display_spool_stats(&stats);
            }
            ResponseData::QueueMessageDetails(details) => {
                // Reuse queue view display
                println!("=== Message Details (from Spool) ===\n");
                print!("{details}");
            }
            ResponseData::Message(msg) => {
                println!("✓ {msg}");
            }
            ResponseData::DnsCache(_)
            | ResponseData::MxOverrides(_)
            | ResponseData::SystemStatus(_)
            | ResponseData::QueueList(_)
            | ResponseData::QueueStats(_)
            | ResponseData::ConnectionStats(_)
            | ResponseData::OutboundBindings(_)
            | ResponseData::TlsContexts(_)
            | ResponseData::SmtpCapabilities(_) => {
                println!("Unexpected response for spool command: {data:?}");
            }
        },
        ResponsePayload::Error(err) => {
            anyhow::bail!("Server error: {err}");
        }
    }

    Ok(())
}

/// Handle spool management commands
async fn handle_spool_command(client: &ControlClient, action: SpoolAction) -> anyhow::Result<()> {
    let request = build_spool_request(&action);
    let response = client.send_request(request).await?;
    handle_spool_response(response.payload)
}

/// Display spool statistics
fn display_spool_stats(stats: &empath_control::protocol::SpoolStats) {
    println!("=== Spool Statistics ===\n");
    println!("Total messages: {}", stats.total);
    println!(
        "Total size:     {} bytes ({:.2} MB)",
        stats.total_size,
        f64::from(u32::try_from(stats.total_size).unwrap_or(u32::MAX)) / 1_048_576.0
    );

    if !stats.by_status.is_empty() {
        println!("\nBy Status:");
        for (status, count) in &stats.by_status {
            println!("  {status:20} {count}");
        }
    }

    if let Some(age) = stats.oldest_message_age_secs {
        println!("\nOldest message age: {}", format_duration(age));
    }
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

const fn total_pages(total: usize, page_size: usize) -> usize {
    if total == 0 || page_size == 0 {
        0
    } else {
        total.div_ceil(page_size)
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

fn format_timestamp_secs(timestamp: u64) -> String {
    let datetime = Utc.timestamp_opt(i64::try_from(timestamp).unwrap_or(0), 0);
    datetime.single().map_or_else(
        || "unknown".to_string(),
        |dt| dt.format("%Y-%m-%d %H:%M:%S UTC").to_string(),
    )
}
