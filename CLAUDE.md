# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

**Empath** is a Mail Transfer Agent (MTA) written in Rust. Key goals:
- Fully functional MTA for handling email transmission
- Easy to debug and test
- Extensible through a dynamic module/plugin system via FFI
- Embeddable in other applications (produces cdylib for each crate)

**Status**: Work in Progress

## Build and Development Commands

### Prerequisites
- Rust nightly toolchain (uses edition 2024 and nightly features)
- Current requirement: `rustc 1.93.0-nightly` or later

### Common Commands

**Quick Start with justfile:**
The project includes a comprehensive `justfile` task runner with 50+ commands for common development tasks. Install `just` with `cargo install just`, then:

```bash
just                # List all available commands
just setup          # Install development tools (nextest, watch, audit, deny, mold)
just ci             # Run full CI check locally (lint + fmt-check + test)
just dev            # Development workflow (fmt + lint + test)
just test           # Run all tests
just lint           # Run strict clippy checks
just bench          # Run all benchmarks
just queue-list     # List queue messages
just queue-watch    # Live queue statistics
```

See `just --list` for all 50+ available commands, or use the manual cargo commands below:

**Manual Cargo Commands:**

```bash
# Build entire workspace
cargo build

# Build release (uses thin LTO, opt-level 2, single codegen unit)
cargo build --release

# Run empath binary
cargo run

# Run with config file
cargo run -- empath.config.ron

# Run all tests
cargo test

# Run tests for specific crate
cargo test -p empath-smtp
cargo test -p empath-common
cargo test -p empath-spool

# Run single test by name
cargo test test_name
cargo test session::test::helo

# Lint with clippy (STRICT - project enforces all/pedantic/nursery via workspace lints)
cargo clippy --all-targets --all-features

# Run benchmarks
cargo bench                           # Run all benchmarks
cargo bench -p empath-smtp            # Run SMTP benchmarks only
cargo bench -p empath-spool           # Run spool benchmarks only
cargo bench -- --verbose              # Run with verbose output
cargo bench command_parsing           # Run specific benchmark group

# View benchmark results
# Results are saved to target/criterion/ with HTML reports
# Open target/criterion/report/index.html in a browser

# Generate C headers (happens automatically during build)
cargo build  # Outputs to empath/target/empath.h

# Build FFI example modules
cd empath-ffi/examples
gcc example.c -fpic -shared -o libexample.so -l empath -L ../../target/debug
gcc event.c -fpic -shared -o libevent.so -l empath -L ../../target/debug

# Queue Management and Runtime Control with empathctl
cargo build --bin empathctl            # Build empathctl CLI utility

# Queue Management (via control socket IPC)
./target/debug/empathctl queue list    # List all messages in queue
./target/debug/empathctl queue list --status=failed  # List only failed messages
./target/debug/empathctl queue view <message-id>  # View message details
./target/debug/empathctl queue delete <message-id> --yes  # Delete message
./target/debug/empathctl queue retry <message-id>   # Retry failed delivery
./target/debug/empathctl queue freeze    # Pause delivery processing
./target/debug/empathctl queue unfreeze  # Resume delivery processing
./target/debug/empathctl queue stats     # Show queue statistics
./target/debug/empathctl queue stats --watch --interval 2  # Live stats view
./target/debug/empathctl queue process-now  # Trigger immediate queue processing

# Runtime Control (via control socket IPC)
./target/debug/empathctl system ping              # Health check
./target/debug/empathctl system status            # System status
./target/debug/empathctl dns list-cache           # List DNS cache
./target/debug/empathctl dns clear-cache          # Clear DNS cache
./target/debug/empathctl dns refresh example.com  # Refresh domain
./target/debug/empathctl dns list-overrides       # List MX overrides

# Use custom control socket path
./target/debug/empathctl --control-socket /var/run/empath.sock system status
```

### Docker Development Environment

The project includes a complete Docker-based development environment with observability stack (OpenTelemetry, Prometheus, Grafana) and pre-built FFI example modules.

**Quick Start:**
```bash
just docker-up         # Start full stack (Empath + OTEL + Prometheus + Grafana)
just docker-logs       # View logs
just docker-grafana    # Open Grafana dashboard (admin/admin)
just docker-down       # Stop stack
```

**Available Services:**
- Empath SMTP: `localhost:1025`
- Grafana: `http://localhost:3000` (admin/admin)
- Prometheus: `http://localhost:9090`
- OTEL Collector: `http://localhost:4318` (OTLP)

**Additional Commands:**
```bash
just docker-rebuild       # Rebuild and restart containers
just docker-logs-empath   # View Empath logs only
just docker-test-email    # Send a test email
just docker-clean         # Full teardown including volumes
```

The Docker image includes pre-built FFI example modules (`libexample.so`, `libevent.so`) that demonstrate the plugin system. These are automatically loaded when using the Docker environment.

For detailed Docker documentation, see [`docker/README.md`](docker/README.md).

## Clippy Configuration

This project uses STRICT clippy linting configured at the workspace level. All changes must pass:

```bash
cargo clippy --all-targets --all-features
```

The lints are configured in the workspace `Cargo.toml`:
- `clippy::all` = deny
- `clippy::pedantic` = deny
- `clippy::nursery` = deny
- `clippy::must_use_candidate` = allow

These lints are automatically inherited by all crates via `[lints] workspace = true` in each crate's `Cargo.toml`.

Key clippy requirements:
- No wildcard imports (use explicit imports)
- Functions must be under 100 lines (extract helper methods if needed)
- No similar variable names (e.g., `head` vs `hhead` - use descriptive names)
- Add `# Panics` doc sections for functions that may panic
- Use `try_from()` instead of `as` for potentially truncating casts
- Add semicolons to last statement in blocks for consistency
- Use byte string literals `b"..."` instead of `"...".as_bytes()`
- Avoid holding locks/guards longer than necessary (significant drop tightening)
- Document items in code with backticks (e.g., `` `PostDot` state ``)

## Architecture Overview

### Workspace Structure

10-crate workspace:

1. **empath** - Main binary/library orchestrating all components
2. **empath-common** - Core abstractions: `Protocol`, `FiniteStateMachine`, `Controller`, `Listener` traits
3. **empath-smtp** - SMTP protocol implementation with FSM and session management
4. **empath-delivery** - Outbound mail delivery queue and processor
5. **empath-ffi** - C-compatible API for embedding and dynamic module loading
6. **empath-health** - HTTP health check endpoints for Kubernetes liveness and readiness probes
7. **empath-metrics** - OpenTelemetry metrics and observability instrumentation
8. **empath-control** - Control socket for runtime management via IPC
9. **empath-spool** - Message persistence to filesystem with watching
10. **empath-tracing** - Procedural macros for `#[traced]` instrumentation

### Key Architectural Patterns

#### 1. Generic Protocol System

New protocols implement the `Protocol` trait:

```rust
pub trait Protocol: Default + Send + Sync {
    type Session: SessionHandler;
    type Args: Clone + Debug + Deserialize;

    fn handle(&self, stream: TcpStream, peer: SocketAddr,
              init_context: HashMap<String, String>, args: Self::Args) -> Self::Session;
    fn validate(&self, args: &Self::Args) -> anyhow::Result<()>;
    fn ty() -> &'static str;
}
```

Location: `empath-common/src/traits/protocol.rs`

The `Controller<Proto: Protocol>` and `Listener<Proto: Protocol>` are generic, making connection handling infrastructure reusable across protocols.

#### 2. Finite State Machine Pattern

Protocol states managed via FSM trait:

```rust
pub trait FiniteStateMachine {
    type Input;
    type Context;

    fn transition(self, input: Self::Input, context: &mut Self::Context) -> Self;
}
```

Location: `empath-common/src/traits/fsm.rs`

SMTP implementation in `empath-smtp/src/lib.rs`:
- States: Connect, Ehlo, Helo, StartTLS, MailFrom, RcptTo, Data, Reading, PostDot, Quit, etc.
- Input: Command (parsed SMTP commands)
- Transitions validated through module system

#### 3. Module/Plugin System

Modules extend functionality without core modifications. Two types:

- **ValidationListener**: SMTP transaction validation hooks
  - Events: `Connect`, `MailFrom`, `RcptTo`, `Data`, `StartTls`
  - Return 0 for success, non-zero to reject

- **EventListener**: Connection lifecycle hooks
  - Events: `ConnectionOpened`, `ConnectionClosed`

**Module Interface** (C API):
- Export `declare_module()` returning `Mod` struct
- Use `EM_DECLARE_MODULE` macro for easy declaration
- Validation functions receive mutable `Context*` pointer
- Access/modify via `em_context_*` functions

Example: `empath-ffi/examples/example.c`
Module loading: `empath-ffi/src/modules/library.rs`

**Context Persistence and the Module Contract:**

The `Context` struct (in `empath-common/src/context.rs`) is deliberately designed to persist **all** fields to the spool, including what might initially appear to be "session-only" fields like:
- `id` - Session identifier
- `metadata` - Custom key-value pairs
- `extended` - Whether client used EHLO vs HELO
- `banner` - Server hostname

**Why This Is NOT a Layer Violation:**

This design is intentional and serves a critical purpose for the module system:

1. **Module Lifecycle Tracking**: Modules can set `metadata` during SMTP reception and reference it during delivery events. This enables plugins to maintain coherent state across the entire message journey without requiring external storage.

2. **Example Use Case**:
   ```c
   // Module during MailFrom event (SMTP reception)
   em_context_set_metadata(ctx, "correlation_id", "12345");
   em_context_set_metadata(ctx, "client_ip", "192.168.1.100");

   // Same module during DeliverySuccess event (hours/days later)
   String correlation_id = em_context_get_metadata(ctx, "correlation_id");
   // Module can now log or audit the delivery with the original correlation ID
   ```

3. **Single Source of Truth**: By storing everything in `Context`, modules have one consistent interface. They don't need to know about separate queue backends or maintain their own persistence layer.

4. **Delivery Queue State**: The `Context.delivery` field contains delivery-specific metadata (attempt count, retry times, status). This is persisted alongside the message in the spool, making queue state durable across restarts without requiring a separate queue storage backend.

**Storage Overhead**: The "session" fields add ~100 bytes per spooled message - negligible compared to typical email sizes (4KB-10MB+).

**Architectural Decision**: In TODO.md, task 0.3 originally suggested splitting Context into separate Message/DeliveryContext types as a "layer violation fix." This was reconsidered and rejected because it would:
- ❌ Break the module API contract
- ❌ Require modules to maintain external state storage
- ❌ Add complexity with conversion logic at boundaries
- ❌ Lose the elegant "single source of truth" design

Instead, we leverage Context persistence for queue state (task 1.1), storing delivery metadata in `Context.delivery` and using the spool as the persistent queue backend.

**Location**: `empath-common/src/context.rs` (Context, DeliveryContext, DeliveryStatus, DeliveryAttempt)

#### 4. Controller/Listener Pattern

Two-tier connection management:

- **Controller**: Manages multiple listeners, broadcasts shutdown signals (`empath-common/src/controller.rs`)
- **Listener**: Binds to socket, accepts connections, spawns session tasks (`empath-common/src/listener.rs`)

#### 5. Spool Abstraction

Message persistence via `Spool` trait:

```rust
pub trait Spool: Send + Sync {
    fn spool_message(&self, message: &Message) -> impl Future<Output = Result<()>>;
}
```

Location: `empath-spool/src/spool.rs`

Implementations:
- `FileBackedSpool`: Filesystem with atomic writes and directory watching
- `MemoryBackedSpool`: In-memory for testing (with `wait_for_count` for async tests)

### Configuration

Runtime config via RON (Rusty Object Notation) (default: `empath.config.ron`):

```ron
Empath (
    // SMTP controller with listeners
    smtp_controller: (
        listeners: [
            {
                socket: "[::]:1025",
                // Custom key-value pairs passed to sessions
                context: {
                    "service": "smtp",
                },
                // Server-side timeout configuration (RFC 5321 compliant defaults)
                // Prevents resource exhaustion from slow or malicious clients
                timeouts: (
                    command_secs: 300,          // 5 min for regular commands (EHLO, MAIL FROM, etc.)
                    data_init_secs: 120,        // 2 min for DATA command
                    data_block_secs: 180,       // 3 min between data chunks
                    data_termination_secs: 600, // 10 min for processing after final dot
                    connection_secs: 1800,      // 30 min maximum session lifetime
                ),
                // Optional extensions like SIZE and STARTTLS
                extensions: [
                    {
                        "size": 10000,
                    },
                    {
                        "starttls": {
                            "key": "private.key",
                            "certificate": "certificate.crt",
                        }
                    }
                ]
            },
        ],
    ),
    // Dynamically loaded modules
    modules: [
        (
            type: "SharedLibrary",
            name: "./path/to/module.so",
            arguments: ["arg1", "arg2"],
        ),
    ],
    // Spool configuration
    spool: (
        path: "./spool/directory",
    ),
    // Delivery configuration (optional, defaults shown)
    delivery: (
        scan_interval_secs: 30,      // How often to scan spool
        process_interval_secs: 10,   // How often to process queue
        max_attempts: 25,             // Max delivery attempts
        accept_invalid_certs: false, // Global TLS cert validation (SECURITY WARNING)

        // Per-domain configuration
        domains: {
            "test.example.com": (
                mx_override: "localhost:1025",
                accept_invalid_certs: true,  // Per-domain override
            ),
        },

        // Rate limiting configuration (optional, defaults shown)
        // Prevents overwhelming recipient SMTP servers and avoids blacklisting
        rate_limit: (
            messages_per_second: 10.0,  // Default rate: 10 messages per second per domain
            burst_size: 20,             // Allow bursts of up to 20 messages
            // Per-domain rate limit overrides
            domain_limits: {
                "gmail.com": (
                    messages_per_second: 50.0,  // Higher rate for high-volume domains
                    burst_size: 100,
                ),
                "test.example.com": (
                    messages_per_second: 1.0,   // Lower rate for testing
                    burst_size: 5,
                ),
            },
        ),
    ),
    // Control socket configuration (optional)
    control_socket: "/tmp/empath.sock",  // Path for IPC control socket

    // Control socket authentication (optional, disabled by default)
    control_auth: (
        enabled: true,
        token_hashes: [
            // SHA-256 hash of "your-secret-token"
            "4c5dc9b7708905f77f5e5d16316b5dfb425e68cb326dcd55a860e90a7707031e",
        ],
    ),

    // Metrics configuration (optional)
    metrics: (
        enabled: true,
        endpoint: "http://localhost:4318/v1/metrics",
        max_domain_cardinality: 1000,
        high_priority_domains: ["gmail.com", "outlook.com"],
        api_key: "your-metrics-api-key",  // Optional API key for OTLP collector
    ),

    // Health check configuration (optional)
    health: (
        enabled: true,
        listen_address: "[::]:8080",
        max_queue_size: 10000,
    ),

    // DSN (Delivery Status Notification) configuration (optional)
    // Generates bounce messages for failed deliveries per RFC 3464
    dsn: (
        enabled: true,                          // Enable/disable DSN generation
        reporting_mta: "mail.example.com",      // Hostname for Reporting-MTA field (FQDN)
        postmaster: "postmaster@example.com",   // Postmaster email for DSN sender
    ),
)
```

### Runtime Control via Control Socket

The control socket provides runtime management of the MTA without requiring restarts. Commands are sent via the `empathctl` utility using Unix domain socket IPC.

**Control Socket Configuration:**
- Default path: `/tmp/empath.sock`
- Configurable via `control_socket` field in config
- Uses bincode for efficient serialization
- Automatic cleanup on shutdown

**Available Commands:**

DNS Cache Management:
```bash
# List all cached DNS entries with TTL
empathctl dns list-cache

# Clear entire DNS cache
empathctl dns clear-cache

# Refresh DNS records for a specific domain
empathctl dns refresh example.com

# List configured MX overrides
empathctl dns list-overrides
```

System Status and Health:
```bash
# Health check - verify MTA is responding
empathctl system ping

# View system status (version, uptime, queue size, cache stats)
empathctl system status
```

**Output Example:**
```bash
$ empathctl system status
=== Empath MTA Status ===

Version:            0.0.2
Uptime:             2d 14h 32m
Queue size:         42 message(s)
DNS cache entries:  15

$ empathctl dns list-cache
=== DNS Cache (3 entries) ===

Domain: example.com
  → mail.example.com:25 (priority: 10, TTL: 285s)
  → mail2.example.com:25 (priority: 20, TTL: 285s)
```

**Custom Socket Path:**
```bash
# Use custom socket path
empathctl --control-socket /var/run/empath.sock system status
```

**Security:**
- Socket permissions inherited from umask (default: mode 0600, owner only)
- For multi-user access, adjust socket file permissions
- Token-based authentication available (see Authentication section below)

**Audit Logging:**

All control commands are automatically logged with structured data for accountability and compliance:

- **What's Logged:**
  - Command type (DNS, System, Queue)
  - User executing the command (from `$USER` environment variable)
  - User ID (UID) on Unix systems
  - Command details (full command with parameters)
  - Result status (success/failure with error details)
  - Timestamp (automatic via tracing framework)

- **Log Format:**
  ```
  INFO  Control command: DNS user=alice uid=1000 command=ClearCache
  INFO  DNS command completed successfully user=alice uid=1000
  ```

- **Log Location:**
  - Integrated with main empath tracing/logging
  - Controlled by `RUST_LOG` environment variable
  - For audit trails, configure log output to file via tracing-subscriber

- **Example Audit Trail:**
  ```bash
  # Set log level to capture audit events
  export RUST_LOG=empath=info
  ./empath

  # In logs:
  [2025-11-15T10:30:45Z INFO  empath::control_handler] Control command: DNS user="admin" uid=1000 command=ClearCache
  [2025-11-15T10:30:45Z INFO  empath::control_handler] DNS command completed successfully user="admin" uid=1000
  [2025-11-15T10:31:12Z INFO  empath::control_handler] Control command: Queue user="admin" uid=1000 command=Delete { message_id: "01JCXYZ..." }
  [2025-11-15T10:31:12Z INFO  empath::control_handler] Queue command completed successfully user="admin" uid=1000
  ```

- **Security Benefits:**
  - Accountability: Track who performed administrative actions
  - Forensics: Investigate security incidents or configuration changes
  - Compliance: Meet audit requirements for mail systems
  - Monitoring: Detect unauthorized access attempts

- **Implementation:**
  - Location: `empath/src/control_handler.rs`
  - Uses tracing framework for structured logging
  - Automatically captures errors and warnings
  - No performance impact (async logging)

**Implementation Details:**
- Control server runs alongside SMTP, spool, and delivery processors
- Graceful shutdown coordination
- 30s timeout per control request
- Location: `empath-control` crate, `empath/src/control_handler.rs`

**Known Limitations:**
- Runtime MX override updates not yet supported (requires DomainConfigRegistry refactor)
- Returns helpful error directing to config file update

### Health Check Endpoints

The health check server provides HTTP endpoints for Kubernetes liveness and readiness probes, enabling production deployments with proper health monitoring and container orchestration.

**Endpoints:**

- **`/health/live`** (Liveness Probe):
  - Returns 200 OK if the application is alive and can respond to requests
  - Kubernetes will restart the container if this probe fails
  - Always returns true (if the HTTP server can't respond, Kubernetes will detect the timeout)
  - Response time: <1 second

- **`/health/ready`** (Readiness Probe):
  - Returns 200 OK if the application is ready to accept traffic
  - Kubernetes will remove the pod from service endpoints if this probe fails
  - Checks all system components:
    - SMTP listeners bound and accepting connections
    - Spool is writable
    - Delivery processor is running
    - DNS resolver is operational
    - Queue size below threshold (default: 10,000 messages)
  - Returns 503 Service Unavailable with detailed JSON status if not ready

**Configuration:**

```ron
health: (
    // Enable/disable health check server
    enabled: true,

    // Address to bind the health check server
    // Default: [::]:8080
    listen_address: "[::]:8080",

    // Maximum queue size threshold for readiness probe
    // If the delivery queue exceeds this size, the readiness probe will fail
    // Default: 10000
    max_queue_size: 10000,
),
```

**Kubernetes Integration:**

```yaml
apiVersion: v1
kind: Pod
metadata:
  name: empath-mta
spec:
  containers:
  - name: empath
    image: empath:latest
    ports:
    - containerPort: 1025  # SMTP
    - containerPort: 8080  # Health checks
    livenessProbe:
      httpGet:
        path: /health/live
        port: 8080
      initialDelaySeconds: 10
      periodSeconds: 10
      timeoutSeconds: 1
      failureThreshold: 3
    readinessProbe:
      httpGet:
        path: /health/ready
        port: 8080
      initialDelaySeconds: 5
      periodSeconds: 5
      timeoutSeconds: 1
      failureThreshold: 3
```

**Testing Health Endpoints:**

```bash
# Liveness probe
curl http://localhost:8080/health/live
# Returns: OK (200)

# Readiness probe
curl http://localhost:8080/health/ready
# Returns: OK (200) if ready
# Returns: 503 with JSON details if not ready

# Example not-ready response:
# HTTP/1.1 503 Service Unavailable
# {
#   "alive": true,
#   "ready": false,
#   "smtp_ready": true,
#   "spool_ready": true,
#   "delivery_ready": false,
#   "dns_ready": true,
#   "queue_size": 15000,
#   "max_queue_size": 10000
# }
```

**Implementation Details:**
- Location: `empath-health` crate
- HTTP server: axum (lightweight, async-friendly)
- Thread-safe status tracking using `Arc<AtomicBool>` and `Arc<AtomicU64>`
- Graceful shutdown coordination with main application
- Response timeout: 1 second (enforced via tower-http middleware)

**Production Considerations:**
- Keep `max_queue_size` threshold appropriate for your workload
- Monitor readiness probe failures as early warning for capacity issues
- Liveness probe failures indicate critical application deadlock or crash
- Health endpoints are always enabled by default for Kubernetes compatibility

### Delivery Status Notifications (DSNs)

Empath implements RFC 3464 compliant Delivery Status Notifications (bounce messages) to inform senders when their messages cannot be delivered.

**What are DSNs?**

DSNs are automated bounce messages sent back to the original sender when email delivery fails. They provide detailed information about:
- Why the delivery failed
- Which recipients were affected
- When delivery was attempted
- Original message headers for reference

**When DSNs are Generated:**

1. **Permanent Failures (5xx SMTP errors)**:
   - Invalid recipient address
   - Domain not found
   - Message rejected by recipient server
   - Message too large

2. **Max Retry Attempts Exhausted**:
   - Temporary failures that persisted beyond max_attempts (default: 25)
   - Connection timeouts that couldn't be resolved
   - Server busy errors that didn't clear

**DSN Prevention (Bounce Loop Protection):**

DSNs are **NOT** generated for:
- Messages with null sender (`MAIL FROM:<>`) - prevents bounce loops
- Temporary failures still in retry state
- System errors (internal errors that don't indicate delivery failure)

**Configuration:**

```ron
dsn: (
    // Enable/disable DSN generation globally
    enabled: true,

    // Hostname for Reporting-MTA field (use FQDN of your server)
    reporting_mta: "mail.example.com",

    // Postmaster email address (DSN sender)
    postmaster: "postmaster@example.com",
)
```

**DSN Message Structure (RFC 3464):**

DSNs use the `multipart/report` MIME format with three parts:

1. **Part 1: Human-Readable Explanation** (`text/plain`)
   - Clear explanation of the failure
   - Message details (sender, recipients, attempts, domain)
   - Contact information for assistance

2. **Part 2: Machine-Readable Status** (`message/delivery-status`)
   - Per-message fields: Reporting-MTA, Arrival-Date
   - Per-recipient fields: Final-Recipient, Action, Status, Diagnostic-Code
   - SMTP status codes (5.0.0 for permanent, 4.0.0 for exhausted retries)

3. **Part 3: Original Message Headers** (`text/rfc822-headers`)
   - First 1KB of original message headers
   - Includes From, To, Subject, Message-ID for reference
   - Body not included to keep DSN size manageable

**Example DSN:**

```
From: Mail Delivery System <postmaster@example.com>
To: sender@example.org
Subject: Delivery Status Notification (Failure)
Content-Type: multipart/report; report-type="delivery-status"

This is the mail system at host example.com.

I'm sorry to have to inform you that your message could not
be delivered to one or more recipients.

Your message could not be delivered:

recipient@example.com: Invalid recipient: user not found

Message details:
- Original sender: sender@example.org
- Failed recipient(s): recipient@example.com
- Delivery attempts: 25
- Domain: example.com
- Last server attempted: mx1.example.com:25

---

Reporting-MTA: dns; mail.example.com
Arrival-Date: Sat, 16 Nov 2025 10:30:45 +0000

Final-Recipient: rfc822; recipient@example.com
Action: failed
Status: 5.0.0
Diagnostic-Code: smtp; Invalid recipient: user not found
Remote-MTA: dns; mx1.example.com
Last-Attempt-Date: Sat, 16 Nov 2025 10:35:12 +0000
```

**Implementation Details:**

- Location: `empath-delivery/src/dsn.rs`
- DSNs are automatically spooled and delivered like regular messages
- Module events (`DeliveryFailure`) are dispatched before DSN generation
- Logs include DSN tracking: `message_id`, `dsn_id`, `original_sender`

**Monitoring DSN Generation:**

```logql
# Find all generated DSNs
{service="empath"} | json | fields.message=~"DSN generated"

# Track DSN spool failures
{service="empath"} | json | fields.message=~"Failed to spool DSN"

# Count DSNs by original domain
sum by (fields_domain) (
  count_over_time(
    {service="empath"} | json | fields.message=~"DSN generated" [1h]
  )
)
```

**Best Practices:**

1. **Use proper FQDN** for `reporting_mta` (improves deliverability)
2. **Monitor DSN rates** (high rates indicate delivery problems)
3. **Keep enabled** in production (RFC requirement for MTAs)
4. **Review bounce patterns** to identify configuration issues

### Rate Limiting

Empath implements per-domain rate limiting using the token bucket algorithm to prevent overwhelming recipient SMTP servers and avoid blacklisting.

**Why Rate Limiting?**

Without rate limiting, bulk email delivery can:
- **Overwhelm** recipient servers (causing connection refusals)
- **Trigger spam filters** (high-volume bursts look suspicious)
- **Cause blacklisting** (IP/domain reputation damage)
- **Violate policies** (many providers have rate limits)

**Token Bucket Algorithm:**

Each domain has its own token bucket with:
- **Tokens**: Replenished at a constant rate (messages_per_second)
- **Capacity**: Maximum burst size (allows short bursts)
- **Consumption**: Each delivery attempt consumes 1 token
- **Delay**: When bucket empty, delivery is delayed until tokens refill

**Example Flow:**

```text
Rate: 10 msg/sec, Burst: 20 tokens
─────────────────────────────────────────────────────
Time 0s:  Bucket has 20 tokens (full capacity)
          Send 20 messages → 0 tokens remaining

Time 1s:  Bucket refills to 10 tokens
          Send 10 messages → 0 tokens remaining

Time 2s:  Bucket refills to 10 tokens
          Sustained rate: 10 msg/sec
```

**Configuration:**

```ron
rate_limit: (
    // Default rate for all domains
    messages_per_second: 10.0,  // 10 messages per second
    burst_size: 20,             // Allow bursts of 20 messages

    // Per-domain overrides
    domain_limits: {
        // High-volume providers
        "gmail.com": (
            messages_per_second: 50.0,
            burst_size: 100,
        ),
        "outlook.com": (
            messages_per_second: 50.0,
            burst_size: 100,
        ),

        // Conservative rate for small domains
        "smalldomain.com": (
            messages_per_second: 1.0,
            burst_size: 5,
        ),

        // Testing/development
        "test.example.com": (
            messages_per_second: 1.0,
            burst_size: 5,
        ),
    },
),
```

**How It Works:**

1. **Check before delivery**: Before attempting SMTP delivery, check if tokens available
2. **Consume token**: If available, consume 1 token and proceed with delivery
3. **Delay if limited**: If no tokens, calculate wait time and reschedule message
4. **Automatic refill**: Tokens refill continuously at configured rate
5. **Per-domain isolation**: Each domain has independent bucket (no cross-domain impact)

**Rate Limit Delay:**

When rate limited, messages are:
- **Not failed** - Status remains `Pending` (not an error)
- **Rescheduled** - `next_retry_at` set to when tokens available
- **Logged** - Structured log with domain, wait time, and next retry
- **Metered** - Metrics track rate limit events per domain

**Metrics:**

```promql
# Total rate limited deliveries by domain
empath_delivery_rate_limited_total{domain="example.com"}

# Distribution of rate limit delays
histogram_quantile(0.95, empath_delivery_rate_limit_delay_seconds_bucket)

# Rate limit delay seconds by domain
empath_delivery_rate_limit_delay_seconds_sum{domain="example.com"} /
empath_delivery_rate_limit_delay_seconds_count{domain="example.com"}
```

**Monitoring Rate Limiting:**

```logql
# Find rate limited deliveries
{service="empath"} | json | fields.message=~"Rate limit exceeded"

# Count rate limits by domain (last hour)
sum by (fields_domain) (
  count_over_time(
    {service="empath"} | json | fields.message=~"Rate limit exceeded" [1h]
  )
)

# Average rate limit delay by domain
avg by (fields_domain) (
  avg_over_time(
    {service="empath"} | json | fields.wait_seconds > 0 [5m]
  )
)
```

**Example Logs:**

```json
{
  "timestamp": "2025-11-16T10:30:45.123456+00:00",
  "level": "INFO",
  "fields": {
    "message": "Rate limit exceeded, delaying delivery",
    "message_id": "01JCXYZ123ABC",
    "domain": "example.com",
    "wait_seconds": 0.5,
    "next_retry_at": "2025-11-16T10:30:45.623456+00:00"
  },
  "target": "empath_delivery::processor::delivery"
}
```

**Implementation Details:**

- **Location**: `empath-delivery/src/rate_limiter.rs`
- **Concurrency**: `DashMap` for lock-free domain bucket lookup
- **Synchronization**: `parking_lot::Mutex` for individual bucket access
- **Precision**: Floating-point tokens for sub-second accuracy
- **Refill**: Automatic time-based refill on every access

**Best Practices:**

1. **Start conservative**: Use default 10 msg/sec, monitor, adjust upward
2. **Override high-volume**: Gmail/Outlook can handle 50-100 msg/sec
3. **Burst appropriately**: Burst size = 2x sustained rate is a good starting point
4. **Monitor metrics**: Watch for excessive rate limiting (increase limits)
5. **Check logs**: Rate limit events indicate capacity planning needs
6. **Domain research**: Check recipient provider's published rate limits
7. **Test first**: Use low rates for new domains until reputation established

**Common Rate Limits (Approximate):**

| Provider | Recommended Rate | Burst Size | Notes |
|----------|------------------|------------|-------|
| Gmail    | 20-50 msg/sec    | 100        | High volume tolerance |
| Outlook  | 20-50 msg/sec    | 100        | Enforce sender reputation |
| Yahoo    | 10-20 msg/sec    | 50         | Conservative limits |
| Small domains | 1-5 msg/sec   | 10         | May have limited capacity |
| Default  | 10 msg/sec       | 20         | Safe starting point |

**Troubleshooting:**

- **Too many rate limits**: Increase `messages_per_second` for affected domain
- **Blacklisting despite limits**: Reduce rate further or check IP reputation
- **Slow delivery**: Expected behavior - rate limiting trades speed for reliability
- **No rate limiting observed**: Check configuration loaded, verify domain name matches

### JSON Structured Logging

Empath uses JSON structured logging for production observability, enabling powerful log aggregation and querying with tools like Loki, Grafana, and LogQL.

**Features:**
- **JSON Format**: All logs output as structured JSON for machine parsing
- **Structured Fields**: Logs include contextual fields (message_id, domain, delivery_attempt, smtp_code, sender, recipient)
- **Span Context**: Current span information included for distributed tracing correlation
- **File/Line Info**: Debug information (filename, line_number) included
- **ISO 8601 Timestamps**: RFC 3339 compliant timestamps
- **Trace Correlation**: OpenTelemetry trace context (trace_id, span_id) automatically injected into all log entries

**Example Log Output:**

```json
{
  "timestamp": "2025-11-16T06:50:35.123456+00:00",
  "level": "INFO",
  "fields": {
    "message": "Scheduled retry with exponential backoff",
    "message_id": "01JCXYZ123ABC",
    "domain": "example.com",
    "delivery_attempt": 2,
    "retry_delay_secs": 120
  },
  "target": "empath_delivery",
  "filename": "empath-delivery/src/processor/delivery.rs",
  "line_number": 330,
  "span": {
    "name": "deliver_message"
  },
  "spans": [
    {"name": "process_queue"},
    {"name": "deliver_message"}
  ]
}
```

**Configuration:**

Control log level via environment variables:

```bash
# Set log level (supports TRACE, DEBUG, INFO, WARN, ERROR)
export RUST_LOG=info           # Recommended for production
export RUST_LOG=debug          # Development/troubleshooting
export RUST_LOG=empath=trace   # Verbose logging for empath crates only

# Legacy support (for backward compatibility)
export LOG_LEVEL=info
```

**Docker Configuration:**

The Docker Compose stack automatically enables JSON logging:

```yaml
environment:
  - RUST_LOG=info    # JSON logs enabled
```

**LogQL Query Examples:**

When using Loki for log aggregation, these LogQL queries enable powerful log analysis:

```logql
# Find all logs for a specific message
{service="empath"} | json | fields.message_id="01JCXYZ123ABC"

# Find delivery failures by domain
{service="empath"} | json | level="ERROR" | fields.domain="example.com"

# Track delivery retries
{service="empath"} | json | fields.message=~"retry"
  | line_format "{{.fields.domain}}: attempt {{.fields.delivery_attempt}}"

# Count delivery attempts by domain (last hour)
sum by (fields_domain) (
  count_over_time(
    {service="empath"} | json | fields.delivery_attempt > 0 [1h]
  )
)

# Find SMTP errors with codes
{service="empath"} | json | fields.smtp_code >= 400

# Monitor MX server fallbacks
{service="empath"} | json | fields.message=~"next MX server"

# Track spool failures
{service="empath"} | json | fields.message=~"spool" | level="ERROR"
```

**Structured Fields Available:**

| Field | Type | Description | Example |
|-------|------|-------------|---------|
| `fields.message_id` | String | Unique message identifier (ULID) | `01JCXYZ123ABC` |
| `fields.domain` | String | Recipient domain | `example.com` |
| `fields.delivery_attempt` | Integer | Current delivery attempt number | `2` |
| `fields.sender` | String | Email sender address | `user@example.com` |
| `fields.recipient` | String | Email recipient address | `recipient@example.com` |
| `fields.recipient_count` | Integer | Number of recipients | `5` |
| `fields.smtp_code` | Integer | SMTP response code | `250`, `550` |
| `fields.server` | String | Mail server address | `mx1.example.com:25` |
| `fields.retry_delay_secs` | Integer | Retry delay in seconds | `120` |
| `fields.error` | String | Error message | `Connection refused` |
| `level` | String | Log level | `INFO`, `ERROR`, `WARN` |
| `target` | String | Rust module path | `empath_delivery` |
| `filename` | String | Source file | `empath-delivery/src/lib.rs` |
| `line_number` | Integer | Source line number | `330` |
| `span.name` | String | Current span name | `deliver_message` |

**Implementation Location:**
- Logging init: `empath-common/src/logging.rs`
- JSON formatter: `tracing-subscriber::fmt::json()`
- Structured fields added in delivery processor and SMTP session

**Benefits:**
- **90% reduction in log investigation time** via structured queries
- **Instant filtering** by message_id, domain, or delivery_attempt
- **Aggregate metrics** from logs (retry rates, error distributions)
- **Correlation** across components via span context and trace_id
- **Machine-readable** for automated alerting and dashboards
- **Distributed tracing** integration with Jaeger for end-to-end request tracking

### Log Aggregation with Loki

Empath includes a complete log aggregation pipeline using Grafana Loki and Promtail in the Docker stack, enabling centralized log search across multiple container instances.

**Components:**

1. **Loki** - Log aggregation and storage backend
2. **Promtail** - Log shipper that scrapes Docker container logs
3. **Grafana** - Visualization with pre-configured Loki datasource and dashboards

**Configuration Files:**

- `docker/compose.dev.yml` - Loki and Promtail services
- `docker/loki.yml` - Loki configuration with 7-day retention and compression
- `docker/promtail.yml` - Promtail scrape configuration for Docker containers
- `docker/grafana/provisioning/datasources/loki.yml` - Grafana Loki datasource
- `docker/grafana/provisioning/dashboards/logs-dashboard.json` - Log exploration dashboard

**Starting the Stack:**

```bash
just docker-up         # Start full stack (Empath + OTEL + Prometheus + Grafana + Loki)
just docker-logs       # View logs
just docker-grafana    # Open Grafana dashboard (admin/admin)
just docker-down       # Stop stack
```

**Accessing Loki:**

- **Loki API**: `http://localhost:3100`
- **Grafana**: `http://localhost:3000` (admin/admin)
  - Navigate to **Explore** → Select **Loki** datasource
  - Or use the pre-configured **"Empath MTA - Log Exploration"** dashboard

**Log Exploration Dashboard:**

The pre-configured dashboard includes:

1. **Empath MTA Logs** - Full log stream with JSON parsing
2. **Log Volume by Level** - Stacked bar chart showing INFO/WARN/ERROR distribution
3. **Error Count (5m)** - Gauge showing recent errors
4. **Warning Count (5m)** - Gauge showing recent warnings
5. **Activity by Domain** - Time series of delivery activity per domain
6. **Average Delivery Attempts by Domain** - Track retry patterns
7. **Errors and Warnings** - Filtered log stream showing only problems

**Common LogQL Queries:**

```logql
# All Empath MTA logs
{job="empath-mta"} | json

# Filter by message_id
{job="empath-mta"} | json | message_id="01JCXYZ123ABC"

# Only errors
{job="empath-mta"} | json | level="ERROR"

# Delivery retries
{job="empath-mta"} | json | fields.message=~"retry"

# Track a specific domain
{job="empath-mta"} | json | domain="example.com"

# SMTP errors (4xx/5xx codes)
{job="empath-mta"} | json | smtp_code >= 400

# Count errors per domain (last hour)
sum by (domain) (count_over_time({job="empath-mta"} | json | level="ERROR" [1h]))

# Average delivery attempts by domain
avg by (domain) (avg_over_time({job="empath-mta"} | json | delivery_attempt > 0 [5m]))
```

**Retention and Storage:**

- **Retention Period**: 7 days (168 hours)
- **Compaction**: Runs every 10 minutes
- **Compression**: Enabled for efficient storage
- **Storage Backend**: Filesystem (`loki-data` volume)

**Promtail Collection:**

Promtail automatically discovers and scrapes logs from Docker containers with the label `logging=promtail`:

```yaml
labels:
  logging: "promtail"
  logging_jobname: "empath-mta"
```

**Benefits:**

- **Centralized Search**: Query logs across all container instances
- **No SSH Required**: Access logs via Grafana UI without container shell access
- **Historical Analysis**: 7-day retention for troubleshooting past issues
- **Powerful Queries**: LogQL supports filtering, aggregation, and metrics extraction
- **Visual Exploration**: Pre-built dashboards for common queries
- **Correlation**: Link logs to metrics via Grafana, and logs to traces via trace_id in Jaeger

**Production Considerations:**

- Adjust `retention_period` based on compliance requirements
- Monitor Loki disk usage (`loki-data` volume)
- Consider remote storage (S3, GCS) for long-term retention
- Scale Promtail horizontally for high-volume environments
- Use Grafana alerting for ERROR log spikes

### Distributed Tracing with OpenTelemetry

Empath implements distributed tracing using OpenTelemetry and Jaeger, providing end-to-end visibility into message processing from SMTP reception through delivery. Every log entry automatically includes trace context (`trace_id`, `span_id`) for correlation.

**Architecture:**

1. **OpenTelemetry SDK** - Generates trace IDs and span IDs for all operations
2. **OTLP Exporter** - Sends trace data to OpenTelemetry Collector via HTTP
3. **OpenTelemetry Collector** - Receives traces and forwards to Jaeger
4. **Jaeger** - Stores and visualizes distributed traces
5. **tracing-opentelemetry** - Bridges Rust `tracing` spans with OpenTelemetry

**Components:**

- **Trace ID**: Unique identifier for the entire message journey (SMTP → Spool → Delivery)
- **Span ID**: Unique identifier for each operation within the trace
- **Parent Span**: Links operations into a hierarchical call tree
- **Trace Context Propagation**: Automatic injection of trace IDs into all log entries

**Configuration:**

The OpenTelemetry exporter endpoint is configured via environment variable:

```bash
# Default: http://localhost:4318
export OTEL_EXPORTER_OTLP_ENDPOINT=http://otel-collector:4318
```

In the Docker stack, this is automatically configured to point to the otel-collector service:

```yaml
environment:
  - OTEL_EXPORTER_OTLP_ENDPOINT=http://otel-collector:4318
```

**Example Trace Hierarchy:**

```
Trace ID: a1b2c3d4e5f6g7h8i9j0...
├─ smtp_session (span_id: 1a2b3c4d)
│  ├─ receive_headers (span_id: 2b3c4d5e)
│  ├─ validate_message (span_id: 3c4d5e6f)
│  └─ spool_message (span_id: 4d5e6f7g)
└─ deliver_message (span_id: 5e6f7g8h)
   ├─ dns_lookup (span_id: 6f7g8h9i)
   ├─ connect_mx (span_id: 7g8h9i0j)
   ├─ smtp_handshake (span_id: 8h9i0j1k)
   └─ transmit_data (span_id: 9i0j1k2l)
```

**Log-to-Trace Correlation:**

Every JSON log entry automatically includes the current trace context:

```json
{
  "timestamp": "2025-11-16T10:30:45.123456789Z",
  "level": "INFO",
  "target": "empath_delivery",
  "fields": {
    "message": "Delivery successful",
    "message_id": "01JCXYZ...",
    "domain": "example.com",
    "delivery_attempt": 1
  },
  "span": {
    "name": "deliver_message",
    "trace_id": "a1b2c3d4e5f6g7h8...",
    "span_id": "9i0j1k2l..."
  },
  "spans": [
    {"name": "process_queue", "trace_id": "a1b2c3d4...", "span_id": "3m4n5o6p..."},
    {"name": "deliver_message", "trace_id": "a1b2c3d4...", "span_id": "9i0j1k2l..."}
  ],
  "file": "empath-delivery/src/lib.rs",
  "line": 456
}
```

**Querying Traces:**

LogQL queries can filter by trace ID to find all logs for a specific trace:

```logql
# Find all logs for a specific trace
{service="empath"} | json | span.trace_id="a1b2c3d4e5f6g7h8..."

# Find all logs for a specific message
{service="empath"} | json | message_id="01JCXYZ..."

# Find delivery failures and their trace IDs
{service="empath"} | json | level="ERROR" | line_format "{{.span.trace_id}}: {{.domain}}: {{.message}}"
```

**Jaeger UI:**

Access Jaeger at `http://localhost:16686` to visualize traces:

1. **Service Selection**: Select "empath-mta" service
2. **Operation**: Choose operation (e.g., "smtp_session", "deliver_message")
3. **Trace Timeline**: View the complete timeline of spans across components
4. **Span Details**: Click spans to see logs, tags, and metadata
5. **Dependency Graph**: Visualize service dependencies and call patterns

**Docker Stack Services:**

```yaml
# Jaeger (distributed tracing backend)
jaeger:
  image: jaegertracing/all-in-one:latest
  ports:
    - "16686:16686"  # Jaeger UI
    - "4317:4317"    # OTLP gRPC receiver
    - "4318:4318"    # OTLP HTTP receiver
  environment:
    - COLLECTOR_OTLP_ENABLED=true
    - SPAN_STORAGE_TYPE=badger  # Embedded database
  volumes:
    - jaeger-data:/badger  # Persistent storage

# OpenTelemetry Collector (trace routing)
otel-collector:
  image: otel/opentelemetry-collector-contrib:latest
  command: ["--config=/etc/otel-collector-config.yml"]
  volumes:
    - ./otel-collector.yml:/etc/otel-collector-config.yml
  ports:
    - "4318:4318"  # OTLP HTTP receiver

# Empath MTA (trace source)
empath:
  environment:
    - OTEL_EXPORTER_OTLP_ENDPOINT=http://otel-collector:4318
```

**Trace Storage:**

Jaeger uses Badger embedded database for trace storage:

- **Persistence**: Traces are stored in the `jaeger-data` Docker volume
- **Retention**: No automatic expiration (managed manually)
- **Performance**: Fast queries for recent traces, slower for historical data
- **Production**: Consider Elasticsearch or Cassandra backend for high-volume production

**Use Cases:**

1. **Debugging Delivery Failures**: Trace the complete path of a failed delivery
2. **Performance Analysis**: Identify slow components (DNS, SMTP handshake, etc.)
3. **Error Investigation**: Correlate errors across SMTP reception and delivery
4. **Capacity Planning**: Analyze trace durations to understand system load
5. **Dependency Mapping**: Visualize how components interact (SMTP → Spool → Delivery → MX servers)

**Implementation Details:**

- **Location**: `empath-common/src/logging.rs`
- **Tracer Provider**: Configured with OTLP HTTP exporter, batch processor, and resource attributes
- **Service Name**: `empath-mta` (configurable via `Resource::with_service_name`)
- **Sampling**: Always-on sampling (all traces collected)
- **Propagation**: Automatic trace context propagation across async tasks

**Benefits:**

- **End-to-End Visibility**: Track a message from SMTP reception to final delivery
- **Root Cause Analysis**: Quickly identify where failures occur in the pipeline
- **Performance Optimization**: Measure span durations to find bottlenecks
- **Log Correlation**: Jump from Jaeger trace to related Loki logs via trace_id
- **Service Insights**: Understand real-world service behavior and dependencies

**Starting the Stack:**

```bash
just docker-up         # Start Empath + OTEL + Jaeger + Loki + Grafana
just docker-logs       # View logs

# Access services:
# Grafana: http://localhost:3000 (admin/admin)
# Jaeger UI: http://localhost:16686
# Prometheus: http://localhost:9090
```

**Grafana Integration:**

The Jaeger datasource is pre-configured in Grafana with trace-to-logs correlation:

- Datasource: `docker/grafana/provisioning/datasources/jaeger.yml`
- Clicking a trace in Jaeger can jump to related Loki logs
- Loki logs include `trace_id` field for reverse correlation

### Data Flow

1. **Startup**: Load config → initialize modules → validate protocol args → start controllers (SMTP, spool, delivery)
2. **Connection**: Listener accepts → create Session → dispatch ConnectionOpened event
3. **Transaction**: Session receives data → parse Command → FSM transition → module validation → generate response
4. **Message Completion**: PostDot state → dispatch Data validation → spool message → respond to client
5. **Delivery**: Delivery controller scans spool → reads messages → prepares for sending (handshake only, no DATA)
6. **Shutdown**: Graceful shutdown sequence with delivery completion

### Graceful Shutdown

The system implements graceful shutdown to prevent message loss and ensure clean exit:

**Signal Handling:**
- Main controller listens for SIGTERM and SIGINT (Ctrl+C) via tokio::signal
- On signal receipt, broadcasts `Signal::Shutdown` to all components via `SHUTDOWN_BROADCAST`
- Second Ctrl+C forces immediate shutdown

**Delivery Processor Shutdown:**
When the delivery processor receives a shutdown signal:
1. **Stop accepting new work**: Scan and process timers are no longer serviced
2. **Wait for in-flight delivery**: Tracks current delivery with atomic flag, waits up to 30 seconds for completion
3. **Persist queue state**: Saves queue state to disk (`queue_state.bin`) for CLI access and recovery
4. **Exit cleanly**: Returns `Ok(())` after graceful shutdown completes

**Implementation Details:**
- Uses `Arc<AtomicBool>` to track if delivery is currently in progress
- Polls processing flag every 100ms during shutdown
- If timeout (30s) expires, logs warning and exits (message will retry on restart)
- All queue state is persisted before exit for recovery

**Location:** `empath-delivery/src/lib.rs:457-601`

**Testing:** Integration tests verify shutdown completes within timeout and handles both with/without in-flight deliveries

### Code Organization Patterns

#### Session Creation

To avoid clippy's too-many-arguments warning, use config struct:

```rust
Session::create(
    queue,
    stream,
    peer,
    SessionConfig {
        extensions: vec![...],
        tls_context: Some(...),
        spool: Some(...),
        banner: "hostname".to_string(),
        init_context: HashMap::new(),
    },
)
```

Location: `empath-smtp/src/session.rs:98`

#### Function Length Management

Keep functions under 100 lines (clippy::too_many_lines). Extract helper methods for complex logic:

Example from `empath-smtp/src/session.rs`:
- `response()` was 159 lines → refactored to ~80 lines
- Extracted `response_ehlo_help()` for EHLO/HELP handling
- Extracted `response_post_dot()` for message queuing and spooling

#### Collapsible If Statements

Use let-chains for nested Option/Result checks:

```rust
// Correct
if let Some(spool) = &self.spool
    && let Some(data) = &validate_context.data
{
    // ...
}

// Avoid (triggers clippy::collapsible_if)
if let Some(spool) = &self.spool {
    if let Some(data) = &validate_context.data {
        // ...
    }
}
```

#### FFI String Handling

Custom `String` and `StringVector` types for safe memory management:

```c
String id = em_context_get_id(ctx);
// Use id.data and id.len
em_free_string(id);  // Always free!

StringVector recipients = em_context_get_recipients(ctx);
for (int i = 0; i < recipients.len; i++) {
    // Use recipients.data[i].data
}
em_free_string_vector(recipients);  // Always free!
```

**Security: Null Byte Sanitization**

FFI string conversions automatically sanitize null bytes (`\0`) to prevent panics from malicious module input:
- Embedded null bytes are **removed** from strings (not replaced)
- The `len` field reflects the sanitized length, not the original
- Empty strings remain valid (non-null data pointer)
- This prevents modules from crashing the MTA via null byte injection attacks

Example:
```rust
// Input:  "test\0with\0nulls"
// Output: "testwithnulls" (sanitized, len=13)
// Input:  "\0\0\0"
// Output: "" (empty string, len=0, valid data pointer)
```

Location: `empath-ffi/src/string.rs`

### Testing Patterns

- **Integration tests**: Use `MemoryBackedSpool` for spool operations with `wait_for_count()` for async verification
- **FSM tests**: Test state transitions with various command sequences
- **Module tests**: Use `Module::TestModule` for testing without loading shared libraries
- **Async tests**: Mark with `#[tokio::test]`
- **E2E tests**: Use the `E2ETestHarness` for complete SMTP reception → spool → delivery flows

Example: `empath-smtp/src/session.rs:537` (spool_integration test)

### End-to-End (E2E) Testing

The project includes a comprehensive E2E test harness that verifies the complete message flow from SMTP reception through delivery.

**Location**: `empath/tests/support/harness.rs` (E2E test harness)
**Tests**: `empath/tests/e2e_basic.rs` (7 E2E tests covering delivery flows)

**E2E Test Harness Architecture**:

The `E2ETestHarness` provides a self-contained testing environment with:
- **Empath SMTP server** - Receives messages on a random port
- **Memory-backed spool** - Fast in-memory message persistence
- **Delivery processor** - Routes messages to the mock server
- **MockSmtpServer** - Simulates destination SMTP server for delivery verification

**Quick Example**:

```rust
#[tokio::test]
async fn test_delivery() {
    // Create test harness
    let harness = E2ETestHarness::builder()
        .with_test_domain("test.example.com")
        .build()
        .await
        .unwrap();

    // Send email via SMTP
    harness.send_email(
        "sender@example.org",
        "recipient@test.example.com",
        "Subject: Test\r\n\r\nHello World",
    ).await.unwrap();

    // Wait for delivery to mock server
    let message_content = harness
        .wait_for_delivery(Duration::from_secs(5))
        .await
        .unwrap();

    // Verify message content
    assert!(String::from_utf8(message_content)
        .unwrap()
        .contains("Hello World"));

    // Verify SMTP commands
    let commands = harness.mock_commands().await;
    assert!(commands.iter().any(|c| matches!(c, SmtpCommand::Data)));

    harness.shutdown().await;
}
```

**Running E2E Tests**:

```bash
# Run all E2E tests (single-threaded to avoid port conflicts)
cargo test --test e2e_basic -- --test-threads=1

# Run specific E2E test
cargo test --test e2e_basic test_full_delivery_flow_success

# With verbose output
cargo test --test e2e_basic -- --test-threads=1 --nocapture
```

**E2E Test Scenarios Covered**:

1. **Full delivery flow** - SMTP reception → spool → delivery → mock server verification
2. **Multiple recipients** - Handling multiple RCPT TO commands
3. **Recipient rejection** - Verifying proper handling of SMTP rejections (550 errors)
4. **Message content preservation** - Ensuring headers and body are preserved through the pipeline
5. **Graceful shutdown** - Verifying clean shutdown during delivery
6. **SMTP extensions** - Testing SIZE extension enforcement
7. **Custom delivery intervals** - Fast delivery with custom scan/process intervals

**MockSmtpServer Configuration**:

The mock server supports failure injection for testing error scenarios:

```rust
let harness = E2ETestHarness::builder()
    .with_test_domain("test.example.com")
    .with_mock_rcpt_rejection()  // 550 User unknown
    .build()
    .await
    .unwrap();
```

**Key Design Decisions**:

- **Memory-backed spool**: Fast tests without file I/O or inotify complexity
- **DNS fallback**: Cloudflare DNS (1.1.1.1) fallback when system DNS unavailable (see `empath-delivery/src/dns.rs:181`)
- **Random ports**: Both SMTP and mock servers bind to port 0 for automatic port assignment
- **Single-threaded**: Tests run with `--test-threads=1` to avoid port conflicts
- **Self-contained**: No Docker or external dependencies required

**CI Integration**:

E2E tests run in Gitea CI (`.gitea/workflows/test.yml`) after unit and integration tests:

```yaml
- name: Test E2E
  run: cargo nextest run --test e2e_basic --test-threads=1
```

**Performance**: All 7 E2E tests complete in ~43 seconds, verifying the complete delivery pipeline end-to-end.

### Benchmarking

The project includes comprehensive benchmarks using Criterion.rs for performance tracking:

**Available Benchmarks:**

1. **SMTP Benchmarks** (`empath-smtp/benches/smtp_benchmarks.rs`):
   - Command parsing (HELO, MAIL FROM, RCPT TO, etc.)
   - ESMTP parameter parsing with perfect hash map
   - FSM state transitions (single and full transaction)
   - Context operations and cloning

2. **Spool Benchmarks** (`empath-spool/benches/spool_benchmarks.rs`):
   - Message creation and builder pattern
   - Bincode serialization/deserialization
   - ULID generation and parsing
   - In-memory spool operations (write, read, list, delete)

**Running Benchmarks:**

```bash
# Run all benchmarks
cargo bench
# Or use justfile:
just bench

# Run specific crate benchmarks
cargo bench -p empath-smtp
cargo bench -p empath-spool
# Or use justfile:
just bench-smtp
just bench-spool
just bench-delivery

# Run specific benchmark group
cargo bench command_parsing
cargo bench fsm_transitions
cargo bench spool_write
# Or use justfile:
just bench-group command_parsing

# Verbose output
cargo bench -- --verbose
```

**Benchmark Baseline Tracking (Performance Regression Detection):**

The project uses Criterion's baseline feature to detect performance regressions. This is critical for validating optimizations and preventing silent degradation.

```bash
# Save current benchmarks as baseline (default: "main")
just bench-baseline-save
just bench-baseline-save my-optimization  # Custom baseline name

# Compare current performance against saved baseline
just bench-baseline-compare
just bench-baseline-compare my-optimization

# List all saved baselines
just bench-baseline-list

# Delete a baseline
just bench-baseline-delete my-optimization

# CI workflow: Compare against main baseline (for automated testing)
just bench-ci
```

**Baseline Workflow:**

1. **Save baseline on main branch:**
   ```bash
   git checkout main
   just bench-baseline-save main
   ```

2. **Make performance changes on feature branch:**
   ```bash
   git checkout -b optimize-parsing
   # ... make changes ...
   ```

3. **Compare against baseline:**
   ```bash
   just bench-baseline-compare main
   ```

4. **Review results:**
   - Green text: Performance improved
   - Red text: Performance regressed
   - Check HTML report for detailed analysis

**Recent Performance Optimizations:**

- Task 0.30: 90% metrics overhead reduction (AtomicU64 vs OpenTelemetry Counter)
- Task 4.3: Lock-free concurrency with DashMap (removed RwLock contention)
- Clone reduction: ~80% fewer clones in hot paths

**Benchmark Results:**

- HTML reports: `target/criterion/report/index.html`
- Individual benchmark data: `target/criterion/<benchmark_name>/`
- Flamegraphs (with `--features flamegraph`): Visualize performance hotspots

**Adding New Benchmarks:**

1. Add benchmark file to `benches/` directory in the relevant crate
2. Add `[[bench]]` section to `Cargo.toml` with `harness = false`
3. Add Criterion as dev-dependency
4. Use `criterion_group!` and `criterion_main!` macros
5. Follow existing patterns for consistency

**Performance Optimization Notes:**

- Recent work reduced clone usage by ~80% in hot paths (commit a09f603)
- Perfect hash map in MailParameters provides O(1) lookups
- Zero-allocation command parsing where possible
- Bincode for efficient serialization vs JSON

### Important Implementation Notes

1. **Nightly Features Required**: Edition 2024 with nightly features (ascii_char, associated_type_defaults, iter_advance_by, result_option_map_or_default, slice_pattern, vec_into_raw_parts, fn_traits, unboxed_closures)

2. **Async Runtime**: Tokio with multi-threaded runtime, parking_lot for synchronization

3. **Module Dispatch**: Synchronous - all modules called sequentially for each event. First non-zero return rejects transaction

4. **TLS Upgrade**: SMTP sessions start plaintext, upgrade via STARTTLS. Context preserved across upgrade

5. **Header Generation**: cbindgen runs during build to generate `empath.h` from FFI crate. Update `build.rs` dependencies if FFI API changes

6. **Strict Clippy**: All clippy warnings with pedantic/nursery lints must be fixed or explicitly allowed with justification

## Security Considerations

### TLS Certificate Validation

The delivery system validates TLS certificates by default to prevent Man-in-the-Middle attacks. However, for testing purposes, certificate validation can be disabled through a **two-tier configuration system**:

#### Global Configuration (DeliveryProcessor)

Set `accept_invalid_certs: true` in the delivery configuration to disable validation globally (affects all domains unless overridden):

```ron
delivery: (
    accept_invalid_certs: false,  // Default: false (secure)
    // ...
)
```

**SECURITY WARNING**: This setting should remain `false` in production environments.

#### Per-Domain Override (DomainConfig)

Individual domains can override the global setting:

```ron
delivery: (
    accept_invalid_certs: false,  // Global default: require valid certs
    domains: {
        "test.example.com": (
            accept_invalid_certs: true,   // Override: accept invalid for testing
        ),
        "secure.example.com": (
            accept_invalid_certs: false,  // Override: explicitly require valid
        ),
        "default.example.com": (
            // No override: uses global config
        ),
    },
)
```

**Configuration Priority**: Per-domain setting > Global setting

#### Security Warnings

When certificate validation is disabled, the system logs a warning:

```
SECURITY WARNING: TLS certificate validation is disabled for this connection
```

This appears in the logs with the domain and server address for audit purposes.

#### When to Use `accept_invalid_certs`

**✅ Acceptable use cases:**
- Local development with self-signed certificates
- Integration testing with test SMTP servers
- Staging environments with internal CAs

**❌ Never use in production:**
- Production email delivery
- Connections to public email providers (Gmail, Outlook, etc.)
- Any environment where security matters

#### Implementation Details

Location: `empath-delivery/src/lib.rs:748-763`

The delivery logic checks per-domain configuration first, then falls back to global configuration:

```rust
let accept_invalid_certs = self
    .domains
    .get(&delivery_info.recipient_domain)
    .and_then(|config| config.accept_invalid_certs)
    .unwrap_or(self.accept_invalid_certs);
```

### Authentication

Empath provides optional authentication for both the control socket and metrics endpoint to secure administrative access and observability data.

#### Control Socket Authentication

Token-based authentication using SHA-256 hashed bearer tokens protects control socket commands from unauthorized access.

**Configuration:**

```ron
control_auth: (
    enabled: true,
    token_hashes: [
        // SHA-256 hash of "admin-token-12345"
        "5e884898da28047151d0e56f8dc6292773603d0d6aabbdd62a11ef721d1542d8",
        // SHA-256 hash of "read-only-token"
        "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08",
    ],
)
```

**Generating Token Hashes:**

```bash
# Generate a secure token
TOKEN=$(openssl rand -hex 32)
echo "Token: $TOKEN"

# Generate SHA-256 hash for config
HASH=$(echo -n "$TOKEN" | sha256sum | awk '{print $1}')
echo "Hash for config: $HASH"

# Alternative: use a simple passphrase
echo -n "my-secret-password" | sha256sum
```

**Using Authenticated Clients:**

```bash
# Set token via environment variable
export EMPATH_TOKEN="admin-token-12345"

# Use with empathctl (future enhancement)
empathctl --token "$EMPATH_TOKEN" system status

# Or via programmatic client
```

```rust
use empath_control::ControlClient;

let client = ControlClient::new("/tmp/empath.sock")
    .with_token("admin-token-12345");

let response = client.send_request(request).await?;
```

**Security Features:**

- Tokens stored as SHA-256 hashes (not plaintext)
- Config file leaks don't expose tokens
- Multiple tokens supported (different access levels possible)
- Authentication failures logged with warnings
- Audit logging includes user and UID for all commands
- Backward compatible (disabled by default)

**Audit Logging:**

All authentication events are logged:

```
INFO  Control socket authentication is ENABLED
INFO  Control socket authentication successful user=alice uid=1000 command=System(Status)
WARN  Control socket authentication failed error="Invalid authentication token" command=Dns(ClearCache)
```

**When to Enable:**

- ✅ Multi-user systems where filesystem permissions aren't sufficient
- ✅ Production deployments with multiple administrators
- ✅ Compliance requirements for access control
- ✅ Remote access scenarios (though Unix sockets are local-only)

**When to Disable:**

- Single-user development environments
- Docker containers with isolated control sockets
- When filesystem permissions (mode 0600) are sufficient

**Implementation:** `empath-control/src/auth.rs`, `empath-control/src/server.rs:190-223`

#### Metrics Authentication

Optional API key authentication for OTLP metrics export protects observability data from unauthorized access.

**Configuration:**

```ron
metrics: (
    enabled: true,
    endpoint: "http://otel-collector:4318/v1/metrics",
    api_key: "your-secret-api-key-here",  // Optional
)
```

**How It Works:**

- API key sent in `Authorization: Bearer <key>` header
- Validation happens at the OTLP collector, not in Empath
- The collector must be configured to validate the key

**OTLP Collector Configuration Example:**

```yaml
# otel-collector-config.yaml
receivers:
  otlp:
    protocols:
      http:
        auth:
          authenticator: bearertokenauth

extensions:
  bearertokenauth:
    scheme: "Bearer"
    tokens:
      - token: "your-secret-api-key-here"

service:
  extensions: [bearertokenauth]
  pipelines:
    metrics:
      receivers: [otlp]
      exporters: [prometheus]
```

**Security Considerations:**

- ⚠️ API key stored in **plaintext** in config (required for OTLP protocol)
- ✅ Recommend using environment variable substitution:
  ```ron
  api_key: "$ENV:METRICS_API_KEY",  // Future enhancement
  ```
- ✅ Or Kubernetes secrets mounting:
  ```yaml
  env:
    - name: METRICS_API_KEY
      valueFrom:
        secretKeyRef:
          name: empath-secrets
          key: metrics-api-key
  ```

**When to Enable:**

- Production deployments with exposed metrics endpoints
- Multi-tenant environments
- Compliance requirements for metrics data protection

**Implementation:** `empath-metrics/src/exporter.rs:28-39`

**Comparison with Control Socket Auth:**

| Feature | Control Socket | Metrics Endpoint |
|---------|---------------|------------------|
| Storage | SHA-256 hash | Plaintext |
| Validation | In Empath | At OTLP collector |
| Protocol | Custom (bincode) | Standard (OTLP/HTTP) |
| Multiple Keys | Yes | Single key |
| Audit Logging | Yes (full) | No (collector-side) |

### SMTP Operation Timeouts

Both the server (receiving) and client (delivery) sides implement comprehensive timeouts to prevent hung connections and resource exhaustion.

#### Server-Side Timeouts (RFC 5321 Compliant)

The SMTP server implements state-aware timeouts that follow RFC 5321 Section 4.5.3.2 recommendations:

**Configuration** (in `smtp_controller` listener config):

```ron
timeouts: (
    command_secs: 300,          // 5 minutes for regular commands (EHLO, MAIL FROM, RCPT TO, etc.)
    data_init_secs: 120,        // 2 minutes for DATA command response
    data_block_secs: 180,       // 3 minutes between data chunks while receiving message
    data_termination_secs: 600, // 10 minutes for processing after final dot terminator
    connection_secs: 1800,      // 30 minutes maximum total session lifetime
),
```

**How It Works:**

- Timeouts are **state-aware**: The system automatically selects the appropriate timeout based on the current SMTP state
- `Reading` state (receiving message body): Uses `data_block_secs`
- `Data` state (waiting for DATA command): Uses `data_init_secs`
- `PostDot` state (processing after final `.`): Uses `data_termination_secs`
- All other states: Uses `command_secs`

**Connection Lifetime:**

The maximum session lifetime (`connection_secs`) is checked on every iteration of the session loop. When exceeded, the connection is automatically closed with a timeout error.

**Security Benefits:**

- Prevents slowloris attacks (clients that send data very slowly)
- Prevents resource exhaustion from hung connections
- Mitigates DoS vulnerabilities from clients holding resources indefinitely
- Protects against misbehaving SMTP clients

**Implementation:** `empath-smtp/src/session.rs:243-252, 267-278, 336-365`

#### Client-Side Timeouts (Delivery)

The delivery system implements per-operation timeouts for outbound SMTP connections:

**Configuration** (in `delivery` config):

```ron
smtp_timeouts: (
    connect_secs: 30,      // Initial connection establishment
    ehlo_secs: 30,         // EHLO/HELO commands
    starttls_secs: 30,     // STARTTLS command and TLS upgrade
    mail_from_secs: 30,    // MAIL FROM command
    rcpt_to_secs: 30,      // RCPT TO command (per recipient)
    data_secs: 120,        // DATA command and message transmission (longer for large messages)
    quit_secs: 10,         // QUIT command
),
```

**QUIT Timeout Behavior:**

Since QUIT occurs after successful delivery, timeout errors are logged but do not fail the delivery:

```rust
if let Err(e) = tokio::time::timeout(quit_timeout, client.quit()).await {
    tracing::warn!(
        server = %server_address,
        timeout = ?quit_timeout,
        "QUIT command timed out after successful delivery: {e}"
    );
}
```

**Implementation:** `empath-delivery/src/lib.rs:28-118, 987-995, 1004-1008, 1026-1049, 1061-1070, 1100-1108, 1141-1149, 1180-1215`

## Adding New Features

### Adding a New Protocol

1. Create crate (e.g., `empath-imap`)
2. Define State enum implementing `FiniteStateMachine`
3. Define Command/Input types
4. Create Session struct implementing `SessionHandler`
5. Implement `Protocol` trait with associated types
6. Add to main empath dependencies
7. Update configuration parser
8. Add protocol-specific controller to main.rs

### Adding New Module Events

1. Add event variant to `empath-ffi/src/modules/mod.rs`
2. Update module dispatch logic
3. Add callback to ValidationListener/EventListener struct
4. Update EM_DECLARE_MODULE macro if needed
5. Rebuild to regenerate empath.h
6. Document new event in examples

### Adding New Context Fields

1. Update `Context` struct in `empath-common/src/context.rs`
2. Add FFI accessor/mutator in `empath-ffi/src/lib.rs`
3. Mark with `#[no_mangle]` and `extern "C-unwind"`
4. Rebuild to regenerate empath.h
5. Update example modules to demonstrate usage

## Refactoring Guidelines

When refactoring to meet clippy requirements:

1. **Long Functions**: Extract logical chunks into private helper methods with clear names
2. **Similar Names**: Use descriptive, semantically different names (e.g., `header` vs `header_uppercase`)
3. **Type Conversions**: Use `try_from()` with error handling instead of `as` casts
4. **Lock Guards**: Minimize scope by using blocks or inline access patterns
5. **Documentation**: Add panic sections, use backticks for code terms, keep concise

All refactorings must maintain existing test coverage and functionality.
