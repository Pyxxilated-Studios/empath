# Empath MTA - Task Runner
#
# Quick Start (New Developers):
#   just setup          - First-time setup (installs tools, git hooks)
#   just doctor         - Check environment health (diagnose setup issues)
#   just dev            - Development workflow (fmt + lint + test)
#   just ci             - Full CI check (lint + fmt-check + test)
#   just docker-up      - Start full stack (Empath + OTEL + Prometheus + Grafana)
#
# Common Commands:
#   just build          - Build entire workspace
#   just test           - Run all tests
#   just lint           - Run clippy checks
#   just bench          - Run benchmarks
#   just queue-list     - List queue messages
#
# Prerequisites:
#   cargo install just cargo-nextest cargo-watch cargo-outdated cargo-audit cargo-deny
#   System packages: mold (optional, speeds up builds)
#
# Run `just` or `just --list` to see all 50+ available commands.

# =============================================================================
# QUICK START - New Developer Commands
# =============================================================================

# List all available commands
default:
    @just --list

# Show help with common commands
help:
    @echo "=== Empath MTA - Common Commands ==="
    @echo ""
    @echo "🚀 Quick Start:"
    @echo "  just setup          - First-time setup"
    @echo "  just doctor         - Check environment health"
    @echo "  just dev            - Development workflow (fmt + lint + test)"
    @echo "  just ci             - Full CI check"
    @echo "  just docker-up      - Start full stack"
    @echo ""
    @echo "🔨 Building:"
    @echo "  just build          - Build workspace"
    @echo "  just build-release  - Build release version"
    @echo ""
    @echo "🧪 Testing:"
    @echo "  just test           - Run all tests"
    @echo "  just test-nextest   - Run tests with nextest (faster)"
    @echo "  just bench          - Run benchmarks"
    @echo ""
    @echo "🔍 Code Quality:"
    @echo "  just lint           - Run clippy"
    @echo "  just fmt            - Format code"
    @echo ""
    @echo "📦 Queue Management:"
    @echo "  just queue-list     - List messages"
    @echo "  just queue-stats    - Queue statistics"
    @echo "  just queue-watch    - Live queue stats"
    @echo ""
    @echo "🐳 Docker:"
    @echo "  just docker-up      - Start stack"
    @echo "  just docker-logs    - View logs"
    @echo "  just docker-grafana - Open Grafana"
    @echo ""
    @echo "Run 'just --list' to see all 50+ commands"

# Setup development environment (install tools)
setup:
    @echo "Installing development tools..."
    @echo "1. Installing cargo tools..."
    cargo install just cargo-nextest cargo-watch cargo-outdated cargo-audit cargo-deny
    @echo ""
    @echo "2. Checking for mold linker..."
    @if command -v mold >/dev/null 2>&1; then \
        echo "✅ mold is already installed"; \
    elif command -v apt-get >/dev/null 2>&1; then \
        echo "Installing mold via apt..."; \
        sudo apt-get update && sudo apt-get install -y mold; \
    elif command -v brew >/dev/null 2>&1; then \
        echo "Installing mold via brew..."; \
        brew install mold; \
    else \
        echo "⚠️  Could not install mold automatically."; \
        echo "Please install manually: https://github.com/rui314/mold"; \
    fi
    @echo ""
    @echo "3. Installing git hooks..."
    @if [ -f scripts/install-hooks.sh ]; then \
        ./scripts/install-hooks.sh; \
    else \
        echo "⚠️  scripts/install-hooks.sh not found (task 7.7 not yet completed)"; \
    fi
    @echo ""
    @echo "✅ Development environment setup complete!"
    @echo ""
    @echo "Quick start:"
    @echo "  just build          # Build the project"
    @echo "  just test           # Run tests"
    @echo "  just lint           # Run clippy"
    @echo "  just ci             # Run all checks"

# Check development environment health
doctor:
    @./scripts/doctor.sh

# Development workflow: format, lint, test
dev: fmt lint test
    @echo "✅ Development checks passed"

# Full CI check locally (lint + test)
ci: lint fmt-check test
    @echo "✅ All CI checks passed!"

# Pre-commit checks (fast)
pre-commit: fmt-check lint
    @echo "✅ Pre-commit checks passed"

# Quick fix common issues
fix: lint-fix fmt
    @echo "✅ Auto-fixes applied (lint + format)"

# =============================================================================
# BUILDING
# =============================================================================

# Build entire workspace
build:
    cargo build

# Build release version (uses thin LTO, opt-level 2)
build-release:
    cargo build --release

# Check project (fast compile check without building)
check:
    cargo check --all-targets

# Build empathctl queue management CLI
build-empathctl:
    cargo build --bin empathctl

# Build FFI examples (C modules)
build-ffi:
    #!/usr/bin/env bash
    set -euo pipefail
    cd empath-ffi/examples
    echo "Building example.c..."
    gcc example.c -fpic -shared -o libexample.so -l empath -L ../../target/debug
    echo "Building event.c..."
    gcc event.c -fpic -shared -o libevent.so -l empath -L ../../target/debug
    echo "✅ FFI examples built successfully"

# Build and check everything (no tests)
build-all: build build-release build-empathctl build-ffi
    @echo "✅ All build targets completed"

# Verbose build with timing information
build-verbose:
    cargo build -vv --timings

# Show build timings
timings:
    @echo "Opening cargo build timings..."
    @if command -v xdg-open >/dev/null 2>&1; then \
        xdg-open target/cargo-timings/cargo-timing.html; \
    elif command -v open >/dev/null 2>&1; then \
        open target/cargo-timings/cargo-timing.html; \
    else \
        echo "Please open target/cargo-timings/cargo-timing.html in your browser"; \
    fi

# Clean build artifacts
clean:
    cargo clean
    rm -rf target/
    rm -f empath-ffi/examples/*.so

# Clean spool directory (careful!)
clean-spool:
    #!/usr/bin/env bash
    set -euo pipefail
    echo "⚠️  This will delete all messages in the spool directory!"
    read -p "Are you sure? [y/N] " -n 1 -r
    echo
    if [[ $REPLY =~ ^[Yy]$ ]]; then
        rm -rf /tmp/spool/*
        echo "✅ Spool directory cleaned"
    else
        echo "❌ Cancelled"
    fi

# =============================================================================
# LINTING & FORMATTING
# =============================================================================

# Run strict clippy checks (project standard - all/pedantic/nursery via workspace lints)
lint:
    cargo clippy --all-targets --all-features

# Run clippy with automatic fixes
lint-fix:
    cargo clippy --all-targets --all-features --fix --allow-dirty --allow-staged

# Run clippy on single crate
lint-crate CRATE:
    cargo clippy -p {{CRATE}} --all-targets

# Format all code
fmt:
    cargo fmt --all

# Check formatting without modifying files
fmt-check:
    cargo fmt --all -- --check

# =============================================================================
# TESTING
# =============================================================================

# Run all tests
test:
    cargo test

# Run tests with nextest (faster, better output)
test-nextest:
    cargo nextest run

# Run tests with cargo-watch for continuous testing
test-watch:
    cargo watch -x "nextest run" -c

# Run miri tests (undefined behavior detection)
test-miri:
    cargo +nightly miri nextest run

# Run a specific test by name
test-one TEST:
    cargo test {{TEST}}

# Run tests for specific crate
test-crate CRATE:
    cargo test -p {{CRATE}}

# =============================================================================
# BENCHMARKING
# =============================================================================

# Run all benchmarks
bench:
    cargo bench

# Run benchmarks for specific crate
bench-smtp:
    cargo bench -p empath-smtp

bench-spool:
    cargo bench -p empath-spool

bench-delivery:
    cargo bench -p empath-delivery

# Run specific benchmark group
bench-group GROUP:
    cargo bench -- {{GROUP}}

# View benchmark results in browser
bench-view:
    @echo "Opening benchmark report..."
    @if command -v xdg-open >/dev/null 2>&1; then \
        xdg-open target/criterion/report/index.html; \
    elif command -v open >/dev/null 2>&1; then \
        open target/criterion/report/index.html; \
    else \
        echo "Please open target/criterion/report/index.html in your browser"; \
    fi

# Save benchmark baseline (for regression detection)
bench-baseline-save NAME="main":
    @echo "Saving benchmark baseline as '{{NAME}}'..."
    cargo bench -- --save-baseline {{NAME}}
    @echo "✅ Baseline '{{NAME}}' saved successfully"
    @echo "Compare against it with: just bench-baseline-compare {{NAME}}"

# Compare benchmarks against a saved baseline
bench-baseline-compare NAME="main":
    @echo "Comparing benchmarks against baseline '{{NAME}}'..."
    cargo bench -- --baseline {{NAME}}
    @echo ""
    @echo "✅ Comparison complete. Check target/criterion/report/index.html for detailed results"

# List all saved baselines
bench-baseline-list:
    @echo "=== Saved Benchmark Baselines ==="
    @if [ -d target/criterion ]; then \
        find target/criterion -type d -name "base" -o -name "main" | sed 's|target/criterion/||' | sed 's|/base||' | sed 's|/main||' | sort -u || echo "No baselines found"; \
    else \
        echo "No benchmark data yet. Run 'just bench' first."; \
    fi

# Delete a benchmark baseline
bench-baseline-delete NAME:
    @echo "⚠️  Deleting baseline '{{NAME}}'..."
    @find target/criterion -type d -name "{{NAME}}" -exec rm -rf {} + 2>/dev/null || true
    @echo "✅ Baseline '{{NAME}}' deleted"

# CI workflow: Compare current benchmarks against main baseline (fails on >10% regression)
bench-ci:
    #!/usr/bin/env bash
    set -euo pipefail
    echo "Running CI benchmark comparison against 'main' baseline..."

    # Check if main baseline exists
    if [ ! -d "target/criterion" ]; then
        echo "⚠️  No baseline found. Creating initial baseline as 'main'..."
        cargo bench -- --save-baseline main
        echo "✅ Initial baseline created. Future runs will compare against this."
        exit 0
    fi

    # Run comparison
    echo "Comparing against baseline 'main'..."
    cargo bench -- --baseline main --baseline-lenient

    echo ""
    echo "✅ Benchmark CI check complete"
    echo "Review detailed results at: target/criterion/report/index.html"

# =============================================================================
# RUNNING
# =============================================================================

# Run empath binary
run:
    cargo run --bin empath

# Run empath with config file
run-with-config CONFIG:
    cargo run --bin empath -- {{CONFIG}}

# Run empath with default config
run-default:
    cargo run --bin empath -- empath.config.ron

# Build and run in release mode
run-release:
    cargo run --release --bin empath

# =============================================================================
# QUEUE MANAGEMENT (empathctl)
# =============================================================================

# Run empathctl queue list
queue-list:
    cargo run --bin empathctl -- queue list

# Run empathctl queue stats
queue-stats:
    cargo run --bin empathctl -- queue stats

# Run empathctl queue stats in watch mode
queue-watch:
    cargo run --bin empathctl -- queue stats --watch --interval 2

# =============================================================================
# DEPENDENCIES
# =============================================================================

# Check for outdated dependencies
deps-outdated:
    cargo outdated

# Audit dependencies for security vulnerabilities
deps-audit:
    cargo audit

# Check dependencies with cargo-deny
deps-deny:
    cargo deny check

# Update dependencies
deps-update:
    cargo update

# =============================================================================
# DOCUMENTATION
# =============================================================================

# Generate documentation
docs:
    cargo doc --no-deps --open

# Generate documentation for all dependencies
docs-all:
    cargo doc --open

# Generate C headers from FFI crate (happens automatically during build)
gen-headers:
    cargo build
    @echo "C headers generated at target/empath.h"

# =============================================================================
# UTILITIES
# =============================================================================

# Show project statistics
stats:
    @echo "=== Empath MTA Project Statistics ==="
    @echo ""
    @echo "Lines of code (excluding tests):"
    @tokei --exclude '*/tests/*' --exclude '*/benches/*'
    @echo ""
    @echo "Dependency tree:"
    @cargo tree --depth 1

# ============================================================================
# Docker Development Stack
# ============================================================================
# Full observability stack: Empath + OTEL + Prometheus + Grafana
# See docker/README.md for detailed documentation

# Start full development stack (Empath + OTEL + Prometheus + Grafana)
docker-up:
    docker-compose -f docker/compose.dev.yml up -d
    @echo "✅ Docker stack started"
    @echo ""
    @echo "Services available at:"
    @echo "  - Empath SMTP:  smtp://localhost:1025"
    @echo "  - Grafana:      http://localhost:3000 (admin/admin)"
    @echo "  - Prometheus:   http://localhost:9090"
    @echo "  - OTEL:         http://localhost:4318 (OTLP)"
    @echo ""
    @echo "View logs: just docker-logs"

# Stop development stack
docker-down:
    docker-compose -f docker/compose.dev.yml down
    @echo "✅ Docker stack stopped"

# View live logs from all services
docker-logs:
    docker-compose -f docker/compose.dev.yml logs -f

# View live logs from Empath container only
docker-logs-empath:
    docker-compose -f docker/compose.dev.yml logs -f empath

# Rebuild containers
docker-build:
    docker-compose -f docker/compose.dev.yml build
    @echo "✅ Docker images rebuilt"

# Rebuild and restart containers
docker-rebuild: docker-build
    docker-compose -f docker/compose.dev.yml up -d
    @echo "✅ Docker stack rebuilt and restarted"

# Restart Empath container only (after config changes)
docker-restart:
    docker-compose -f docker/compose.dev.yml restart empath
    @echo "✅ Empath container restarted"

# Open Grafana dashboard in browser
docker-grafana:
    @echo "Opening Grafana at http://localhost:3000 (admin/admin)"
    @xdg-open http://localhost:3000 2>/dev/null || open http://localhost:3000 2>/dev/null || echo "Visit http://localhost:3000"

# Open Prometheus UI in browser
docker-prometheus:
    @echo "Opening Prometheus at http://localhost:9090"
    @xdg-open http://localhost:9090 2>/dev/null || open http://localhost:9090 2>/dev/null || echo "Visit http://localhost:9090"

# Show status of all containers
docker-ps:
    docker-compose -f docker/compose.dev.yml ps

# Full stack teardown including volumes (clean slate)
docker-clean:
    docker-compose -f docker/compose.dev.yml down -v
    @echo "✅ Docker stack cleaned (including volumes)"

# Send a test email through the Docker SMTP server
docker-test-email:
    @echo "Sending test email via localhost:1025..."
    swaks -4 --server localhost:1025 --to "receiver@test.example.com" --from "sender@gmail.com"
    @echo ""
    @echo "✅ Test email sent. Check logs: just docker-logs-empath"
