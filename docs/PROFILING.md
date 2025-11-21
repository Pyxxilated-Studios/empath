# Empath MTA - Performance Profiling Guide

This guide covers profiling techniques for optimizing Empath MTA performance.

## Quick Start

```bash
# CPU profiling with flamegraph
just profile-cpu

# Memory profiling with dhat
just profile-memory  # (to be added)

# Benchmark with baseline comparison
just bench-baseline-save main
# ... make changes ...
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
# Profile the main binary
sudo cargo flamegraph --bin empath -- empath.config.ron

# Profile with release optimizations
sudo cargo flamegraph --release --bin empath -- empath.config.ron

# Profile specific crate's benchmarks
sudo cargo flamegraph --bench smtp_benchmarks

# Open result
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
sudo cargo flamegraph --release --bin empath -- empath.config.ron

# 2. Identify hot functions (widest bars)
# Common culprits: DNS lookups, regex parsing, serialization

# 3. Profile specific operation
cargo bench -- <suspected-operation>
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

## CI Integration

**Benchmark Regression Detection** (.gitea/workflows/bench.yml):

```yaml
name: Benchmark Regression Detection

on:
  pull_request:
    branches: [main, master]

jobs:
  bench:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v6
        with:
          fetch-depth: 0  # Need full history for baseline

      - uses: dtolnay/rust-toolchain@nightly

      - name: Checkout main and save baseline
        run: |
          git checkout main
          cargo bench -- --save-baseline main

      - name: Checkout PR and compare
        run: |
          git checkout ${{ github.sha }}
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
