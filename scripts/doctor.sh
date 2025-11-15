#!/usr/bin/env bash
# Empath MTA - Development Environment Health Check
# Diagnoses common setup issues before they become blockers

set -euo pipefail

# Color codes for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Counters for summary
ERRORS=0
WARNINGS=0
SUCCESS=0

# Helper functions
check_success() {
    echo -e "${GREEN}✅${NC} $1"
    SUCCESS=$((SUCCESS + 1))
}

check_warning() {
    echo -e "${YELLOW}⚠️${NC}  $1"
    WARNINGS=$((WARNINGS + 1))
}

check_error() {
    echo -e "${RED}❌${NC} $1"
    ERRORS=$((ERRORS + 1))
}

check_info() {
    echo -e "${BLUE}ℹ️${NC}  $1"
}

echo -e "${BLUE}=== Empath MTA - Environment Doctor ===${NC}"
echo ""

# =============================================================================
# Rust Toolchain Checks
# =============================================================================
echo -e "${BLUE}🦀 Rust Toolchain${NC}"

# Check rustc
if command -v rustc >/dev/null 2>&1; then
    RUSTC_VERSION=$(rustc --version)
    check_success "rustc: $RUSTC_VERSION"

    # Check if nightly
    if echo "$RUSTC_VERSION" | grep -q "nightly"; then
        check_success "Nightly toolchain detected"
    else
        check_warning "Not using nightly toolchain (required for this project)"
        check_info "Run: rustup default nightly"
    fi

    # Check version (should be 1.93.0-nightly or later)
    VERSION=$(echo "$RUSTC_VERSION" | grep -oP '\d+\.\d+\.\d+' | head -1)
    if [[ $(echo -e "1.93.0\n$VERSION" | sort -V | head -1) == "1.93.0" ]]; then
        check_success "Version meets minimum requirement (>= 1.93.0)"
    else
        check_error "Version $VERSION is below minimum requirement (1.93.0)"
        check_info "Run: rustup update nightly"
    fi
else
    check_error "rustc not found"
    check_info "Install Rust: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
fi

# Check cargo
if command -v cargo >/dev/null 2>&1; then
    CARGO_VERSION=$(cargo --version)
    check_success "cargo: $CARGO_VERSION"
else
    check_error "cargo not found (should be installed with rustc)"
fi

echo ""

# =============================================================================
# Build Tools
# =============================================================================
echo -e "${BLUE}🔨 Build Tools${NC}"

# Check just
if command -v just >/dev/null 2>&1; then
    JUST_VERSION=$(just --version)
    check_success "just: $JUST_VERSION"
else
    check_error "just not found (required for task runner)"
    check_info "Install: cargo install just"
fi

# Check mold (optional but recommended)
if command -v mold >/dev/null 2>&1; then
    MOLD_VERSION=$(mold --version | head -1)
    check_success "mold: $MOLD_VERSION (40-60% faster builds)"
else
    check_warning "mold not found (optional, speeds up builds)"
    check_info "Install on Linux: sudo apt-get install mold"
    check_info "Install on macOS: Use lld or zld (mold dropped Mach-O support)"
fi

echo ""

# =============================================================================
# Cargo Development Tools
# =============================================================================
echo -e "${BLUE}📦 Cargo Tools${NC}"

# Check cargo-nextest
if command -v cargo-nextest >/dev/null 2>&1; then
    NEXTEST_VERSION=$(cargo nextest --version)
    check_success "cargo-nextest: $NEXTEST_VERSION"
else
    check_error "cargo-nextest not found (required for fast testing)"
    check_info "Install: cargo install cargo-nextest"
fi

# Check cargo-deny
if cargo deny --version >/dev/null 2>&1; then
    DENY_VERSION=$(cargo deny --version)
    check_success "cargo-deny: $DENY_VERSION"
else
    check_warning "cargo-deny not found (recommended for CI)"
    check_info "Install: cargo install cargo-deny"
fi

# Check cargo-audit
if cargo audit --version >/dev/null 2>&1; then
    AUDIT_VERSION=$(cargo audit --version)
    check_success "cargo-audit: $AUDIT_VERSION"
else
    check_warning "cargo-audit not found (recommended for security)"
    check_info "Install: cargo install cargo-audit"
fi

# Check cargo-watch (optional)
if cargo watch --version >/dev/null 2>&1; then
    WATCH_VERSION=$(cargo watch --version)
    check_success "cargo-watch: $WATCH_VERSION"
else
    check_warning "cargo-watch not found (optional, for continuous testing)"
    check_info "Install: cargo install cargo-watch"
fi

echo ""

# =============================================================================
# Docker
# =============================================================================
echo -e "${BLUE}🐳 Docker${NC}"

if command -v docker >/dev/null 2>&1; then
    DOCKER_VERSION=$(docker --version)
    check_success "docker: $DOCKER_VERSION"

    # Check if docker daemon is running
    if docker info >/dev/null 2>&1; then
        check_success "Docker daemon is running"
    else
        check_warning "Docker daemon is not running"
        check_info "Start daemon: sudo systemctl start docker"
    fi
else
    check_warning "docker not found (optional, for Docker development stack)"
    check_info "Install: https://docs.docker.com/get-docker/"
fi

# Check docker-compose
if command -v docker-compose >/dev/null 2>&1; then
    COMPOSE_VERSION=$(docker-compose --version)
    check_success "docker-compose: $COMPOSE_VERSION"
else
    check_warning "docker-compose not found (optional, for Docker stack)"
    check_info "Install: sudo apt-get install docker-compose"
fi

echo ""

# =============================================================================
# Git Hooks
# =============================================================================
echo -e "${BLUE}🪝 Git Hooks${NC}"

if [ -f .git/hooks/pre-commit ]; then
    check_success "pre-commit hook installed"
else
    check_warning "pre-commit hook not installed"
    check_info "Install: ./scripts/install-hooks.sh"
fi

echo ""

# =============================================================================
# Project Build Check
# =============================================================================
echo -e "${BLUE}🏗️  Project Build${NC}"

check_info "Running cargo check (this may take a moment)..."
if cargo check --quiet 2>/dev/null; then
    check_success "Project builds successfully"
else
    check_error "Project build failed"
    check_info "Run 'cargo check' to see detailed errors"
fi

echo ""

# =============================================================================
# Test Compilation Check
# =============================================================================
echo -e "${BLUE}🧪 Test Compilation${NC}"

check_info "Checking test compilation..."
if cargo test --no-run --quiet 2>/dev/null; then
    check_success "Tests compile successfully"
else
    check_error "Test compilation failed"
    check_info "Run 'cargo test --no-run' to see detailed errors"
fi

echo ""

# =============================================================================
# Environment Variables
# =============================================================================
echo -e "${BLUE}🌍 Environment Variables${NC}"

if [ -n "${RUST_BACKTRACE:-}" ]; then
    check_success "RUST_BACKTRACE=$RUST_BACKTRACE"
else
    check_info "RUST_BACKTRACE not set (optional, helps with debugging)"
    check_info "Add to shell: export RUST_BACKTRACE=1"
fi

if [ -n "${RUST_LOG:-}" ]; then
    check_success "RUST_LOG=$RUST_LOG"
else
    check_info "RUST_LOG not set (optional, controls log verbosity)"
    check_info "Add to shell: export RUST_LOG=debug"
fi

echo ""

# =============================================================================
# Summary
# =============================================================================
echo -e "${BLUE}=== Summary ===${NC}"
echo ""

if [ $ERRORS -eq 0 ] && [ $WARNINGS -eq 0 ]; then
    echo -e "${GREEN}✅ Environment is perfect! You're ready to develop.${NC}"
    exit 0
elif [ $ERRORS -eq 0 ]; then
    echo -e "${YELLOW}⚠️  Environment is functional with $WARNINGS warning(s).${NC}"
    echo "   Consider addressing warnings for optimal experience."
    exit 0
else
    echo -e "${RED}❌ Environment has $ERRORS error(s) and $WARNINGS warning(s).${NC}"
    echo "   Please address errors before developing."
    exit 1
fi
