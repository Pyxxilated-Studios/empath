# Empath MTA - Performance Profiling Guide

This guide covers profiling techniques for optimizing Empath MTA performance.

## Quick Start

```bash
# CPU profiling with flamegraph
just profile-cpu

# Profile specific benchmark
just profile-cpu smtp_benchmarks

# Benchmark with baseline comparison
just bench-baseline-save main
# ... make changes ...
just bench-baseline-compare main

# View benchmark results
just bench-view
```

---

## Justfile Commands Reference

Empath provides convenient `just` commands for all profiling and benchmarking workflows:

### Benchmarking Commands

| Command | Description |
|---------|-------------|
| `just bench` | Run all benchmarks across all crates |
| `just bench-smtp` | Run SMTP protocol benchmarks only |
| `just bench-spool` | Run spool operation benchmarks only |
| `just bench-delivery` | Run delivery processor benchmarks only |
| `just bench-group GROUP` | Run specific benchmark group (e.g., `fsm_transitions`) |
| `just bench-view` | Open HTML benchmark report in browser |

### Baseline Management

| Command | Description |
|---------|-------------|
| `just bench-baseline-save NAME` | Save current performance as baseline |
| `just bench-baseline-compare NAME` | Compare current run vs saved baseline |
| `just bench-baseline-list` | List all saved baselines |
| `just bench-baseline-delete NAME` | Delete a specific baseline |
| `just bench-ci` | CI workflow (creates baseline, compares, fails on regression) |

### Profiling Commands

| Command | Description |
|---------|-------------|
| `just profile-cpu` | Profile main binary with flamegraph (auto-opens SVG) |
| `just profile-cpu BENCH` | Profile specific benchmark (e.g., `smtp_benchmarks`) |
| `just changelog` | Generate CHANGELOG.md (useful after performance work) |

### Example Workflows

**Pre-optimization Baseline**:
```bash
git checkout main
just bench-baseline-save before-optimization
```

**Post-optimization Comparison**:
```bash
git checkout feat/my-optimization
just bench-baseline-compare before-optimization
```

**Profile Hot Path**:
```bash
# Find hot functions
just profile-cpu

# Profile specific benchmark
just profile-cpu smtp_benchmarks

# Compare performance
just bench-baseline-compare main
```

---

## CPU Profiling

### Flamegraph (Recommended)

**What it shows**: CPU time spent in each function, visualized as an interactive SVG

**Installation**:
```bash
cargo install flamegraph
```

**Linux Setup** (requires perf):
```bash
# Install perf
sudo apt-get install linux-tools-common linux-tools-generic  # Debian/Ubuntu
sudo yum install perf  # RHEL/CentOS

# Allow non-root perf access (optional)
echo 0 | sudo tee /proc/sys/kernel/perf_event_paranoid
```

**macOS Setup** (uses DTrace):
```bash
# No additional setup needed - DTrace is built-in
# Will prompt for sudo password when running
```

**Usage**:
```bash
# Profile the main binary (recommended - uses justfile)
just profile-cpu

# Profile specific benchmark
just profile-cpu smtp_benchmarks
just profile-cpu spool_benchmarks

# Or manually with cargo
sudo cargo flamegraph --bin empath -- empath.config.ron
sudo cargo flamegraph --release --bin empath -- empath.config.ron
sudo cargo flamegraph --bench smtp_benchmarks

# Result opens automatically, or manually open:
open flamegraph.svg  # macOS
xdg-open flamegraph.svg  # Linux
```

**Interpreting Results**:
- **Width**: Percentage of total CPU time
- **Color**: Random (just for visual distinction)
- **Y-axis**: Call stack depth (bottom = root, top = leaf functions)
- **Interactive**: Click to zoom, search for function names

**Example Analysis**:
```
If you see a wide bar for `tokio::runtime::spawn`:
→ Lots of time spawning tasks (potentially too much task overhead)

If you see a wide bar for `dns::lookup`:
→ DNS resolution is a bottleneck (consider caching)

If you see a wide bar for `spool::write`:
→ Disk I/O is slow (consider batching writes)
```

---

### perf (Linux Advanced)

**What it shows**: Low-level CPU profiling with hardware counter support

**Installation**:
```bash
sudo apt-get install linux-tools-common linux-tools-generic
```

**Usage**:
```bash
# Build with debug symbols
cargo build --release

# Record profile data
sudo perf record -g ./target/release/empath empath.config.ron
# ... let it run for 30-60 seconds, then Ctrl+C ...

# View interactive report
sudo perf report

# Generate flamegraph from perf data
sudo perf script | stackcollapse-perf.pl | flamegraph.pl > perf-flamegraph.svg
```

**Key Commands**:
```bash
# CPU cycles
sudo perf record -g -e cycles ./target/release/empath

# Cache misses
sudo perf record -g -e cache-misses ./target/release/empath

# Branch mispredictions
sudo perf record -g -e branch-misses ./target/release/empath
```

---

### Instruments (macOS)

**What it shows**: Apple's native profiling tool with CPU, memory, I/O, and network

**Usage**:
```bash
# Build with debug symbols
cargo build --release

# Launch Instruments
open -a Instruments

# Or command-line
instruments -t "Time Profiler" ./target/release/empath empath.config.ron
```

**Workflow**:
1. Select "Time Profiler" template
2. Choose `target/release/empath` as target
3. Add arguments: `empath.config.ron`
4. Click Record
5. Let run for 30-60 seconds
6. Stop and analyze call tree

---

## Memory Profiling

### dhat (Heap Profiling)

**What it shows**: Heap allocations, lifetimes, and peak memory usage

**Installation**:
```bash
# Add to Cargo.toml (dev-dependencies)
[dev-dependencies]
dhat = "0.3"
```

**Usage**:

Create `empath/src/bin/empath-dhat.rs`:
```rust
use dhat::{Dhat, DhatAlloc};

#[global_allocator]
static ALLOCATOR: DhatAlloc = DhatAlloc;

fn main() {
    let _dhat = Dhat::start_heap_profiling();
    empath::run();  // Your main function
}
```

```bash
# Run with dhat
cargo run --bin empath-dhat -- empath.config.ron

# Open result (dhat-heap.json generated on exit)
# Upload to https://nnethercote.github.io/dh_view/dh_view.html
```

**Interpreting Results**:
- **Total bytes**: How much memory allocated over lifetime
- **Max bytes**: Peak memory usage
- **At t-gmax**: Allocations alive at peak
- **Reads/Writes**: Memory access patterns

---

### Valgrind Massif (Linux)

**What it shows**: Heap and stack memory usage over time

**Installation**:
```bash
sudo apt-get install valgrind
```

**Usage**:
```bash
# Run with massif
valgrind --tool=massif --massif-out-file=massif.out ./target/release/empath empath.config.ron

# Visualize with massif-visualizer (GUI)
massif-visualizer massif.out

# Or text-based
ms_print massif.out
```

---

### Memory Allocator Statistics

**jemalloc** (Linux/macOS):

Add to `Cargo.toml`:
```toml
[dependencies]
tikv-jemallocator = "0.5"
tikv-jemalloc-ctl = "0.5"

[profile.release]
# Enable jemalloc profiling
debug = 1
```

Enable in `main.rs`:
```rust
use tikv_jemallocator::Jemalloc;

#[global_allocator]
static GLOBAL: Jemalloc = Jemalloc;

fn main() {
    // Your code
}
```

Profile:
```bash
# Run with profiling enabled
MALLOC_CONF=prof:true,prof_prefix:jeprof.out ./target/release/empath

# Analyze with jeprof
jeprof --pdf ./target/release/empath jeprof.out.*.heap > profile.pdf
```

---

## Benchmarking

### Criterion.rs (Current Approach)

**What it measures**: Microbenchmark performance with statistical rigor

**Running Benchmarks**:
```bash
# Run all benchmarks
just bench
# Or: cargo bench

# Specific crate
just bench-smtp
# Or: cargo bench -p empath-smtp

# Specific benchmark group
just bench-group fsm_transitions
# Or: cargo bench -- fsm_transitions

# View results
just bench-view
# Or: open target/criterion/report/index.html
```

**Baseline Workflow** (Regression Detection):
```bash
# 1. Save baseline on main branch
git checkout main
just bench-baseline-save main

# 2. Make changes on feature branch
git checkout feature/my-optimization

# 3. Compare against baseline
just bench-baseline-compare main

# 4. Check for regressions
# Criterion will show percentage changes:
#   "change: +2.5%" = 2.5% slower (REGRESSION)
#   "change: -5.0%" = 5% faster (IMPROVEMENT)
#   "No change" = within noise threshold
```

**Interpreting Results**:
```
Command parsing/basic_commands
                        time:   [145.23 ns 147.89 ns 150.91 ns]
                        change: [-2.3421% +0.5123% +3.4567%] (p = 0.52 > 0.05)
                        No change in performance detected.
```

- **time**: [lower_bound estimate upper_bound] (95% confidence interval)
- **change**: Performance change vs baseline (negative = faster)
- **p-value**: < 0.05 = statistically significant change

**Best Practices**:
- Run benchmarks multiple times to reduce noise
- Disable CPU frequency scaling: `sudo cpupower frequency-set --governor performance`
- Close background applications
- Use `--baseline` to compare against previous runs

**Managing Baselines**:
```bash
# List saved baselines
just bench-baseline-list

# Delete old baseline
just bench-baseline-delete old-feature

# CI workflow (fails on >10% regression)
just bench-ci
```

---

### Custom Load Testing

**SMTP Load Test** (using `swaks` and GNU `parallel`):

```bash
# Install tools
sudo apt-get install swaks parallel  # Debian/Ubuntu
brew install swaks parallel  # macOS

# Send 1000 emails in parallel (10 concurrent)
seq 1 1000 | parallel -j 10 \
  swaks --server localhost:1025 \
        --to "user{}@example.com" \
        --from "sender@test.com" \
        --body "Load test message {}"

# Monitor queue stats
just queue-watch
```

**Delivery Load Test**:

```bash
# 1. Populate queue with test messages
for i in {1..1000}; do
  swaks --server localhost:1025 \
        --to "user${i}@test.example.com" \
        --from "sender@loadtest.com"
done

# 2. Watch delivery throughput
watch -n 1 'just queue-stats'

# 3. Analyze metrics in Grafana
just docker-grafana
# Navigate to "Empath MTA - Delivery" dashboard
```

---

## Profiling Workflows

### Scenario 1: High CPU Usage

**Symptom**: CPU at 100%, slow message processing

**Diagnosis**:
```bash
# 1. Generate flamegraph
just profile-cpu

# 2. Identify hot functions (widest bars)
# Common culprits: DNS lookups, regex parsing, serialization

# 3. Profile specific operation
just bench-group <suspected-operation>
```

**Example Fix**:
```
If flamegraph shows 40% time in `dns::lookup`:
→ Add DNS caching (empath-delivery/src/dns.rs)
→ Benchmark before/after with: cargo bench -p empath-delivery -- dns
```

---

### Scenario 2: High Memory Usage

**Symptom**: Memory usage grows over time, potential leak

**Diagnosis**:
```bash
# 1. Run with dhat
cargo run --bin empath-dhat -- empath.config.ron

# 2. Let run for 5-10 minutes
# 3. Stop and analyze dhat-heap.json

# 4. Look for:
# - High "at t-gmax" (memory not freed at peak)
# - High "Total bytes" (excessive allocations)
```

**Common Issues**:
- **Unbounded queues**: Use bounded channels (`tokio::sync::mpsc::channel(100)`)
- **Connection leaks**: Ensure `TcpStream::shutdown()` called
- **Spool accumulation**: Old messages not cleaned up

---

### Scenario 3: Slow Message Delivery

**Symptom**: Messages queued for long time

**Diagnosis**:
```bash
# 1. Check queue age metrics
just queue-stats

# 2. Profile delivery processor
cargo bench -p empath-delivery

# 3. Check DNS resolution time
# Look at empath.dns.lookup.duration.seconds histogram in Prometheus

# 4. Check SMTP client timeouts
# Look for timeout errors in logs: docker-logs | grep -i timeout
```

**Common Bottlenecks**:
- DNS resolution (add caching)
- TLS handshake overhead (connection pooling - task 2.2)
- Sequential delivery (already parallelized via `JoinSet`)
- Rate limiting too aggressive (tune `messages_per_second`)

---

## Common Optimization Patterns

### 1. Reduce Allocations with `Cow`

**Problem**: Unnecessary string allocations in hot paths

**Before** (allocates String):
```rust
fn process_command(cmd: &str) -> String {
    cmd.to_uppercase()  // Always allocates
}
```

**After** (zero-copy when possible):
```rust
use std::borrow::Cow;

fn process_command(cmd: &str) -> Cow<str> {
    if cmd.chars().all(|c| c.is_uppercase()) {
        Cow::Borrowed(cmd)  // No allocation!
    } else {
        Cow::Owned(cmd.to_uppercase())
    }
}
```

**Impact**: Flamegraph width of `alloc::` functions shrinks.

**Empath Example**: `empath-smtp/src/command.rs` uses this pattern for SMTP commands.

---

### 2. Preallocate Collections

**Problem**: `Vec` reallocations during growth

**Before**:
```rust
let mut recipients = Vec::new();
for addr in addresses {
    recipients.push(addr);  // May reallocate 3-4 times
}
```

**After**:
```rust
let mut recipients = Vec::with_capacity(addresses.len());
for addr in addresses {
    recipients.push(addr);  // No reallocations
}
```

**Impact**: dhat shows fewer heap allocations, flamegraph shows less time in `Vec::grow`.

---

### 3. Use `Arc` for Shared Data

**Problem**: Expensive clones of large structs

**Before** (clones entire Context):
```rust
fn validate(ctx: Context) -> bool {
    // ctx is moved, caller must clone
}

let result = validate(ctx.clone());  // Expensive!
```

**After** (reference-counted sharing):
```rust
fn validate(ctx: &Arc<Context>) -> bool {
    // Cheap Arc clone when needed
}

let ctx = Arc::new(context);
let result = validate(&ctx);  // Arc clone is just pointer + refcount
```

**Impact**: Flamegraph shows significantly less time in `Context::clone`.

**Empath Example**: `empath-common/src/context.rs` uses `Arc<Envelope>` and `Arc<[u8]>` for shared data.

---

### 4. Cache Expensive Computations

**Problem**: Regex recompilation on every call

**Before**:
```rust
fn is_valid_domain(domain: &str) -> bool {
    let regex = Regex::new(r"^[a-z0-9.-]+$").unwrap();  // Recompiled!
    regex.is_match(domain)
}
```

**After** (cached regex):
```rust
use once_cell::sync::Lazy;

static DOMAIN_REGEX: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^[a-z0-9.-]+$").unwrap()
});

fn is_valid_domain(domain: &str) -> bool {
    DOMAIN_REGEX.is_match(domain)  // Reuse compiled regex
}
```

**Impact**: Flamegraph shows `Regex::new` only once at startup.

---

### 5. Perfect Hash Maps for Known Keys

**Problem**: Hash function overhead for static keys

**Before** (runtime hashing):
```rust
let mut params: HashMap<String, String> = HashMap::new();
params.insert("SIZE".to_string(), value);
```

**After** (compile-time perfect hash):
```rust
use phf::phf_map;

static PARAM_IDS: phf::Map<&'static str, u8> = phf_map! {
    "SIZE" => 0,
    "BODY" => 1,
    "AUTH" => 2,
};
```

**Impact**: Constant-time lookups, no hash function calls in flamegraph.

**Empath Example**: `empath-smtp/src/mail_parameters.rs` uses perfect hashing for ESMTP parameters.

---

### 6. Byte String Literals

**Problem**: String → bytes conversion overhead

**Before**:
```rust
let quit_response = "221 Goodbye\r\n".as_bytes();  // Runtime conversion
```

**After**:
```rust
let quit_response = b"221 Goodbye\r\n";  // Compile-time &[u8]
```

**Impact**: Removes allocation/conversion from flamegraph.

**Empath Example**: `empath-smtp/src/response.rs` uses byte string literals throughout.

---

### 7. Minimize Lock Scope

**Problem**: Holding lock during expensive operations

**Before** (lock held during processing):
```rust
let mut queue = QUEUE.lock().unwrap();
for msg in &queue.messages {
    process_expensive(msg);  // Blocks other threads!
}
```

**After** (minimize critical section):
```rust
let messages = {
    let queue = QUEUE.lock().unwrap();
    queue.messages.clone()  // Clone IDs only, release lock
};
for msg in messages {
    process_expensive(&msg);  // Process outside lock
}
```

**Impact**: Flamegraph shows less lock contention, better parallelism.

**Empath Example**: `empath-ffi/src/modules/mod.rs` uses `Arc<[Module]>` for lock-free reads after initialization.

---

### 8. Avoid Redundant UTF-8 Validation

**Problem**: Multiple UTF-8 checks on same data

**Before**:
```rust
fn process(data: &[u8]) -> String {
    String::from_utf8(data.to_vec()).unwrap()  // Validates UTF-8
}

let text = process(data);
let upper = text.to_uppercase();  // Re-validates UTF-8
```

**After** (unsafe, but proven invariant):
```rust
fn process(data: &[u8]) -> String {
    // SAFETY: Data is ASCII (0x00-0x7F), which is valid UTF-8
    unsafe { String::from_utf8_unchecked(data.to_vec()) }
}
```

**Impact**: Removes UTF-8 validation from flamegraph (when safe).

**Empath Example**: `empath-smtp/src/command.rs:374` uses this for parsed SMTP commands (proven ASCII).

---

### 9. Use `parking_lot` Instead of `std::sync`

**Problem**: Slower standard library mutexes

**Before**:
```rust
use std::sync::Mutex;

static COUNTER: Mutex<u64> = Mutex::new(0);
```

**After** (faster, smaller locks):
```rust
use parking_lot::Mutex;

static COUNTER: Mutex<u64> = Mutex::const_new(parking_lot::lock_api::RawMutex::INIT, 0);
```

**Impact**: 2-5x faster lock/unlock in microbenchmarks, smaller memory footprint.

**Empath Example**: `Cargo.toml` uses `parking_lot` workspace-wide.

---

### 10. Batch I/O Operations

**Problem**: Many small writes to disk/network

**Before** (one write per message):
```rust
for message in messages {
    file.write_all(message.as_bytes())?;  // Many syscalls
}
```

**After** (buffered writing):
```rust
use std::io::BufWriter;

let mut writer = BufWriter::new(file);
for message in messages {
    writer.write_all(message.as_bytes())?;  // Batched
}
writer.flush()?;
```

**Impact**: Flamegraph shows fewer syscalls, faster I/O.

---

### 11. Inline Hot Functions

**Problem**: Function call overhead in tight loops

**Before**:
```rust
fn is_whitespace(c: char) -> bool {
    c == ' ' || c == '\t'
}

// Called millions of times
for c in data.chars() {
    if is_whitespace(c) { ... }  // Function call overhead
}
```

**After**:
```rust
#[inline]
fn is_whitespace(c: char) -> bool {
    c == ' ' || c == '\t'
}
```

**Impact**: Compiler inlines the function, removing call overhead from flamegraph.

**Note**: Use `#[inline]` sparingly - only for hot, small functions (< 10 lines). Over-inlining increases binary size and reduces instruction cache efficiency.

---

### 12. Use `ahash` for HashMap

**Problem**: `std::collections::HashMap` uses SipHash (crypto-secure, slow)

**Before**:
```rust
use std::collections::HashMap;

let mut map: HashMap<String, String> = HashMap::new();
```

**After** (non-cryptographic, faster):
```rust
use ahash::AHashMap;

let mut map: AHashMap<String, String> = AHashMap::new();
```

**Impact**: 2-3x faster hashing in flamegraph.

**Empath Example**: `empath-common/src/context.rs` uses `AHashMap` for context metadata.

---

### Verification Checklist

After applying optimizations:

- [ ] Run benchmarks: `just bench-baseline-compare before`
- [ ] Check flamegraph: `just profile-cpu`
- [ ] Verify correctness: `cargo test`
- [ ] Check memory: Use dhat to ensure no regressions
- [ ] Document changes: Update comments with performance rationale

---

## CI Integration

**Benchmark Regression Detection** (CI workflow example):

```yaml
name: Benchmark Regression Detection

on:
  pull_request:
    branches: [main, master]

jobs:
  bench:
    runs-on: ubuntu-latest
    steps:
      - name: Checkout repository
        run: git fetch --unshallow || true

      - name: Install nightly toolchain
        run: rustup toolchain install nightly

      - name: Checkout main and save baseline
        run: |
          git checkout main
          cargo bench -- --save-baseline main

      - name: Checkout change and compare
        run: |
          git checkout ${CI_COMMIT_SHA}
          cargo bench -- --baseline main --baseline-lenient

      # Fails if any benchmark regresses >10%
```

---

## Performance Targets

Based on NEW-22 (Capacity Planning), Empath should achieve:

### Throughput
- **SMTP Reception**: >1000 messages/sec (single listener)
- **Delivery**: >500 deliveries/sec (with parallelism, no rate limits)
- **DNS Lookups**: <10ms p95 (with cache enabled)

### Latency
- **Queue Age**: p95 <5 minutes (SLO from NEW-05)
- **SMTP Command**: <100ms per command
- **Spool Write**: <5ms per message

### Resource Usage
- **Memory**: <512MB baseline, <2GB under load (10k queued messages)
- **CPU**: <50% average (4 cores)
- **Disk I/O**: <100 IOPS (SSDs)

**Verify with**:
```bash
# Throughput test
seq 1 10000 | parallel -j 50 swaks --server localhost:1025 ...

# Check metrics in Grafana
just docker-grafana
# Dashboard: "Empath MTA - Performance"
```

---

## Optimization Checklist

Before profiling:
- [ ] Use release build (`--release`)
- [ ] Enable LTO if not default (check `Cargo.toml` profile)
- [ ] Disable debug logging (`RUST_LOG=warn`)
- [ ] Close background apps
- [ ] Run on production-like hardware

During profiling:
- [ ] Run long enough to reach steady state (5-10 minutes)
- [ ] Use realistic workload (not just synthetic)
- [ ] Profile both CPU and memory
- [ ] Take baseline measurements first

After optimization:
- [ ] Benchmark before/after (use baselines)
- [ ] Verify correctness (run full test suite)
- [ ] Check for regressions in other areas
- [ ] Update benchmarks if behavior changed

---

## Tools Reference

| Tool | Platform | Purpose | Output |
|------|----------|---------|--------|
| `flamegraph` | Linux/macOS | CPU profiling | flamegraph.svg (interactive) |
| `perf` | Linux | CPU + hardware counters | Terminal report |
| Instruments | macOS | CPU/Memory/I/O/Network | .trace file (GUI) |
| `dhat` | All | Heap profiling | dhat-heap.json |
| Valgrind Massif | Linux | Memory over time | massif.out |
| Criterion | All | Microbenchmarks | target/criterion/ |

---

## Further Reading

- **Rust Performance Book**: https://nnethercote.github.io/perf-book/
- **Criterion.rs User Guide**: https://bheisler.github.io/criterion.rs/book/
- **Flamegraph Guide**: https://github.com/flamegraph-rs/flamegraph
- **dhat Documentation**: https://docs.rs/dhat/latest/dhat/
- **The Rust Performance Book**: https://nnethercote.github.io/perf-book/profiling.html

---

## Troubleshooting

**"perf_event_open failed: Permission denied"**:
```bash
# Temporary fix (until reboot)
echo 0 | sudo tee /proc/sys/kernel/perf_event_paranoid

# Permanent fix
echo 'kernel.perf_event_paranoid=0' | sudo tee -a /etc/sysctl.conf
sudo sysctl -p
```

**"Cannot find debug symbols"**:
```toml
# Add to Cargo.toml
[profile.release]
debug = 1  # Include debug symbols without disabling optimizations
```

**Flamegraph shows "unknown" functions**:
```bash
# Install debug symbols for system libraries (Linux)
sudo apt-get install libc6-dbg

# Rebuild with symbols
cargo clean
cargo build --release
```

**Benchmarks show high variance**:
```bash
# Disable CPU frequency scaling
sudo cpupower frequency-set --governor performance

# Disable turbo boost
echo 1 | sudo tee /sys/devices/system/cpu/intel_pstate/no_turbo
```
