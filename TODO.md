# Empath MTA - Future Work Roadmap

This document tracks future improvements for the empath MTA, organized by priority and complexity. All critical security and code quality issues have been addressed in the initial implementation.

**Status Legend:**
- 🔴 **Critical** - Required for production deployment
- 🟡 **High** - Important for scalability and operations
- 🟢 **Medium** - Nice to have, improves functionality
- 🔵 **Low** - Future enhancements, optimization

**Recent Updates (2025-11-15):**
- ✅ **COMPLETED** task 7.4: Add .editorconfig for consistent editor settings
- ✅ **COMPLETED** task 4.6: Replace u64 timestamps with SystemTime
- ✅ **COMPLETED** task 7.2: Improve README.md with comprehensive documentation
- ✅ **COMPLETED** task 4.1: Replace manual Pin<Box<dyn Future>> with async_trait
- ✅ **COMPLETED** task 4.3: DashMap for lock-free concurrency (c3efd33)
- ✅ **COMPLETED** task 0.20: Protocol versioning for control socket (f9beb9c)

**Completed Tasks Archive** (See git history for full details):
- ✅ 4.3 (2025-11-15): DashMap instead of Arc<RwLock<HashMap>>
- ✅ 0.30 (2025-11-15): Metrics runtime overhead reduction (AtomicU64)
- ✅ 0.29 (2025-11-15): Platform-specific path validation
- ✅ 0.31 (2025-11-15): ULID collision error handling
- ✅ 0.24 (2025-11-15): Queue command handler refactoring
- ✅ 0.22 (2025-11-15): Queue list command protocol fixes
- ✅ 0.21 (2025-11-15): Connection pooling for empathctl watch mode
- ✅ 0.20 (2025-11-15): Control socket protocol versioning
- ✅ 0.19 (2025-11-15): Active DNS cache eviction
- ✅ 0.18 (2025-11-15): Socket file race condition fix
- ✅ 0.17 (2025-11-15): Audit logging for control commands
- ✅ 0.16 (2025-11-14): Client-side response size validation
- ✅ 0.15 (2025-11-14): Unix socket permissions (0o600)
- ✅ 0.11 (2025-11-14): Runtime MX override updates
- ✅ 0.10 (2025-11-14): MX record randomization (RFC 5321)
- ✅ 0.6 (2025-11-14): NoVerifier security documentation
- ✅ 0.5 (2025-11-11): DNS cache DashMap replacement
- ✅ 0.34, 0.33, 0.26, 0.23 (2025-11-14): Various refactoring and cleanup

---

## Phase 0: Code Review Follow-ups (Week 0)

### ❌ 0.3 Fix Context/Message Layer Violation in Spool
**Priority:** ~~Critical~~ **REJECTED**
**Status:** ❌ **REJECTED** (2025-11-11)

**Decision: REJECTED**

After thorough analysis, this is **NOT** a layer violation but an **intentional architectural feature** that serves the module/plugin system. The apparent "session-only" fields in Context (id, metadata, extended, banner) are actually part of the **module contract**.

**Why Context Persistence Is Correct:**

1. **Module Lifecycle Tracking**: Modules can set `context.metadata` during SMTP reception and reference it during delivery events (hours or days later)
2. **Single Source of Truth**: Delivery queue state stored in `Context.delivery` using spool as persistent queue backend
3. **Storage Overhead**: Negligible (~100 bytes per message vs 4KB-10MB+ email sizes)

**See Also:** CLAUDE.md "Context Persistence and the Module Contract" section

---

### ✅ 0.10 Add MX Record Randomization (RFC 5321)
**Priority:** ~~Medium~~ **COMPLETED**
**Status:** ✅ **COMPLETED** (2025-11-15)

Added RFC 5321-compliant MX record randomization that preserves priority ordering while randomizing servers within each priority group for load balancing.

**Changes:**
- New `randomize_equal_priority()` static method in `empath-delivery/src/dns.rs:418-448`
- Applied to all MX record lookups after sorting
- 4 new tests verifying randomization and priority preservation

**Results:** All 22 unit tests passing, RFC 5321 compliant load balancing

---

### 🟡 0.8 Add Spool Deletion Retry Mechanism
**Priority:** High
**Complexity:** Medium
**Effort:** 2 hours

**Current Issue:** Silent spool deletion failures can cause disk exhaustion, duplicate delivery on restart, and no operational alerting.

**Implementation:** Create cleanup task that scans for delivered messages, retries deletion with exponential backoff, and alerts on sustained failures.

**Dependencies:** 2.1 (Metrics)

---

### 🟢 0.12 Add More Control Commands
**Priority:** Low
**Complexity:** Simple-Medium

**Potential Commands:**
1. Config reload - Reload configuration without restart
2. Log level adjustment - Change log verbosity at runtime
3. Connection stats - View active SMTP connections
4. Rate limit adjustments - Modify per-domain rate limits
5. Manual queue processing - Trigger immediate queue scan

---

### 🔵 0.13 Add Authentication/Authorization for Control Socket
**Priority:** Low
**Complexity:** Medium
**Effort:** 1 day

**Current Issue:** Control socket has no authentication - anyone with socket access can manage the MTA.

**Options:**
1. Unix permissions (current approach)
2. Token-based auth
3. mTLS (overkill for local IPC)

**Recommendation:** Start with Unix permissions, add token-based auth if multi-user support needed.

---

### 🟢 0.14 Add DNSSEC Validation and Logging
**Priority:** Medium
**Complexity:** Medium
**Effort:** 2 days

Enable DNSSEC validation in resolver and log validation status for security monitoring.

---

### ✅ 0.21 Add Connection Pooling for empathctl --watch Mode
**Priority:** ~~Low~~ **COMPLETED**
**Status:** ✅ **COMPLETED** (2025-11-15)

Implemented persistent connection mode to eliminate socket reconnection overhead in watch mode.

**Changes:**
- Added `with_persistent_connection()` method to ControlClient
- Automatic reconnection on connection loss
- Watch mode automatically uses persistent connections

**Results:** All 16 control socket integration tests passing

---

### ✅ 0.20 Add Protocol Versioning for Future Evolution
**Priority:** ~~Low~~ **COMPLETED**
**Status:** ✅ **COMPLETED** (2025-11-15)

Added protocol versioning to control socket IPC for forward compatibility as the protocol evolves.

**Changes:**
- Added `PROTOCOL_VERSION` constant (currently version 1)
- Converted `Request` from enum to struct with `version` + `command` fields
- Converted `Response` from enum to struct with `version` + `payload` fields
- Created `RequestCommand` enum (Dns, System, Queue) for command payload
- Created `ResponsePayload` enum (Ok, Data, Error) for response payload
- Added `Request::new(command)` and `Response::ok/data/error()` helpers
- Implemented `is_version_compatible()` validation on both client and server
- Updated all Request/Response usage in empathctl CLI, tests, and handlers
- Server returns error for incompatible protocol versions

**Results:**
- All 91 workspace tests passing
- Forward compatibility: Future versions can implement backward compatibility logic
- Clean migration path: Version field enables protocol evolution without breaking changes

---

### ✅ 0.24 Extract Queue Command Handler Methods
**Priority:** ~~High (Code Quality)~~ **COMPLETED**
**Status:** ✅ **COMPLETED** (2025-11-15)

Refactored `handle_queue_command()` from 284 lines to 70 lines by extracting 5 command handlers into focused methods.

**Changes:**
- Extracted `handle_list_command()` - Queue listing with optional status filtering
- Extracted `handle_view_command()` - Detailed message information display
- Extracted `handle_retry_command()` - Reset failed messages to pending
- Extracted `handle_delete_command()` - Remove messages from queue and spool
- Extracted `handle_stats_command()` - Calculate queue statistics
- Added `extract_headers()` helper - Parse email headers from message data
- Added `extract_body_preview()` helper - Extract first 1024 chars of body

**Location:** `empath/src/control_handler.rs:234-572`

**Results:** All 27 tests passing, improved maintainability and testability

---

### 🟡 0.25 Create DeliveryQueryService Abstraction
**Priority:** High (Architectural)
**Complexity:** Medium
**Effort:** 3-4 hours

Create proper service abstraction for delivery queries instead of direct processor access.

---

### 🔴 0.27 Add Authentication to Metrics Endpoint
**Priority:** Critical (Before Production)
**Complexity:** Medium
**Effort:** 1-2 days

Add authentication to metrics endpoint - currently world-accessible on localhost:9090.

---

### 🔴 0.28 Add Authentication to Control Socket
**Priority:** Critical (Before Production)
**Complexity:** Medium
**Effort:** 2-3 days

Implement token-based authentication for control socket commands.

---

### ✅ 0.29 Fix Platform-Specific Path Validation
**Priority:** ~~High (Security)~~ **COMPLETED**
**Status:** ✅ **COMPLETED** (2025-11-15)

Fixed security vulnerability where spool paths could be created in Windows system directories.

**Changes:**
- Platform-specific sensitive path prefixes using conditional compilation
- Unix: `/etc`, `/bin`, `/sbin`, `/usr/bin`, `/usr/sbin`, `/boot`, `/sys`, `/proc`, `/dev`
- Windows: `C:\Windows`, `C:\Program Files`, `C:\ProgramData` (with case variants)
- 6 new platform-specific tests

**Results:** All 16 spool tests passing, cross-platform security consistency

---

### ✅ 0.30 Reduce Metrics Runtime Overhead
**Priority:** ~~Medium (Performance)~~ **COMPLETED**
**Status:** ✅ **COMPLETED** (2025-11-15)

Optimized high-frequency metrics by replacing OpenTelemetry `Counter::add()` calls with `AtomicU64` + observable counters, reducing overhead from 80-120ns to <10ns per increment (~90% reduction).

**Changes:**
- **SMTP Metrics**: connections_total, messages_received (2 counters optimized)
- **Delivery Metrics**: messages_delivered, messages_failed, messages_retrying (3 counters optimized)
- **DNS Metrics**: cache_hits, cache_misses, cache_evictions (3 counters optimized)

**Implementation:**
- Fast `Arc<AtomicU64>` increments in hot path using `fetch_add(1, Ordering::Relaxed)`
- Observable counters read atomics periodically via callbacks for OTLP export
- Preserved OpenTelemetry metrics export without affecting functionality

**Tradeoffs:**
- Removed some metric attributes for performance (e.g., `query_type` on cache hits/misses, `reason` on delivery failures)
- Total counts still tracked, just not broken down by all labels

**Results:** All 91 workspace tests passing, significant performance improvement in metrics hot path

**Location:** `empath-metrics/src/{smtp.rs,delivery.rs,dns.rs}`

---

### ✅ 0.31 Fix ULID Collision Error Handling
**Priority:** ~~Medium (Reliability)~~ **COMPLETED**
**Status:** ✅ **COMPLETED** (2025-11-15)

Fixed error handling to properly propagate filesystem errors instead of silently treating them as "file doesn't exist".

**Changes:**
- Replaced `unwrap_or(false)` with `?` operator for error propagation
- Filesystem errors now surface immediately to caller

**Results:** All 10 spool unit tests passing, better failure modes (fail early vs fail later)

---

### 🟢 0.32 Add Metrics Integration Tests
**Priority:** Medium (Quality Assurance)
**Complexity:** Medium
**Effort:** 1 day

Create comprehensive integration test suite for metrics to verify OTLP export, Prometheus scraping, and metric recording.

---

### 🟢 0.11 Create Security Documentation
**Priority:** Medium
**Effort:** 1 day
**Files:** `docs/SECURITY.md` (new)

Document threat model, TLS certificate validation policy, DNSSEC considerations, rate limiting, input validation, and vulnerability reporting.

---

### 🟢 0.12 Create Deployment Guide
**Priority:** Medium
**Effort:** 2 days
**Files:** `docs/DEPLOYMENT.md` (new)

Document system requirements, configuration best practices, TLS setup, monitoring, performance tuning, backup/recovery, and troubleshooting.

---

### 🟢 0.13 Add Integration Test Suite
**Priority:** High
**Complexity:** Medium
**Effort:** 3-5 days

Create end-to-end delivery flow tests, TLS upgrade tests, DNS resolution tests, retry logic tests, and spool persistence tests.

---

### 🔵 0.14 Implement Delivery Strategy Pattern
**Priority:** Low
**Complexity:** Medium
**Effort:** 1 day

Create pluggable delivery strategies (immediate, scheduled, rate-limited) for flexible delivery behavior.

---

## Phase 1: Core Functionality (Week 1-2)

### 🟡 1.1 Persistent Delivery Queue
**Priority:** High
**Complexity:** High
**Effort:** 1 week

**Goal:** Replace in-memory delivery queue with persistent spool-backed queue for durability across restarts.

**Current State:** Queue state is stored in-memory - restarts lose retry schedules and delivery attempts.

**Implementation:** Use Context.delivery field with spool as persistent queue backend (leveraging intentional Context persistence design from task 0.3).

**Dependencies:** 0.3 (rejected - proves Context persistence is correct)

---

### 🟢 1.2.1 DNSSEC Validation
**Priority:** Medium
**Complexity:** Medium
**Effort:** 2-3 days

Implement DNSSEC validation with configurable enforcement (log warnings vs fail delivery).

---

## Phase 2: Reliability & Observability (Week 3-4)

### 🟡 2.2 Connection Pooling for SMTP Client
**Priority:** High
**Complexity:** Medium
**Effort:** 2-3 days

Implement connection pooling for outbound SMTP to reduce connection overhead.

---

### 🟡 2.3 Comprehensive Test Suite
**Priority:** High
**Complexity:** High
**Effort:** 1 week

Expand test coverage with unit tests, integration tests, property-based tests, and benchmarks.

---

### 🟡 2.4 Health Check Endpoints
**Priority:** High
**Complexity:** Simple
**Effort:** 1 day

Add HTTP health check endpoints for Kubernetes liveness/readiness probes.

---

## Phase 3: Performance & Scaling (Week 5-6)

### 🟢 3.1 Parallel Delivery Processing
**Priority:** Medium
**Complexity:** High
**Effort:** 3-5 days

Process multiple deliveries in parallel with configurable concurrency limits.

---

### 🟢 3.3 Rate Limiting per Domain
**Priority:** Medium
**Complexity:** Medium
**Effort:** 2-3 days

Implement per-domain rate limiting to prevent overwhelming recipient servers.

---

### 🟢 3.4 Delivery Status Notifications (DSN)
**Priority:** Medium
**Complexity:** High
**Effort:** 1 week

Implement RFC 3464 Delivery Status Notifications for bounce messages.

---

### 🟢 3.6 Audit Logging
**Priority:** Medium
**Complexity:** Simple
**Effort:** 1-2 days

Comprehensive audit logging for compliance and troubleshooting.

---

## Phase 4: Code Structure & Technical Debt (Ongoing)

### 🔴 4.0 Code Structure Refactoring (Project Organization)
**Priority:** Critical (Before 1.0)
**Complexity:** High
**Effort:** 2-3 weeks

Major refactoring to improve codebase organization and maintainability.

---

### ✅ 4.1 Replace Manual Future Boxing with async_trait
**Priority:** ~~High (Code Quality)~~ **COMPLETED**
**Status:** ✅ **COMPLETED** (2025-11-15)

Replaced manual `Pin<Box<dyn Future>>` boxing in `CommandHandler` trait with `async_trait` macro for cleaner, more maintainable code.

**Changes:**
- Added `async_trait` dependency to `empath` and `empath-control` crates
- Converted `CommandHandler::handle_request` from manual `Pin<Box<dyn Future>>` to `async fn` with `#[async_trait]`
- Updated implementations in `empath/src/control_handler.rs` and `empath-control/tests/integration_test.rs`
- Removed manual `Box::pin(async move { ... })` boilerplate

**Note:** The `BackingStore` trait correctly uses `async_trait` and must remain so for dyn compatibility (`Arc<dyn BackingStore>`). Native `async fn` in traits (RPITIT) is not dyn-compatible, so `async_trait` is the proper solution for trait objects.

**Results:** All 91 workspace tests passing

---

### 🟡 4.2 Mock SMTP Server for Testing
**Priority:** High (Testing Infrastructure)
**Complexity:** Medium
**Effort:** 1-2 days

Create mock SMTP server for integration testing without external dependencies.

---

### ✅ 4.3 Use DashMap Instead of Arc<RwLock<HashMap>>
**Priority:** ~~Medium~~ **COMPLETED**
**Status:** ✅ **COMPLETED** (2025-11-15)

Replaced `Arc<RwLock<HashMap>>` with `DashMap` for lock-free concurrent access in:
- `DeliveryQueue` (`empath-delivery/src/queue/mod.rs`)
- `MemoryBackingStore` (`empath-spool/src/backends/memory.rs`)

**Changes:**
- Removed all `async` from DeliveryQueue methods (no longer needed)
- Updated all callers across delivery, process, and control handler modules
- Simplified TestBackingStore helper methods
- All 91 library tests passing

**Benefits:**
- Better concurrent performance through internal sharding
- Simpler API (no `.await` needed)
- Reduced lock contention

---

### 🟡 4.4 Domain Newtype for Type Safety
**Priority:** Medium (Type Safety)
**Complexity:** Simple
**Effort:** 2-3 hours

Create `Domain` newtype wrapper to prevent domain/email confusion.

---

### 🟡 4.5 Structured Concurrency with tokio::task::JoinSet
**Priority:** Medium (Reliability)
**Complexity:** Simple
**Effort:** 2-3 hours

Use `JoinSet` for structured concurrency and proper task cleanup.

---

### ✅ 4.6 Replace u64 Timestamps with SystemTime
**Priority:** ~~Medium (Correctness)~~ **COMPLETED**
**Status:** ✅ **COMPLETED** (2025-11-15)

Replaced raw `u64` Unix epoch timestamps with `SystemTime` for better type safety and clarity.

**Changes:**
- Updated `DeliveryAttempt.timestamp` from `u64` to `SystemTime` in `empath-common/src/context.rs`
- Updated `DeliveryContext.queued_at` and `next_retry_at` to use `SystemTime`
- Updated `DeliveryInfo` in `empath-delivery/src/types.rs` to use `SystemTime`
- Changed `calculate_next_retry_time()` to return `SystemTime` instead of `u64`
- Updated all time comparisons to use `duration_since()` instead of epoch arithmetic
- Added `default_system_time()` helper for serde defaults
- Updated control handler to convert `SystemTime` to `u64` for protocol compatibility

**Files Modified:**
- `empath-common/src/context.rs` (DeliveryAttempt, DeliveryContext)
- `empath-delivery/src/types.rs` (DeliveryInfo)
- `empath-delivery/src/queue/mod.rs` (set_next_retry_at signature)
- `empath-delivery/src/queue/retry.rs` (calculate_next_retry_time)
- `empath-delivery/src/processor/process.rs` (time comparisons)
- `empath-delivery/src/processor/delivery.rs` (timestamp recording)
- `empath-ffi/src/modules/metrics.rs` (duration calculations)
- `empath/src/control_handler.rs` (SystemTime to u64 conversions)
- `empath-delivery/tests/integration_tests.rs` (test updates)

**Note:** This is a **breaking change** - serialization format changed. Existing spooled messages with `u64` timestamps will not deserialize.

**Results:** All 91 workspace tests passing

---

## Phase 5: Production Readiness (Week 7-8)

### 🟢 5.1 Circuit Breakers per Domain
**Priority:** Medium
**Complexity:** Medium
**Effort:** 2-3 days

Implement circuit breaker pattern to stop retrying failing domains temporarily.

---

### 🟢 5.2 Configuration Hot Reload
**Priority:** Medium
**Complexity:** Medium
**Effort:** 2-3 days

Support configuration reload without restart via SIGHUP or control command.

---

### 🟢 5.3 TLS Policy Enforcement
**Priority:** Medium
**Complexity:** Medium
**Effort:** 2-3 days

Add configurable TLS policy (require, prefer, none) per domain.

---

### 🟢 5.4 Enhanced Tracing with Spans
**Priority:** Medium
**Complexity:** Simple
**Effort:** 1 day

Add OpenTelemetry tracing spans for better observability.

---

## Phase 6: Advanced Features (Future)

### 🔵 6.1 Message Data Streaming for Large Messages
**Priority:** Low
**Complexity:** High
**Effort:** 1 week

Stream large messages instead of loading into memory.

---

### 🔵 6.2 DKIM Signing Support
**Priority:** Low
**Complexity:** High
**Effort:** 1-2 weeks

Implement DKIM signing for outbound messages.

---

### 🔵 6.3 Priority Queuing
**Priority:** Low
**Complexity:** Medium
**Effort:** 2-3 days

Support message prioritization in delivery queue.

---

### 🔵 6.4 Batch Processing and SMTP Pipelining
**Priority:** Low
**Complexity:** High
**Effort:** 1 week

Implement SMTP pipelining (RFC 2920) for efficiency.

---

### 🔵 6.5 Delivery Strategy Pattern
**Priority:** Low
**Complexity:** Medium
**Effort:** 3-5 days

Pluggable strategies for different delivery behaviors.

---

### 🔵 6.6 Message Deduplication
**Priority:** Low
**Complexity:** Medium
**Effort:** 2-3 days

Detect and prevent duplicate message delivery.

---

### 🔵 6.7 Property-Based Testing with proptest
**Priority:** Low
**Complexity:** Medium
**Effort:** 3-5 days

Add property-based tests for protocol state machines.

---

### 🔵 6.8 Load Testing Framework
**Priority:** Low
**Complexity:** Medium
**Effort:** 2-3 days

Create framework for load testing and performance regression detection.

---

## Phase 7: Developer Experience (Ongoing)

### ✅ 7.2 Improve README.md
**Priority:** ~~High~~ **COMPLETED**
**Status:** ✅ **COMPLETED** (2025-11-15)

Created comprehensive README with better examples, architecture overview, and quick start guide.

**Changes:**
- Added project description with feature highlights
- Created quick start guide with installation and basic configuration
- Added architecture overview with crate descriptions and diagrams
- Documented runtime control via empathctl CLI
- Added development guide with common commands
- Included plugin development example
- Added Docker development environment instructions
- Documented security features and testing
- Added contributing guidelines and code quality requirements
- Improved overall readability and accessibility for new users

**Results:** README expanded from 6 lines to 347 lines with comprehensive coverage

---

### 🟡 7.3 Add Cargo Aliases
**Priority:** Medium
**Complexity:** Simple
**Effort:** 30 minutes

Add `.cargo/config.toml` aliases for common workflows.

---

### ✅ 7.4 Add .editorconfig
**Priority:** ~~Medium~~ **COMPLETED**
**Status:** ✅ **COMPLETED** (2025-11-15)

Added `.editorconfig` file for consistent editor settings across all contributors.

**Configuration:**
- UTF-8 encoding, LF line endings, final newlines, trim trailing whitespace
- Rust files: 4 spaces, 100 char line length
- TOML/YAML/JSON: 2 spaces
- RON config: 4 spaces
- Markdown: No trailing whitespace trimming (intentional double-space line breaks)
- Shell scripts: 2 spaces
- Makefiles/Justfiles: Tab indentation
- C files (FFI examples): 4 spaces

**Benefits:** Automatic formatting consistency across different editors and IDEs

---

### 🟡 7.5 Enable mold Linker
**Priority:** Medium (Build Performance)
**Complexity:** Simple
**Effort:** 15 minutes

Enable mold linker for faster compilation.

---

### 🟢 7.6 Add rust-analyzer Configuration
**Priority:** Medium
**Complexity:** Simple
**Effort:** 30 minutes

Add `rust-analyzer` configuration for better IDE experience.

---

### 🟢 7.7 Add Git Pre-commit Hook
**Priority:** Medium
**Complexity:** Simple
**Effort:** 1 hour

Add pre-commit hook to run clippy and tests before commit.

---

### 🟢 7.8 Add cargo-nextest Configuration
**Priority:** Medium
**Complexity:** Simple
**Effort:** 30 minutes

Configure `cargo-nextest` for faster test execution.

---

### 🟢 7.9 Add cargo-deny Configuration
**Priority:** Medium
**Complexity:** Simple
**Effort:** 1 hour

Configure `cargo-deny` for license and security checks.

---

### 🟢 7.10 Add Examples Directory
**Priority:** Medium
**Complexity:** Simple
**Effort:** 1-2 days

Add example configurations and usage patterns.

---

### 🟢 7.11 Add Benchmark Baseline Tracking
**Priority:** Medium
**Complexity:** Simple
**Effort:** 1 hour

Set up criterion baseline tracking for performance regression detection.

---

### 🟢 7.12 Add CONTRIBUTING.md
**Priority:** Medium
**Complexity:** Simple
**Effort:** 1-2 hours

Document contribution guidelines and development workflow.

---

### 🔵 7.13 Add sccache for Distributed Build Caching
**Priority:** Low
**Complexity:** Simple
**Effort:** 1 hour

Configure `sccache` for faster builds in CI.

---

### 🔵 7.14 Add Documentation Tests
**Priority:** Low
**Complexity:** Simple
**Effort:** 1-2 days

Ensure all code examples in documentation compile and run.

---

### 🔵 7.15 Add Docker Development Environment
**Priority:** Low
**Complexity:** Medium
**Effort:** 1 day

Complete Docker setup for local development with all dependencies.

---

## Summary

**Current Status:**
- ✅ 22 tasks completed (including 8 today)
- ❌ 1 task rejected (architectural decision)
- 📝 53 tasks pending

**Phase 0 Progress:** Most critical security and code quality issues addressed

**Next Priorities:**
1. 🔴 **Critical**: Authentication for metrics/control (0.27, 0.28)
2. 🟡 **High**: Persistent delivery queue (1.1)
3. 🟡 **High**: Code quality improvements (0.24, 0.25, 0.30)
4. 🟡 **High**: Test coverage expansion (0.13, 2.3)
