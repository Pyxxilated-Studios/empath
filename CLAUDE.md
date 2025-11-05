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
cargo run -- empath.config.toml

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

# Generate C headers (happens automatically during build)
cargo build  # Outputs to empath/target/empath.h

# Build FFI example modules
cd empath-ffi/examples
gcc example.c -fpic -shared -o libexample.so -l empath -L ../../target/debug
gcc event.c -fpic -shared -o libevent.so -l empath -L ../../target/debug
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

6-crate workspace:

1. **empath** - Main binary/library orchestrating all components
2. **empath-common** - Core abstractions: `Protocol`, `FiniteStateMachine`, `Controller`, `Listener` traits
3. **empath-smtp** - SMTP protocol implementation with FSM and session management
4. **empath-ffi** - C-compatible API for embedding and dynamic module loading
5. **empath-spool** - Message persistence to filesystem with watching
6. **empath-tracing** - Procedural macros for `#[traced]` instrumentation

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
- `Controller`: Filesystem with atomic writes and directory watching
- `MockController`: In-memory for testing (with `wait_for_count` for async tests)

### Configuration

Runtime config via TOML (default: `empath.config.toml`):

```toml
# SMTP listeners with optional TLS
[[smtp.listener]]
socket = "[::]:1025"
[smtp.listener.tls]
certificate = "certificate.crt"
key = "private.key"
[smtp.listener.context]
# Custom key-value pairs passed to sessions
service = "smtp"

# Dynamically loaded modules
[[module]]
type = "SharedLibrary"
name = "./path/to/module.so"
arguments = ["arg1", "arg2"]

# Spool configuration
[spool]
path = "./spool/directory"
```

### Data Flow

1. **Startup**: Load config → initialize modules → validate protocol args → start controllers
2. **Connection**: Listener accepts → create Session → dispatch ConnectionOpened event
3. **Transaction**: Session receives data → parse Command → FSM transition → module validation → generate response
4. **Message Completion**: PostDot state → dispatch Data validation → spool message → respond to client
5. **Shutdown**: Broadcast signal → sessions close gracefully → controllers exit

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

- **Integration tests**: Use `MockController` for spool operations with `wait_for_count()` for async verification
- **FSM tests**: Test state transitions with various command sequences
- **Module tests**: Use `Module::TestModule` for testing without loading shared libraries
- **Async tests**: Mark with `#[tokio::test]` and `#[cfg_attr(all(target_os = "macos", miri), ignore)]`

Example: `empath-smtp/src/session.rs:537` (spool_integration test)

### Important Implementation Notes

1. **Nightly Features Required**: Edition 2024 with nightly features (ascii_char, associated_type_defaults, iter_advance_by, result_option_map_or_default, slice_pattern, vec_into_raw_parts, fn_traits, unboxed_closures)

2. **Async Runtime**: Tokio with multi-threaded runtime, parking_lot for synchronization

3. **Module Dispatch**: Synchronous - all modules called sequentially for each event. First non-zero return rejects transaction

4. **TLS Upgrade**: SMTP sessions start plaintext, upgrade via STARTTLS. Context preserved across upgrade

5. **Header Generation**: cbindgen runs during build to generate `empath.h` from FFI crate. Update `build.rs` dependencies if FFI API changes

6. **Strict Clippy**: All clippy warnings with pedantic/nursery lints must be fixed or explicitly allowed with justification

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
