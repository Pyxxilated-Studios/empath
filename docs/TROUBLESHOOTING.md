# Troubleshooting Guide

Common issues and solutions for Empath MTA development. This guide helps you debug problems quickly without needing maintainer support.

**Quick Diagnostics:**
```bash
just doctor  # Run environment health check
```

---

## Table of Contents

- [Build Issues](#build-issues)
- [Test Failures](#test-failures)
- [Runtime Issues](#runtime-issues)
- [Docker Issues](#docker-issues)
- [Clippy Errors](#clippy-errors)
- [Development Workflow Issues](#development-workflow-issues)
- [Performance Issues](#performance-issues)
- [Getting More Help](#getting-more-help)

---

## Build Issues

### "cannot find -lempath" or "ld: library not found"

**Problem:** Trying to build FFI examples before building the main library.

**Solution:**
```bash
# Build empath library first
just build

# Then build FFI examples
just build-ffi
```

**Why:** FFI examples link against `libempath.so` which must exist first.

---

### Slow Build Times (>2 minutes for incremental builds)

**Problem:** Builds are taking too long, slowing down development.

**Solution 1 - Enable mold linker (Linux only):**
```bash
# Install mold
sudo apt-get install mold  # Ubuntu/Debian
brew install mold          # macOS (note: limited support)

# Already configured in .cargo/config.toml for Linux
just build  # Should be 40-60% faster now
```

**Solution 2 - Use sccache for distributed caching:**
```bash
cargo install sccache
export RUSTC_WRAPPER=sccache
just build
```

**Solution 3 - Check disk space:**
```bash
df -h .
# If low on space, clean old builds:
just clean
```

**Expected Times:**
- First build: ~2-3 minutes
- Incremental builds: ~10-30 seconds
- With mold linker: ~5-15 seconds

---

### "error: rustc version too old" or nightly feature errors

**Problem:** Using stable Rust instead of nightly, or outdated nightly version.

**Solution:**
```bash
# Install/update Rust nightly
rustup toolchain install nightly
rustup default nightly

# Verify version (needs 1.93.0-nightly or later)
rustc --version

# If still old, update rustup itself:
rustup update
```

**Why:** Empath uses Edition 2024 features only available in recent nightly builds.

---

### "overflow evaluating the requirement" or trait solver issues

**Problem:** Compiler trait solver hitting recursion limits.

**Solution:**
```bash
# Clean build artifacts
just clean

# Rebuild from scratch
just build
```

**If persists:**
```bash
# Check for circular dependencies
cargo tree --duplicates
```

---

### Permission denied when writing to target/

**Problem:** Build directory has wrong permissions or is owned by root.

**Solution:**
```bash
# Fix ownership
sudo chown -R $USER:$USER target/

# Or delete and rebuild
rm -rf target/
just build
```

---

## Test Failures

### "Address already in use (os error 98)"

**Problem:** Previous test run didn't clean up, or empath is running in background.

**Solution:**
```bash
# Kill any running empath processes
pkill empath

# Or kill process on specific port
sudo lsof -ti:1025 | xargs kill -9

# If using Docker:
just docker-down

# Then retry tests
just test
```

---

### Flaky async tests (intermittent failures)

**Problem:** Async tests timing out or racing.

**Common causes:**
1. System under heavy load
2. Slow disk I/O
3. Test timeouts too short

**Solution 1 - Increase test timeout:**
```bash
# Run tests with longer timeout
RUST_TEST_THREADS=1 cargo test -- --test-threads=1 --nocapture

# Or use nextest with custom timeout
cargo nextest run --test-threads=1 --failure-output immediate
```

**Solution 2 - Run tests serially:**
```bash
just test-one your_test_name
```

**Solution 3 - Check system resources:**
```bash
# CPU usage
top

# Disk I/O
iostat -x 1

# If system is overloaded, close other applications
```

---

### Tests pass individually but fail when run together

**Problem:** Tests are sharing state (ports, files, global variables).

**Solution:**
```bash
# Run with single thread to isolate
RUST_TEST_THREADS=1 cargo test

# Or use nextest which isolates by default
just test-nextest
```

**For contributors:** Fix the test to use unique resources:
- Random ports instead of hardcoded ports
- Unique temp directories per test
- Reset global state in test cleanup

---

### Spool-related test failures

**Problem:** Old test messages in spool directory causing conflicts.

**Solution:**
```bash
# Clean test spool directory
rm -rf /tmp/spool/*

# Run specific spool test
just test-crate empath-spool

# If problem persists, clean everything
just clean-spool
just test
```

---

### "test result: FAILED. 0 passed; 5 failed" after git pull

**Problem:** Changes in dependencies or API changes.

**Solution:**
```bash
# Update dependencies
cargo update

# Clean and rebuild
just clean
just build
just test
```

---

## Runtime Issues

### "Permission denied" on control socket

**Problem:** Control socket file has wrong permissions.

**Solution:**
```bash
# Check socket permissions
ls -la /tmp/empath.sock

# Fix permissions
chmod 600 /tmp/empath.sock

# Or delete and restart empath (socket will be recreated)
rm /tmp/empath.sock
just run
```

---

### Messages stuck in queue (not being delivered)

**Problem:** Delivery processor may be stalled or domain is unreachable.

**Debugging steps:**

1. **Check queue status:**
```bash
just queue-list
just queue-stats
```

2. **Check logs for errors:**
```bash
RUST_LOG=empath=debug just run
```

3. **Check DNS resolution:**
```bash
# Test MX lookup manually
dig MX example.com

# Check for DNS cache issues
just run  # Then use empathctl:
./target/debug/empathctl dns list-cache
./target/debug/empathctl dns clear-cache
```

4. **Check domain configuration:**
```bash
# Review empath.config.ron delivery section
cat empath.config.ron | grep -A 20 "delivery:"
```

5. **Manually retry failed message:**
```bash
# Get message ID from queue-list
just queue-list

# Retry specific message
./target/debug/empathctl queue retry <message-id>
```

---

### "Too many open files" error

**Problem:** System file descriptor limit too low.

**Solution:**
```bash
# Check current limit
ulimit -n

# Increase temporarily (until reboot)
ulimit -n 4096

# Increase permanently (Linux):
echo "* soft nofile 4096" | sudo tee -a /etc/security/limits.conf
echo "* hard nofile 4096" | sudo tee -a /etc/security/limits.conf

# Log out and back in for changes to take effect
```

---

### High memory usage or memory leak

**Problem:** Memory usage growing over time.

**Debugging:**
```bash
# Run with memory profiling
cargo build --release
valgrind --leak-check=full ./target/release/empath

# Or use Rust's heap profiler
cargo install dhat
# Add to Cargo.toml: dhat = "0.3"
# Instrument code and analyze
```

**Common causes:**
- Messages accumulating in delivery queue
- DNS cache growing unbounded
- Module memory leaks (check FFI modules)

**Mitigation:**
```bash
# Clear DNS cache periodically
./target/debug/empathctl dns clear-cache

# Monitor queue size
just queue-stats --watch
```

---

### SMTP connections refused

**Problem:** Empath not listening on expected port or firewall blocking.

**Solution:**
```bash
# Check if empath is running
ps aux | grep empath

# Check which ports are listening
sudo lsof -i -P -n | grep LISTEN | grep empath

# Test connection manually
telnet localhost 1025
# Or:
nc localhost 1025

# Check firewall (Linux)
sudo iptables -L | grep 1025

# Check firewall (macOS)
sudo pfctl -sr | grep 1025
```

---

## Docker Issues

### "Port 1025 already in use"

**Problem:** Another service (or old Docker container) using the port.

**Solution:**
```bash
# Stop any existing empath Docker containers
just docker-down

# Find process using port
sudo lsof -ti:1025

# Kill that process
sudo kill -9 $(sudo lsof -ti:1025)

# Restart Docker stack
just docker-up
```

---

### Grafana won't load / Shows "Bad Gateway"

**Problem:** Grafana container not fully started yet.

**Solution:**
```bash
# Wait 30 seconds after docker-up
sleep 30
just docker-grafana

# Check container logs
just docker-logs

# If still failing, rebuild containers
just docker-rebuild
```

**Common startup sequence:**
1. OTEL Collector (5 seconds)
2. Prometheus (10 seconds)
3. Grafana (20-30 seconds)
4. Empath (5 seconds)

---

### "Cannot connect to Docker daemon"

**Problem:** Docker service not running.

**Solution:**
```bash
# Linux
sudo systemctl start docker
sudo systemctl status docker

# macOS
open -a Docker

# Verify Docker is running
docker ps
```

---

### Docker container keeps restarting

**Problem:** Application crashing on startup.

**Solution:**
```bash
# View container logs
just docker-logs-empath

# Common issues:
# 1. Config file error - check docker/empath.config.ron
# 2. Port conflict - see "Port already in use" above
# 3. File permissions - check spool directory permissions in container

# Debug by running container interactively
docker run -it --rm empath-dev:latest /bin/sh
```

---

### Changes not reflected in Docker container

**Problem:** Using old Docker image with outdated code.

**Solution:**
```bash
# Rebuild and restart
just docker-rebuild

# Or manually:
docker-compose -f docker/compose.dev.yml build --no-cache
just docker-up
```

---

## Clippy Errors

Empath uses **strict clippy lints** (all + pedantic + nursery). Common errors and fixes:

### "function `foo` is too long (over 100 lines)"

**Error:** `clippy::too_many_lines`

**Solution:** Extract helper methods
```rust
// Before (150 lines)
pub fn handle_request(&self, req: Request) -> Response {
    // ... lots of code ...
}

// After (60 lines + helpers)
pub fn handle_request(&self, req: Request) -> Response {
    let validated = self.validate_request(&req)?;
    let processed = self.process_request(validated)?;
    self.build_response(processed)
}

fn validate_request(&self, req: &Request) -> Result<ValidatedRequest> {
    // Validation logic (30 lines)
}

fn process_request(&self, req: ValidatedRequest) -> Result<ProcessedRequest> {
    // Processing logic (40 lines)
}

fn build_response(&self, req: ProcessedRequest) -> Response {
    // Response building (20 lines)
}
```

**Reference:** See CLAUDE.md "Function Length Management"

---

### "this `if` has identical blocks"

**Error:** `clippy::collapsible_if`

**Solution:** Use let-chains
```rust
// Before (triggers clippy)
if let Some(spool) = &self.spool {
    if let Some(data) = &validate_context.data {
        // ...
    }
}

// After (correct)
if let Some(spool) = &self.spool
    && let Some(data) = &validate_context.data
{
    // ...
}
```

---

### "this looks like you are trying to use `.. Common Commands Reference

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
