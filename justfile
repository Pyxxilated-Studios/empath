# Empath MTA - Task Runner
#
# A collection of common development tasks for the Empath project.
# Run `just` or `just --list` to see all available commands.
#
# Prerequisites:
# - just: cargo install just
# - cargo-nextest: cargo install cargo-nextest (optional but recommended)
# - mold: System package (apt install mold / brew install mold)

# List all available commands
default:
    @just --list

# Run strict clippy checks (project standard - all/pedantic/nursery via workspace lints)
lint:
    cargo clippy --all-targets --all-features

# Run clippy with automatic fixes
lint-fix:
    cargo clippy --all-targets --all-features --fix --allow-dirty --allow-staged

# Format all code
fmt:
    cargo fmt --all

# Check formatting without modifying files
fmt-check:
    cargo fmt --all -- --check

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

# Build entire workspace
build:
    cargo build

# Build release version (uses thin LTO, opt-level 2)
build-release:
    cargo build --release

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

# Build empathctl queue management CLI
build-empathctl:
    cargo build --bin empathctl

# Run empathctl queue list
queue-list:
    cargo run --bin empathctl -- queue list

# Run empathctl queue stats
queue-stats:
    cargo run --bin empathctl -- queue stats

# Run empathctl queue stats in watch mode
queue-watch:
    cargo run --bin empathctl -- queue stats --watch --interval 2

# Run empath binary
run:
    cargo run --bin empath

# Run empath with config file
run-with-config CONFIG:
    cargo run --bin empath -- {{CONFIG}}

# Run empath with default config
run-default:
    cargo run --bin empath -- empath.config.ron

# Check project (fast compile check without building)
check:
    cargo check --all-targets

# Full CI check locally (lint + test)
ci: lint fmt-check test
    @echo "✅ All CI checks passed!"

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

# Generate C headers from FFI crate (happens automatically during build)
gen-headers:
    cargo build
    @echo "C headers generated at target/empath.h"

# Run a specific test by name
test-one TEST:
    cargo test {{TEST}}

# Run tests for specific crate
test-crate CRATE:
    cargo test -p {{CRATE}}

# Build and run in release mode
run-release:
    cargo run --release --bin empath

# Show project statistics
stats:
    @echo "=== Empath MTA Project Statistics ==="
    @echo ""
    @echo "Lines of code (excluding tests):"
    @tokei --exclude '*/tests/*' --exclude '*/benches/*'
    @echo ""
    @echo "Dependency tree:"
    @cargo tree --depth 1

# Generate documentation
docs:
    cargo doc --no-deps --open

# Generate documentation for all dependencies
docs-all:
    cargo doc --open

# Quick fix common issues
fix: lint-fix fmt
    @echo "✅ Auto-fixes applied (lint + format)"

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

# Run clippy on single crate
lint-crate CRATE:
    cargo clippy -p {{CRATE}} --all-targets

# Build and check everything (no tests)
build-all: build build-release build-empathctl build-ffi
    @echo "✅ All build targets completed"

# Development workflow: format, lint, test
dev: fmt lint test
    @echo "✅ Development checks passed"

# Pre-commit checks (fast)
pre-commit: fmt-check lint
    @echo "✅ Pre-commit checks passed"
