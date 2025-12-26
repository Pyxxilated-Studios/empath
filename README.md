# Empath MTA

[![License](https://img.shields.io/badge/license-Apache%202.0-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.93.0--nightly+-orange.svg)](https://www.rust-lang.org)

**Empath** is a modern Mail Transfer Agent (MTA) written in Rust, designed to be fully functional, easy to debug, and extensible through a dynamic plugin system.

> ⚠️ **Status**: Work in Progress — actively developed, not production-ready.

## What you get

- SMTP server + control socket, strict FSM, TLS/STARTTLS, size limits, and audit logging
- Delivery engine with spool persistence, MX lookup, retries/backoff, and DSN generation
- Module system: FFI and WASM components for validation/extension across languages
- Observability: OpenTelemetry traces/metrics, structured logs, and Docker dev stack
- Strict linting/formatting, regression tests, and reproducible config samples

## Quick Start

### Prerequisites

- Rust nightly toolchain (`rustc 1.93.0-nightly` or later, see `rust-toolchain.toml`)
- `just` task runner (optional but recommended): `cargo install just`

### Build and run

```bash
git clone https://github.com/Pyxxilated-Studios/empath.git
cd empath

# Build everything (or use `just build`)
cargo build

# Run the MTA with local config
cargo run --bin empath -- empath.config.ron

# Control CLI examples
cargo run --bin empathctl -- queue stats
cargo run --bin empathctl -- system status
```

### Testing the SMTP server

```bash
# Send a test email using telnet
telnet localhost 1025

# Or use the Docker test helper (requires just)
just docker-test-email
```

## Architecture (high level)

- **empath**: main binary and controller loop
- **empath-common**: shared types (contexts, envelope, newtypes, tracing macros)
- **empath-smtp**: server/session FSM, SMTP commands, control socket
- **empath-delivery**: spool-backed queue, MX resolution, retries/DSNs
- **empath-spool**: file/memory spool backends
- **empath-ffi** / **empath-wasm**: module interfaces for FFI and WASM

## Runtime Control

Empath provides runtime control via the `empathctl` CLI utility:

```bash
# Health check
empathctl system ping

# View system status
empathctl system status

# Manage advertised SMTP capabilities
empathctl system capabilities
empathctl system enable-capability AUTH
empathctl system disable-capability AUTH

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

## Operations Runbook

- Day-to-day operations guide: `docs/OPERATOR_RUNBOOK.md`

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
just test-fast   # Fast test run (skips slow integration suites)

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
# Fast path
just test-nextest          # if `cargo nextest` is installed
just test                  # or `cargo test`

# Lint/format
just fmt && just lint

# Benchmarks
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
 - `docs/` - Additional notes (nightly features, completed tasks)

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
- **docs/LOAD_TESTING.md** - Load and stress testing guide (Locust/k6)
- **docs/message-mutation.md** - Examples for Python and C modules that mutate message headers/body
- **docs/DEV_TOOLS.md** - Explanation of `just doctor`, `just check-changed`, and other helper scripts

## License

Licensed under the Apache License, Version 2.0. See [LICENSE](LICENSE) for details.

## Roadmap

See [TODO.md](TODO.md) for the complete roadmap. Current priorities:

- 🔴 Authentication for metrics/control endpoints
- 🟡 Delivery queue polish (pooling, bindings, DR/backups)
- 🟡 WASM module ergonomics and coverage
- 🟡 Observability and DNSSEC validation

## Acknowledgments

Built with:
- [Tokio](https://tokio.rs/) - Async runtime
- [Hickory DNS](https://github.com/hickory-dns/hickory-dns) - DNS resolver
- [OpenTelemetry](https://opentelemetry.io/) - Observability
- [serde](https://serde.rs/) - Serialization
- And many other excellent Rust crates

---

**Note**: Empath is under active development. APIs and configurations may change. Not recommended for production use yet.
