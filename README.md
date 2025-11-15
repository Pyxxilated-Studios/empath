# Empath MTA

[![License](https://img.shields.io/badge/license-Apache%202.0-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.93.0--nightly+-orange.svg)](https://www.rust-lang.org)

**Empath** is a modern Mail Transfer Agent (MTA) written in Rust, designed to be fully functional, easy to debug, and extensible through a dynamic plugin system.

> ⚠️ **Status**: Work in Progress - This project is actively being developed and should not be used in production environments yet.

## Features

- ✉️ **Full MTA Functionality** - Complete SMTP server and client implementation
- 🔌 **Plugin System** - Extensible via FFI with dynamic module loading (C, C++, Rust, etc.)
- 🛠️ **Easy Debugging** - Built with testability and debugging in mind
- 📦 **Embeddable** - Can be embedded in other applications (produces cdylib for each crate)
- 🔒 **Secure** - TLS support with STARTTLS, certificate validation, and security-first design
- 📊 **Observable** - OpenTelemetry integration with Prometheus and Grafana support
- ⚡ **Performance** - Written in Rust for speed and safety, with async I/O via Tokio
- 🎯 **Modern Architecture** - Clean separation of concerns with trait-based protocols

## Quick Start

### Prerequisites

- Rust nightly toolchain (`rustc 1.93.0-nightly` or later)
- Just task runner (optional but recommended): `cargo install just`

### Installation

```bash
# Clone the repository
git clone https://github.com/Pyxxilated-Studios/empath.git
cd empath

# Build the project
cargo build --release

# Or use the justfile for a complete development setup
just setup   # Install development tools
just build   # Build all crates
```

### Running Empath

```bash
# Run with default configuration
cargo run

# Run with custom configuration
cargo run -- path/to/config.ron

# Or use the release binary
./target/release/empath empath.config.ron
```

### Basic Configuration

Create a `empath.config.ron` file:

```ron
Empath (
    smtp_controller: (
        listeners: [
            {
                socket: "[::]:1025",
                context: {
                    "service": "smtp",
                },
                timeouts: (
                    command_secs: 300,
                    data_init_secs: 120,
                    data_block_secs: 180,
                    data_termination_secs: 600,
                    connection_secs: 1800,
                ),
            },
        ],
    ),
    spool: (
        path: "./spool",
    ),
    delivery: (
        scan_interval_secs: 30,
        process_interval_secs: 10,
        max_attempts: 25,
    ),
    control_socket: "/tmp/empath.sock",
)
```

### Testing the SMTP Server

```bash
# Send a test email using telnet
telnet localhost 1025

# Or use the Docker test helper (requires just)
just docker-test-email
```

## Architecture

Empath is built as a modular workspace with clear separation of concerns:

### Core Crates

- **empath** - Main binary/library orchestrating all components
- **empath-common** - Core abstractions (`Protocol`, `FiniteStateMachine`, `Controller` traits)
- **empath-smtp** - SMTP protocol implementation with FSM and session management
- **empath-delivery** - Outbound mail delivery queue and processor with MX lookup
- **empath-spool** - Message persistence to filesystem with watching
- **empath-control** - Control socket IPC for runtime management
- **empath-metrics** - OpenTelemetry metrics integration
- **empath-ffi** - C-compatible API for plugins and embedding

### Key Design Patterns

1. **Generic Protocol System** - New protocols implement the `Protocol` trait
2. **Finite State Machine** - SMTP states managed via FSM pattern for correctness
3. **Plugin/Module System** - Extend functionality via FFI without modifying core
4. **Controller/Listener** - Two-tier connection management for clean shutdown
5. **Spool Abstraction** - Pluggable storage backends (file, memory, database)

```
┌─────────────┐
│   empath    │  Main orchestrator
└──────┬──────┘
       │
       ├──────┬──────────┬──────────┬──────────┐
       │      │          │          │          │
   ┌───▼───┐ │      ┌───▼───┐  ┌───▼────┐ ┌───▼────┐
   │ SMTP  │ │      │Deliv  │  │ Spool  │ │Control │
   │       │ │      │ery    │  │        │ │        │
   └───────┘ │      └───────┘  └────────┘ └────────┘
             │
         ┌───▼───┐
         │  FFI  │  Plugin interface
         │Modules│
         └───────┘
```

## Runtime Control

Empath provides runtime control via the `empathctl` CLI utility:

```bash
# Health check
empathctl system ping

# View system status
empathctl system status

# DNS cache management
empathctl dns list-cache
empathctl dns clear-cache
empathctl dns refresh example.com

# Queue management
empathctl queue list
empathctl queue stats
empathctl queue stats --watch  # Live updates
empathctl queue view <message-id>
empathctl queue retry <message-id>
empathctl queue delete <message-id> --yes
```

## Development

### Common Commands

```bash
# Quick start (installs tools)
just setup

# Development workflow
just dev         # Format + lint + test
just ci          # Full CI check locally

# Testing
just test        # Run all tests
just test-quick  # Fast test run

# Linting (STRICT - clippy::all + pedantic + nursery)
just lint        # Run clippy with strict rules
just fmt         # Format code

# Benchmarks
just bench       # Run all benchmarks
```

### Project Structure

```
empath/
├── empath/              # Main binary and orchestration
├── empath-common/       # Core traits and abstractions
├── empath-smtp/         # SMTP protocol implementation
├── empath-delivery/     # Outbound delivery processor
├── empath-spool/        # Message persistence layer
├── empath-control/      # Control socket IPC
├── empath-metrics/      # OpenTelemetry metrics
├── empath-ffi/          # FFI and plugin system
├── empath-tracing/      # Tracing macros
└── docker/              # Docker development environment
```

### Plugin Development

Empath supports dynamic modules for extending functionality:

```c
// example.c - Simple validation module
#include "empath.h"

int validate_mail_from(Context* ctx) {
    String sender = em_context_get_sender(ctx);
    // Validation logic
    em_free_string(sender);
    return 0;  // 0 = success, non-zero = reject
}

EM_DECLARE_MODULE(
    validation,                    // Module name
    validate_mail_from,           // MailFrom handler
    NULL,                         // RcptTo handler
    NULL,                         // Data handler
    NULL                          // StartTLS handler
);
```

Build and load:

```bash
gcc example.c -fpic -shared -o libexample.so -l empath -L target/release
```

## Docker Development Environment

Complete Docker stack with observability:

```bash
# Start full stack (Empath + OpenTelemetry + Prometheus + Grafana)
just docker-up

# View logs
just docker-logs

# Open Grafana dashboard
just docker-grafana  # Opens http://localhost:3000 (admin/admin)

# Send test email
just docker-test-email

# Cleanup
just docker-down
```

Services:
- Empath SMTP: `localhost:1025`
- Grafana: `http://localhost:3000`
- Prometheus: `http://localhost:9090`
- OTEL Collector: `http://localhost:4318`

## Testing

Empath has comprehensive test coverage:

```bash
# Run all tests
cargo test

# Run tests for specific crate
cargo test -p empath-smtp

# Run benchmarks
cargo bench
```

## Security

- ✅ TLS certificate validation (configurable per-domain)
- ✅ STARTTLS support for encrypted connections
- ✅ Input validation and sanitization
- ✅ SMTP operation timeouts (RFC 5321 compliant)
- ✅ Path traversal prevention in spool
- ✅ Audit logging for control commands
- ⚠️ Authentication for metrics/control endpoints (TODO before production)

See `CLAUDE.md` for detailed security considerations.

## Configuration

For detailed configuration options, see:
- `CLAUDE.md` - Complete project documentation
- `empath.config.ron` - Example configuration
- `docker/empath.config.ron` - Docker environment config

## Contributing

Contributions are welcome! Please:

1. Follow the existing code style (enforced by clippy)
2. Add tests for new functionality
3. Update documentation as needed
4. Run `just ci` before submitting PRs

### Code Quality

This project uses STRICT clippy linting:
- `clippy::all` = deny
- `clippy::pedantic` = deny
- `clippy::nursery` = deny

All code must pass `cargo clippy --all-targets --all-features`.

## Documentation

- **CLAUDE.md** - Comprehensive technical documentation for development
- **docker/README.md** - Docker environment documentation
- **TODO.md** - Project roadmap and future work

## License

Licensed under the Apache License, Version 2.0. See [LICENSE](LICENSE) for details.

## Roadmap

See [TODO.md](TODO.md) for the complete roadmap. Current priorities:

- 🔴 Authentication for metrics and control endpoints
- 🟡 Persistent delivery queue
- 🟡 DNSSEC validation
- 🟡 Connection pooling for SMTP client
- 🟡 Enhanced test coverage

## Acknowledgments

Built with:
- [Tokio](https://tokio.rs/) - Async runtime
- [Hickory DNS](https://github.com/hickory-dns/hickory-dns) - DNS resolver
- [OpenTelemetry](https://opentelemetry.io/) - Observability
- [serde](https://serde.rs/) - Serialization
- And many other excellent Rust crates

---

**Note**: Empath is under active development. APIs and configurations may change. Not recommended for production use yet.
