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
```bash
just                # List all available commands
just setup          # Install development tools
just ci             # Run full CI check locally
just dev            # Development workflow (fmt + lint + test)
just test           # Run all tests
just lint           # Run strict clippy checks
just bench          # Run all benchmarks
just queue-list     # List queue messages
just queue-watch    # Live queue statistics
```

**Manual Cargo Commands:**
```bash
# Build and run
cargo build
cargo run -- empath.config.ron

# Testing
cargo test                    # All tests
cargo test -p empath-smtp     # Specific crate
cargo test test_name          # Specific test

# Lint (STRICT - enforces all/pedantic/nursery)
cargo clippy --all-targets --all-features

# Benchmarks
cargo bench                   # All benchmarks
cargo bench -p empath-smtp    # Specific crate
just bench-baseline-save      # Save baseline for regression detection
just bench-baseline-compare   # Compare against baseline

# Queue Management with empathctl
cargo build --bin empathctl
./target/debug/empathctl queue list
./target/debug/empathctl queue stats --watch
./target/debug/empathctl system status
./target/debug/empathctl dns clear-cache
```

### Docker Development Environment

Complete observability stack with OpenTelemetry, Prometheus, Grafana, Loki, and Jaeger:

```bash
just docker-up         # Start full stack
just docker-logs       # View logs
just docker-grafana    # Open Grafana (admin/admin)
just docker-down       # Stop stack
```

**Services:**
- Empath SMTP: `localhost:1025`
- Grafana: `http://localhost:3000` (admin/admin)
- Prometheus: `http://localhost:9090`
- Jaeger UI: `http://localhost:16686`
- Loki API: `http://localhost:3100`

See [`docker/README.md`](docker/README.md) for details.

## Clippy Configuration

STRICT linting enforced at workspace level:
- `clippy::all` = deny
- `clippy::pedantic` = deny
- `clippy::nursery` = deny

Key requirements:
- No wildcard imports
- Functions under 100 lines
- No similar variable names
- Add `# Panics` doc sections
- Use `try_from()` instead of `as` for casts
- Use byte string literals `b"..."` instead of `"...".as_bytes()`
- Minimize lock guard scope
- Document code items in backticks

## Architecture Overview

### Workspace Structure

10-crate workspace:
1. **empath** - Main binary orchestrating components
2. **empath-common** - Core traits (`Protocol`, `FiniteStateMachine`, `Controller`, `Listener`)
3. **empath-smtp** - SMTP protocol with FSM and session management
4. **empath-delivery** - Outbound delivery queue and processor
5. **empath-ffi** - C-compatible API for modules
6. **empath-health** - HTTP health check endpoints
7. **empath-metrics** - OpenTelemetry metrics
8. **empath-control** - Control socket IPC
9. **empath-spool** - Message persistence
10. **empath-tracing** - `#[traced]` instrumentation macros

### Key Architectural Patterns

#### 1. Generic Protocol System

New protocols implement the `Protocol` trait (`empath-common/src/traits/protocol.rs`):
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

#### 2. Finite State Machine Pattern

Protocol states managed via FSM trait (`empath-common/src/traits/fsm.rs`):
```rust
pub trait FiniteStateMachine {
    type Input;
    type Context;
    fn transition(self, input: Self::Input, context: &mut Self::Context) -> Self;
}
```

SMTP states: Connect, Ehlo, Helo, StartTLS, MailFrom, RcptTo, Data, Reading, PostDot, Quit

**SMTP Three-Layer Architecture:**

The SMTP protocol implementation uses a clean three-layer separation:

1. **Session State Layer** (`empath-smtp/src/session_state.rs`):
   - `SessionState` struct manages pure FSM state: client ID, extended mode flag, envelope (sender/recipients)
   - Separates protocol state from business logic concerns
   - Convertible to/from full business `Context` for backward compatibility
   - Serializable for persistence

2. **FSM Layer** (`empath-smtp/src/fsm.rs`):
   - Implements `FiniteStateMachine` trait for `State` enum
   - Pure state transitions with zero side effects
   - Uses only `SessionState` (not business `Context`)
   - Enables polymorphic FSM usage and testing in isolation

3. **Transaction Handler Layer** (`empath-smtp/src/transaction_handler.rs`):
   - `SmtpTransactionHandler` trait separates validation/spooling from protocol concerns
   - `DefaultSmtpTransactionHandler` dispatches to FFI module system
   - Async operations (spooling, validation, audit logging)
   - Called after FSM transitions for states requiring validation

**Design Benefits:**
- **Testability**: Each layer tested independently
- **Purity**: FSM transitions are pure functions
- **Flexibility**: Swap implementations (production vs testing)
- **Single Responsibility**: Clear separation of concerns (protocol vs business vs I/O)

**Example - Pure FSM Usage:**
```rust
use empath_common::traits::fsm::FiniteStateMachine;
use empath_smtp::{session_state::SessionState, state::State, command::Command};

let mut session_state = SessionState::new();
let state = State::default(); // Connect state

// Pure FSM transition - no side effects
let new_state = FiniteStateMachine::transition(
    state,
    Command::Helo(HeloVariant::Ehlo("client.example.com".to_string())),
    &mut session_state
);
```

**Example - Transaction Handler Usage:**
```rust
use empath_smtp::transaction_handler::{SmtpTransactionHandler, DefaultSmtpTransactionHandler};

let handler = DefaultSmtpTransactionHandler::new(Some(spool), peer);
let valid = handler.validate_connect(&mut business_context).await;
if valid {
    handler.handle_message(&mut business_context).await;
}
```

Location: `empath-smtp/src/state.rs:197-331` (transition methods), `empath-smtp/src/fsm.rs:49-84` (trait impl)

#### 3. Module/Plugin System

Two module types extend functionality without core modifications:
- **ValidationListener**: SMTP transaction validation (Connect, MailFrom, RcptTo, Data, StartTls)
- **EventListener**: Connection lifecycle (ConnectionOpened, ConnectionClosed)

**C API Interface:**
- Export `declare_module()` returning `Mod` struct
- Use `EM_DECLARE_MODULE` macro
- Access/modify `Context*` via `em_context_*` functions

Example: `empath-ffi/examples/example.c`

**Context Persistence:**
The `Context` struct persists ALL fields to spool (including session metadata) to enable modules to track state across the entire message lifecycle (SMTP reception → delivery). This allows plugins to:
- Set metadata during reception, access during delivery events
- Maintain coherent state without external storage
- Use spool as single source of truth

Location: `empath-common/src/context.rs`

#### 4. Controller/Listener Pattern

Two-tier connection management:
- **Controller**: Manages listeners, broadcasts shutdown (`empath-common/src/controller.rs`)
- **Listener**: Binds socket, accepts connections, spawns sessions (`empath-common/src/listener.rs`)

#### 5. Spool Abstraction

Message persistence via `Spool` trait (`empath-spool/src/spool.rs`):
- `FileBackedSpool`: Filesystem with atomic writes
- `MemoryBackedSpool`: In-memory for testing

### Configuration

Runtime config via RON (default: `empath.config.ron`). See config file for complete examples. Key sections:

```ron
Empath (
    smtp_controller: (
        listeners: [{
            socket: "[::]:1025",
            timeouts: (
                command_secs: 300,
                data_block_secs: 180,
                connection_secs: 1800,
            ),
            extensions: [{"size": 10000}, {"starttls": {...}}]
        }],
    ),
    spool: (path: "./spool"),
    delivery: (
        scan_interval_secs: 30,
        process_interval_secs: 10,
        max_attempts: 25,
        max_concurrent_deliveries: 8,
        domains: {
            "test.example.com": (
                mx_override: "localhost:1025",
            ),
        },
        rate_limit: (
            messages_per_second: 10.0,
            burst_size: 20,
            domain_limits: { ... },
        ),
        circuit_breaker: (
            failure_threshold: 5,
            timeout_secs: 300,
        ),
        smtp_timeouts: (
            connect_secs: 30,
            data_secs: 120,
            quit_secs: 10,
        ),
    ),
    control_socket: "/tmp/empath.sock",
    control_auth: (
        enabled: true,
        token_hashes: ["..."],  # SHA-256 hashes
    ),
    metrics: (
        enabled: true,
        endpoint: "http://localhost:4318/v1/metrics",
        api_key: "...",  # Optional
    ),
    health: (
        enabled: true,
        listen_address: "[::]:8080",
        max_queue_size: 10000,
    ),
    dsn: (
        enabled: true,
        reporting_mta: "mail.example.com",
        postmaster: "postmaster@example.com",
    ),
)
```

### Runtime Control via Control Socket

Commands via `empathctl` using Unix domain socket IPC:

**DNS Management:**
```bash
empathctl dns list-cache
empathctl dns clear-cache
empathctl dns refresh example.com
```

**System Status:**
```bash
empathctl system ping
empathctl system status
```

**Queue Management:**
```bash
empathctl queue list
empathctl queue stats --watch
empathctl queue retry <message-id>
```

**Security:**
- Socket permissions: mode 0600 (owner only) by default
- Optional token-based authentication (SHA-256 hashed)
- All commands audit logged (user, UID, command, status)

Implementation: `empath-control` crate, `empath/src/control_handler.rs`

### Health Check Endpoints

HTTP endpoints for Kubernetes probes (`empath-health` crate):

- **`/health/live`**: Liveness probe (200 OK if alive)
- **`/health/ready`**: Readiness probe (200 OK if all systems ready, 503 with JSON status otherwise)

Readiness checks: SMTP listeners, spool writability, delivery processor, DNS resolver, queue size threshold.

### Delivery Features

#### Delivery Status Notifications (DSNs)

RFC 3464 compliant bounce messages generated for:
- Permanent failures (5xx SMTP errors)
- Exhausted retry attempts

Not generated for:
- Null sender (`MAIL FROM:<>`) - prevents loops
- Temporary failures in retry state

DSN format: `multipart/report` with human-readable explanation, machine-readable status, original headers.

Implementation: `empath-delivery/src/dsn.rs`

#### Rate Limiting

Per-domain token bucket algorithm prevents overwhelming recipient servers:
- Configurable `messages_per_second` and `burst_size`
- Per-domain overrides for high-volume providers
- Rate-limited messages rescheduled (not failed)
- Metrics tracked per domain

Implementation: `empath-delivery/src/rate_limiter.rs` (DashMap for lock-free lookup)

#### Circuit Breakers

Prevent retry storms during server outages:
- States: Closed (normal) → Open (rejecting) → Half-Open (testing)
- Tracks temporary failures (connection timeouts, DNS errors)
- Permanent failures (5xx) don't trip circuit
- Configurable thresholds and recovery timeout

Implementation: `empath-delivery/src/circuit_breaker.rs`

#### Parallel Delivery Processing

`tokio::task::JoinSet` processes multiple messages concurrently:
- `max_concurrent_deliveries` controls workers (default: num_cpus)
- Thread-safe rate limiting and circuit breakers
- 5-8x throughput improvement over sequential
- Graceful shutdown waits for in-flight deliveries

Implementation: `empath-delivery/src/processor/process.rs`

### Observability

#### JSON Structured Logging

All logs output as JSON with structured fields:
- `message_id`, `domain`, `delivery_attempt`, `smtp_code`, `sender`, `recipient`
- Trace correlation: `trace_id`, `span_id` in every log entry
- Control via `RUST_LOG` environment variable

Implementation: `empath-common/src/logging.rs`

**Common LogQL Queries:**
```logql
# Track specific message
{service="empath"} | json | fields.message_id="01JCXYZ..."

# Delivery failures by domain
{service="empath"} | json | level="ERROR" | fields.domain="example.com"

# Count errors by domain
sum by (fields_domain) (count_over_time({service="empath"} | json | level="ERROR" [1h]))

# Rate limited deliveries
{service="empath"} | json | fields.message=~"Rate limit exceeded"

# Circuit breaker trips
{service="empath"} | json | fields.message=~"Circuit breaker OPENED"
```

#### Log Aggregation with Loki

Docker stack includes Loki, Promtail, and Grafana with pre-configured dashboards:
- 7-day retention
- Automatic Docker log collection
- Pre-built "Empath MTA - Log Exploration" dashboard

Access: `http://localhost:3000` (Grafana) or `http://localhost:3100` (Loki API)

#### Distributed Tracing with OpenTelemetry

End-to-end visibility from SMTP reception through delivery:
- Trace IDs link operations across components
- Automatic trace context in all logs
- Jaeger UI at `http://localhost:16686`
- Log-to-trace correlation via `trace_id` field

Configuration: `OTEL_EXPORTER_OTLP_ENDPOINT` environment variable

Implementation: `empath-common/src/logging.rs` (OTLP HTTP exporter)

### Data Flow

1. **Startup**: Load config → init modules → start controllers
2. **Connection**: Listener accepts → create Session → dispatch events
3. **Transaction**: Receive data → parse Command → FSM transition → module validation
4. **Completion**: PostDot → dispatch Data validation → spool → respond
5. **Delivery**: Scan spool → process queue in parallel → DNS → SMTP delivery
6. **Shutdown**: Graceful shutdown waits for in-flight deliveries (30s timeout)

### Graceful Shutdown

- SIGTERM/SIGINT handled via tokio::signal
- Broadcasts `Signal::Shutdown` to all components
- Delivery processor: stops new work, waits for in-flight tasks, persists queue state
- JoinSet ensures all parallel tasks complete
- Queue state saved to `queue_state.bin` for recovery

Location: `empath-delivery/src/processor/mod.rs:326-450`

### Code Organization Patterns

#### Session Creation
Use config struct to avoid too-many-arguments:
```rust
Session::create(queue, stream, peer, SessionConfig { ... })
```

#### Function Length
Keep under 100 lines. Extract helpers for complex logic.

#### Collapsible If Statements
Use let-chains:
```rust
if let Some(spool) = &self.spool
    && let Some(data) = &validate_context.data
{ ... }
```

#### FFI String Handling

Custom `String` and `StringVector` types with null byte sanitization:
```c
String id = em_context_get_id(ctx);
em_free_string(id);  // Always free!
```

Null bytes removed to prevent crashes from malicious modules.

Location: `empath-ffi/src/string.rs`

### Testing Patterns

- **Integration tests**: Use `MemoryBackedSpool` with `wait_for_count()`
- **FSM tests**: Test state transitions
- **Module tests**: Use `Module::TestModule`
- **Async tests**: Mark with `#[tokio::test]`
- **E2E tests**: Use `E2ETestHarness` for complete SMTP → spool → delivery flows

#### E2E Testing

Comprehensive harness in `empath/tests/support/harness.rs`:
- Self-contained environment (SMTP server, mock server, memory spool)
- 7 test scenarios: full flow, multiple recipients, rejections, shutdown, extensions
- Run with `cargo test --test e2e_basic -- --test-threads=1`
- ~43 seconds for full suite

### Benchmarking

Criterion.rs benchmarks in `empath-smtp` and `empath-spool`:
- Command parsing, FSM transitions
- Spool operations, serialization
- Baseline tracking for regression detection

```bash
cargo bench
just bench-baseline-save main        # Save baseline
just bench-baseline-compare main      # Compare against baseline
```

Results: `target/criterion/report/index.html`

### Important Implementation Notes

1. **Nightly Features Required**: Edition 2024 with nightly features
2. **Async Runtime**: Tokio with multi-threaded runtime, parking_lot for synchronization
3. **Module Dispatch**: Synchronous - all modules called sequentially
4. **TLS Upgrade**: STARTTLS preserves context across upgrade
5. **Header Generation**: cbindgen generates `empath.h` during build
6. **Strict Clippy**: All warnings must be fixed or explicitly allowed

## Security Considerations

### TLS Certificate Validation

Validates certs by default. Testing override via two-tier config:
- Global: `delivery.accept_invalid_certs`
- Per-domain: `delivery.domains.{domain}.accept_invalid_certs`

**WARNING**: Keep `false` in production. Only use for testing with self-signed certs.

Location: `empath-delivery/src/lib.rs:748-763`

### Authentication

**Control Socket**: Optional SHA-256 token-based auth. Generate tokens:
```bash
echo -n "my-secret" | sha256sum
```

Configure via `control_auth.token_hashes`. All auth events audit logged.

**Metrics Endpoint**: Optional API key in `Authorization: Bearer` header. Validated at OTLP collector.

Implementation: `empath-control/src/auth.rs`, `empath-metrics/src/exporter.rs:28-39`

### SMTP Timeouts

**Server-Side** (RFC 5321 compliant):
- State-aware timeouts: `command_secs`, `data_block_secs`, `data_termination_secs`
- Max session lifetime: `connection_secs`
- Prevents slowloris and resource exhaustion

**Client-Side** (Delivery):
- Per-operation: `connect_secs`, `ehlo_secs`, `data_secs`, `quit_secs`
- QUIT timeout doesn't fail delivery

Implementation: `empath-smtp/src/session.rs`, `empath-delivery/src/lib.rs:28-118`

## Adding New Features

### Adding a New Protocol
1. Create crate with State enum implementing `FiniteStateMachine`
2. Define Command/Input types
3. Create Session implementing `SessionHandler`
4. Implement `Protocol` trait
5. Add to main dependencies and config parser

### Adding New Module Events
1. Add event variant to `empath-ffi/src/modules/mod.rs`
2. Update dispatch logic and callbacks
3. Rebuild to regenerate `empath.h`

### Adding New Context Fields
1. Update `Context` in `empath-common/src/context.rs`
2. Add FFI accessor/mutator in `empath-ffi/src/lib.rs`
3. Mark `#[no_mangle]` and `extern "C-unwind"`
4. Rebuild and update examples

## Refactoring Guidelines

1. **Long Functions**: Extract helpers with clear names
2. **Similar Names**: Use semantically different names
3. **Type Conversions**: Use `try_from()` instead of `as`
4. **Lock Guards**: Minimize scope
5. **Documentation**: Add panic sections, use backticks

All refactorings must maintain test coverage and functionality.
