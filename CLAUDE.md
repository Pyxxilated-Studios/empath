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

# Lint with clippy (STRICT - project enforces all/pedantic/nursery)
cargo clippy --all-targets --all-features -- -D clippy::all -D clippy::pedantic -D clippy::nursery -A clippy::must_use_candidate

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

# Queue Management with empathctl
cargo build --bin empathctl            # Build queue management CLI
./target/debug/empathctl queue list    # List all messages in queue
./target/debug/empathctl queue list --status=failed  # List only failed messages
./target/debug/empathctl queue view <message-id>  # View message details
./target/debug/empathctl queue delete <message-id> --yes  # Delete message
./target/debug/empathctl queue retry <message-id>   # Retry failed delivery
./target/debug/empathctl queue freeze    # Pause delivery processing
./target/debug/empathctl queue unfreeze  # Resume delivery processing
./target/debug/empathctl queue stats     # Show queue statistics
./target/debug/empathctl queue stats --watch --interval 2  # Live stats view
```

## Clippy Configuration

This project uses STRICT clippy linting. All changes must pass:

```bash
cargo clippy --all-targets --all-features -- \
  -D clippy::all \
  -D clippy::pedantic \
  -D clippy::nursery \
  -A clippy::must_use_candidate
```

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

7-crate workspace:

1. **empath** - Main binary/library orchestrating all components
2. **empath-common** - Core abstractions: `Protocol`, `FiniteStateMachine`, `Controller`, `Listener` traits
3. **empath-smtp** - SMTP protocol implementation with FSM and session management
4. **empath-delivery** - Outbound mail delivery queue and processor
5. **empath-ffi** - C-compatible API for embedding and dynamic module loading
6. **empath-spool** - Message persistence to filesystem with watching
7. **empath-tracing** - Procedural macros for `#[traced]` instrumentation

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
    ),
)
```

### Data Flow

1. **Startup**: Load config → initialize modules → validate protocol args → start controllers (SMTP, spool, delivery)
2. **Connection**: Listener accepts → create Session → dispatch ConnectionOpened event
3. **Transaction**: Session receives data → parse Command → FSM transition → module validation → generate response
4. **Message Completion**: PostDot state → dispatch Data validation → spool message → respond to client
5. **Delivery**: Delivery controller scans spool → reads messages → prepares for sending (handshake only, no DATA)
6. **Shutdown**: Broadcast signal → sessions close gracefully → controllers exit

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

Location: `empath-ffi/src/string.rs`

### Testing Patterns

- **Integration tests**: Use `MemoryBackedSpool` for spool operations with `wait_for_count()` for async verification
- **FSM tests**: Test state transitions with various command sequences
- **Module tests**: Use `Module::TestModule` for testing without loading shared libraries
- **Async tests**: Mark with `#[tokio::test]` and `#[cfg_attr(all(target_os = "macos", miri), ignore)]`

Example: `empath-smtp/src/session.rs:537` (spool_integration test)

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

# Run specific crate benchmarks
cargo bench -p empath-smtp
cargo bench -p empath-spool

# Run specific benchmark group
cargo bench command_parsing
cargo bench fsm_transitions
cargo bench spool_write

# Verbose output
cargo bench -- --verbose

# Save baseline for comparison
cargo bench -- --save-baseline main

# Compare against baseline
cargo bench -- --baseline main
```

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
