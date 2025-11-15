# New Developer Onboarding

Welcome to Empath MTA! This guide will help you get from zero to productive contributor in under 30 minutes.

## Quick Overview

**Empath** is a Mail Transfer Agent (MTA) written in Rust with a focus on:
- Easy debugging and testing
- Extensible plugin system via FFI
- Production-ready observability (OpenTelemetry, Prometheus, Grafana)
- Modern async Rust architecture

**Tech Stack:**
- Rust nightly (Edition 2024)
- Tokio async runtime
- Criterion for benchmarking
- Docker for local development
- OpenTelemetry for observability

---

## Setup Checklist (15 minutes)

Work through these steps in order. Each should take 1-3 minutes.

### 1. Prerequisites

- [ ] **Install Rust nightly** (required for Edition 2024 features)
  ```bash
  rustup toolchain install nightly
  rustup default nightly
  rustc --version  # Should show 1.93.0-nightly or later
  ```

- [ ] **Install just** (task runner, makes development much easier)
  ```bash
  cargo install just
  just --version
  ```

- [ ] **Clone repository**
  ```bash
  git clone https://github.com/Pyxxilated-Studios/empath.git
  cd empath
  ```

### 2. Development Tools Setup

- [ ] **Run automated setup** (installs all dev tools)
  ```bash
  just setup
  ```
  This installs: cargo-nextest, cargo-watch, cargo-outdated, cargo-audit, cargo-deny, mold linker, and git hooks.

- [ ] **Verify environment health**
  ```bash
  just doctor
  ```
  This checks your Rust version, build tools, and project configuration.

### 3. Build and Test

- [ ] **Build the project** (first build takes ~2 minutes)
  ```bash
  just build
  ```

- [ ] **Run all tests** (should all pass, ~10 seconds with nextest)
  ```bash
  just test
  ```

- [ ] **Run linter** (ensures code meets project standards)
  ```bash
  just lint
  ```

### 4. Try It Out

- [ ] **Start the MTA** (runs SMTP server on localhost:1025)
  ```bash
  just run
  ```
  Press Ctrl+C to stop.

- [ ] **Start full Docker stack** (Empath + Grafana + Prometheus + OTEL)
  ```bash
  just docker-up
  ```
  Services available:
  - SMTP: `localhost:1025`
  - Grafana: `http://localhost:3000` (admin/admin)
  - Prometheus: `http://localhost:9090`

- [ ] **Send a test email**
  ```bash
  just docker-test-email
  ```

- [ ] **View queue messages**
  ```bash
  just queue-list
  just queue-stats
  ```

- [ ] **Stop Docker stack**
  ```bash
  just docker-down
  ```

### 5. Editor Setup (Optional but Recommended)

- [ ] **VS Code users**: Extensions will auto-prompt on first open
  - rust-analyzer (essential)
  - even-better-toml
  - crates (dependency management)

  Settings are pre-configured in `.vscode/settings.json`

- [ ] **Other editors**: Use EditorConfig (`.editorconfig` is already configured)
  - 4 spaces for Rust
  - 2 spaces for TOML/YAML
  - LF line endings

---

## Understanding the Codebase (30 minutes)

### Architecture Overview (10 minutes)

**📊 Visual Guide:** See [docs/ARCHITECTURE.md](ARCHITECTURE.md) for comprehensive diagrams including component architecture, data flow, and state machines.

**7-Crate Workspace:**

1. **empath** - Main binary, orchestrates all components
2. **empath-common** - Core traits (Protocol, FSM, Controller, Listener)
3. **empath-smtp** - SMTP protocol implementation
4. **empath-delivery** - Outbound mail delivery queue
5. **empath-spool** - Message persistence to filesystem
6. **empath-ffi** - C API for plugins/modules
7. **empath-tracing** - Procedural macros for instrumentation

**Data Flow:**
```
Client → Listener → Session (SMTP FSM) → Module Validation → Spool → Delivery Queue → External SMTP
```

**Key Patterns:**
- **Generic Protocol System**: `Protocol` trait allows new protocols (IMAP, POP3, etc.)
- **Finite State Machine**: SMTP session states with typed transitions
- **Module/Plugin System**: C FFI for extending functionality without core changes

### Essential Reading (15 minutes)

Read these in order:

1. **README.md** (5 min) - Project overview, quick start, features
2. **CLAUDE.md** (10 min) - Architecture deep dive, skim initially
   - Focus on: "Architecture Overview" and "Code Organization Patterns"
   - Bookmark for later reference on clippy requirements

### Hands-On Exploration (5 minutes)

- [ ] **Explore workspace structure**
  ```bash
  just stats  # See lines of code and dependency tree
  ```

- [ ] **View benchmark results**
  ```bash
  just bench-smtp  # Run SMTP benchmarks
  just bench-view  # Open HTML report
  ```

- [ ] **Try queue management**
  ```bash
  just queue-list
  just queue-stats --watch  # Live stats (Ctrl+C to exit)
  ```

---

## Your First Contribution

Now that you're set up, here are good first tasks to get familiar with the codebase:

### Starter Tasks (Pick One)

**Option 1: Add a Simple Test** (30-60 min, Good for learning)
- Pick any function without tests
- Write unit test
- Run with `just test-one your_test_name`
- Good practice for Rust testing and understanding code

**Option 2: Simple TODO Task** (1-2 hours, Real contribution)

Pick a "Simple" complexity task from TODO.md:
- ✅ 7.12: Add CONTRIBUTING.md (1-2 hours) - Document contribution process
- 🟢 7.23: Add Architecture Diagram (2-3 hours) - Create Mermaid diagram
- 🟢 7.10: Add Examples Directory (1-2 days) - Example configs and usage

**Option 3: Documentation Improvement** (15-30 min, Easy win)
- Find unclear documentation
- Improve explanation or add examples
- Good for understanding the system

### Contribution Workflow

1. **Create feature branch**
   ```bash
   git checkout -b feature/my-improvement
   ```

2. **Make changes**
   - Code follows clippy pedantic/nursery lints (strict!)
   - `just dev` runs fmt + lint + test
   - Git pre-commit hook ensures quality

3. **Test thoroughly**
   ```bash
   just ci  # Run full CI checks locally
   ```

4. **Commit with clear message**
   ```bash
   git add .
   git commit -m "feat: Add feature description"
   ```
   Follow conventional commits: `feat:`, `fix:`, `docs:`, `refactor:`, `test:`

5. **Push and create PR**
   ```bash
   git push -u origin feature/my-improvement
   ```

---

## Common Commands Reference

**Daily Development:**
```bash
just dev              # Format + lint + test (main workflow)
just ci               # Full CI check before pushing
just build            # Build entire workspace
just test             # Run all tests
just test-watch       # Continuous testing (watches for changes)
```

**Code Quality:**
```bash
just lint             # Run clippy (strict checks)
just fmt              # Format code
just fix              # Auto-fix lint issues + format
```

**Running:**
```bash
just run              # Start Empath SMTP server
just docker-up        # Start full observability stack
just queue-list       # List queued messages
just queue-watch      # Live queue statistics
```

**Benchmarking:**
```bash
just bench            # Run all benchmarks
just bench-smtp       # SMTP benchmarks only
just bench-baseline-save main    # Save baseline for comparison
just bench-baseline-compare main # Check for regressions
```

**Get Help:**
```bash
just help             # Show common commands
just --list           # Show all 50+ commands
just doctor           # Diagnose environment issues
```

---

## Getting Help

**Resources:**
- **CLAUDE.md** - Comprehensive architecture and coding standards
- **TODO.md** - Active development roadmap
- **README.md** - Project overview and quick start
- **TROUBLESHOOTING.md** (coming soon) - Common issues and solutions

**Questions?**
- Check existing documentation first (CLAUDE.md is very comprehensive)
- Run `just doctor` to diagnose setup issues
- Review similar code in the codebase
- Create an issue on GitHub with your question

---

## What's Next?

After completing your first contribution:

1. **Review Advanced Topics in CLAUDE.md:**
   - Module/Plugin System
   - FFI Integration
   - Benchmarking and Performance
   - Security Considerations

2. **Explore High-Impact Areas:**
   - Testing (we need more E2E tests!)
   - Documentation (always room for improvement)
   - Performance (benchmark and optimize hot paths)

3. **Pick a Bigger Task from TODO.md:**
   - Review "Next Sprint Priorities" section
   - Critical tasks have highest impact
   - Match complexity to your experience level

---

## Success Checklist

You're ready to contribute when you can:

- [ ] Build the project without errors
- [ ] Run tests and see them all pass
- [ ] Start the SMTP server and send a test email
- [ ] Run clippy without warnings
- [ ] Understand the 7-crate workspace structure
- [ ] Know where to find documentation (CLAUDE.md)
- [ ] Use `just` commands for common tasks

**Welcome to the team! 🚀**
