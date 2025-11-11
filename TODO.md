# Empath MTA - Future Work Roadmap

This document tracks future improvements for the empath MTA, organized by priority and complexity. All critical security and code quality issues have been addressed in the initial implementation.

**Status Legend:**
- 🔴 **Critical** - Required for production deployment
- 🟡 **High** - Important for scalability and operations
- 🟢 **Medium** - Nice to have, improves functionality
- 🔵 **Low** - Future enhancements, optimization

**Recent Updates:**
- **2025-11-11:** ✅ Implemented exponential backoff with message expiration
- **2025-11-11:** ✅ Implemented graceful shutdown handling with 30s timeout
- **2025-11-10:** Comprehensive code review completed (see CODE_REVIEW_2025-11-10.md)
- **2025-11-10:** ✅ Fixed TLS certificate validation (two-tier configuration system)
- **2025-11-10:** ✅ Implemented comprehensive SMTP operation timeouts (server and client)
- **2025-11-10:** ✅ Added empathctl queue management CLI utility
- **2025-11-10:** ✅ Extracted SMTP transaction logic into separate module
- **2025-11-09:** ✅ Implemented typed error handling with thiserror
- **2025-11-09:** ✅ Added per-domain configuration with DNS MX resolution
- **2025-11-08:** ✅ Comprehensive benchmarking infrastructure with Criterion.rs
- **2025-11-08:** ✅ Reduced clone usage by ~80% in hot paths

---

## Phase 0: Code Review Follow-ups (Week 0)

### ✅ 0.1 TLS Certificate Validation Security Fix
**Priority:** Critical
**Complexity:** Medium
**Effort:** 1 day
**Status:** ✅ **COMPLETED** (2025-11-10)

**Implementation:**
- ✅ Added global `accept_invalid_certs` flag to DeliveryProcessor (defaults to false)
- ✅ Added per-domain `accept_invalid_certs` override to DomainConfig
- ✅ Implemented priority resolution (per-domain > global)
- ✅ Added security warning logging when validation disabled
- ✅ Updated example configuration with commented examples
- ✅ Comprehensive documentation in CLAUDE.md
- ✅ Added test coverage for two-tier configuration

**Files Modified:**
- `empath-delivery/src/lib.rs`
- `empath-delivery/src/domain_config.rs`
- `empath.config.ron`
- `CLAUDE.md`

---

### ✅ 0.2 Add SMTP Operation Timeouts
**Priority:** Critical
**Complexity:** Simple
**Effort:** 1 day
**Status:** ✅ **COMPLETED** (2025-11-10)

**Implementation:**
- ✅ Server-side RFC 5321-compliant timeouts with state-aware selection
- ✅ Client-side per-operation timeouts for all SMTP commands
- ✅ Configurable timeouts via empath.config.ron
- ✅ Connection lifetime tracking (max 30 minutes)
- ✅ Comprehensive logging of timeout events
- ✅ Security benefits: prevents slowloris attacks and DoS vulnerabilities

**Server-side timeouts:**
- command_secs: 300s (regular commands)
- data_init_secs: 120s (DATA command)
- data_block_secs: 180s (between data chunks)
- data_termination_secs: 600s (processing after final dot)
- connection_secs: 1800s (maximum session lifetime)

**Client-side timeouts:**
- connect_secs, ehlo_secs, starttls_secs, mail_from_secs, rcpt_to_secs
- data_secs: 120s (longer for large messages)
- quit_secs: 10s (logged but doesn't fail delivery)

**Files Modified:**
- `empath-delivery/src/lib.rs`
- `empath-smtp/src/lib.rs`
- `empath-smtp/src/session.rs`
- `empath.config.ron`
- `CLAUDE.md`

**Dependencies:** None
**Source:** CODE_REVIEW_2025-11-10.md Section 1.4

---

### 🔴 0.3 Fix Context/Message Layer Violation in Spool
**Priority:** Critical
**Complexity:** Medium
**Effort:** 1 day
**Files:** `empath-spool/src/spool.rs`, `empath-common/src/message.rs` (new)

**Current Issue:** Spool stores `Context` (session state) instead of `Message` (data), violating architectural layer separation.

**Implementation:**
```rust
// File: empath-common/src/message.rs (new)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub envelope: Envelope,
    pub data: Arc<[u8]>,
    pub received_timestamp: u64,
    pub delivery_metadata: DeliveryMetadata,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeliveryMetadata {
    pub helo_domain: String,
    pub source_ip: String,
    // Only delivery-relevant fields, NOT session state
}

// Conversion at boundaries
impl From<Context> for Message { /* extract relevant fields */ }
impl From<Message> for Context { /* reconstitute with defaults */ }

// Update spool API
pub trait Spool {
    async fn spool_message(&self, message: Message) -> Result<SpooledMessageId>;
    async fn read_message(&self, id: &SpooledMessageId) -> Result<Message>;
}
```

**Benefits:**
- Proper layer separation
- No session state persistence
- Cleaner architecture

**Dependencies:** None
**Source:** CODE_REVIEW_2025-11-10.md Section 2.1

---

### ✅ 0.4 Optimize String Cloning in Hot Path
**Priority:** High
**Complexity:** Simple
**Effort:** 1 hour
**Status:** ✅ **COMPLETED** (2025-11-08)

**Implementation:**
- ✅ Reduced clone usage by ~80% across hot paths
- ✅ Message builder refactoring using `std::mem::take()` instead of cloning
- ✅ BackingStore API changed to take ownership instead of reference
- ✅ Session Arc wrappers for `banner` and `init_context`
- ✅ Command::inner() optimization returning `Cow<'_, str>` instead of `String`

**Performance Impact:**
- Before: ~5,000-6,000 allocations/sec (1000 emails/sec workload)
- After: ~1,000 allocations/sec
- **Reduction: ~80% fewer allocations in hot paths**

**Files Modified:**
- `empath-smtp/src/session.rs`
- `empath-smtp/src/command.rs`
- `empath-spool/src/spool.rs`
- `empath-common/src/context.rs`

**Dependencies:** None
**Source:** CODE_REVIEW_2025-11-10.md Section 3.1

---

### 🟡 0.5 Fix DNS Cache Mutex Contention
**Priority:** High
**Complexity:** Simple
**Effort:** 1 hour
**Files:** `empath-delivery/src/dns.rs:133`

**Current Issue:** `Mutex<LruCache>` serializes all DNS lookups under load.

**Implementation:**
```rust
use dashmap::DashMap;

pub struct DnsResolver {
    resolver: TokioAsyncResolver,
    cache: Arc<DashMap<String, CachedResult>>,  // Lock-free reads
    config: DnsConfig,
}
```

**Benefits:**
- Lock-free concurrent reads
- Better throughput under load
- Lower latency

**Dependencies:** Add `dashmap` crate
**Source:** CODE_REVIEW_2025-11-10.md Section 3.2

---

### 🟡 0.6 Add Compile-Time Guard to NoVerifier
**Priority:** High
**Complexity:** Simple
**Effort:** 30 minutes
**Files:** `empath-smtp/src/client/smtp_client.rs`

**Current Issue:** `NoVerifier` accepts all certificates without compile-time protection.

**Implementation:**
```rust
#[cfg(any(test, feature = "insecure-tls"))]
pub struct NoVerifier;

// Add to Cargo.toml:
// [features]
// insecure-tls = []  # DANGEROUS: See SECURITY.md
```

**Alternative:** Remove `NoVerifier` entirely and rely on the two-tier configuration system.

**Dependencies:** None
**Source:** CODE_REVIEW_2025-11-10.md Section 1.2

---

### ✅ 0.7 Extract SmtpTransaction from DeliveryProcessor
**Priority:** High (Refactoring)
**Complexity:** Medium
**Effort:** 1 day
**Status:** ✅ **COMPLETED** (2025-11-10)

**Implementation:**
- ✅ Created new `smtp_transaction.rs` module with `SmtpTransaction` struct
- ✅ Extracted SMTP protocol logic from DeliveryProcessor
- ✅ Methods: `negotiate_tls()`, `send_mail_from()`, `send_rcpt_to()`, `send_message_data()`
- ✅ Updated DeliveryProcessor.deliver_message() to use SmtpTransaction
- ✅ Removed duplicated extract_email_address() function

**Benefits:**
- **Reduced lib.rs from 1522 to 1219 lines (20% reduction)**
- **Created focused 370-line smtp_transaction.rs module**
- Improved separation of concerns
- Makes code more testable and maintainable
- Prepares for future DeliveryStrategy pattern (TODO 6.5)

**Files Modified:**
- `empath-delivery/src/lib.rs` (reduced from 1522 to 1219 lines)
- `empath-delivery/src/smtp_transaction.rs` (new, 370 lines)

**Dependencies:** None
**Source:** CODE_REVIEW_2025-11-10.md Section 2.2

---

### 🟡 0.8 Add Spool Deletion Retry Mechanism
**Priority:** High
**Complexity:** Medium
**Effort:** 2 hours
**Files:** `empath-delivery/src/lib.rs:592`

**Current Issue:** Silent spool deletion failures can cause:
- Disk exhaustion
- Duplicate delivery on restart
- No operational alerting

**Implementation:**
```rust
// Add metrics
static SPOOL_DELETION_FAILURES: Counter = /* ... */;

// Create cleanup task
async fn cleanup_delivered_messages(&self) {
    loop {
        // Scan for messages in "delivered" state
        // Retry deletion with exponential backoff
        // Alert on sustained failures
        tokio::time::sleep(Duration::from_secs(300)).await;
    }
}

// Add queue state
pub enum DeliveryStatus {
    // ... existing states
    DeliveredPendingCleanup { delivered_at: u64 },
}
```

**Dependencies:** 2.1 (Metrics)
**Source:** CODE_REVIEW_2025-11-10.md Section 1.5

---

### 🟢 0.9 Add DNSSEC Validation and Logging
**Priority:** Medium
**Complexity:** Medium
**Effort:** 2 days
**Files:** `empath-delivery/src/dns.rs`

**Implementation:**
```rust
// Enable DNSSEC in resolver
let mut opts = ResolverOpts::default();
opts.validate = true;

// Log validation status
if let Some(dnssec) = &response.dnssec() {
    if !dnssec.is_secure() {
        warn!(domain = %domain, "DNSSEC validation failed");
    }
}
```

**Configuration:**
```ron
delivery: (
    dns: (
        dnssec: (
            enabled: true,
            enforce: false,  // Log warnings vs. fail delivery
        ),
    ),
)
```

**Dependencies:** 1.2.1 (DNSSEC Validation section exists)
**Source:** CODE_REVIEW_2025-11-10.md Section 1.3

---

### 🟢 0.10 Add MX Record Randomization (RFC 5321)
**Priority:** Medium
**Complexity:** Simple
**Effort:** 2 hours
**Files:** `empath-delivery/src/dns.rs:266-267`

**Current Issue:** Equal-priority MX records are not randomized as recommended by RFC 5321.

**Implementation:**
```rust
use rand::seq::SliceRandom;

// After sorting by priority, randomize within each priority group
let mut priority_groups: HashMap<u16, Vec<MailServer>> = HashMap::new();
for server in servers {
    priority_groups.entry(server.priority).or_default().push(server);
}

let mut result = Vec::new();
for priority in priority_groups.keys().sorted() {
    let mut group = priority_groups.remove(priority).unwrap();
    group.shuffle(&mut rand::thread_rng());
    result.extend(group);
}
```

**Dependencies:** None
**Source:** CODE_REVIEW_2025-11-10.md Section 1.3

---

### 🟢 0.11 Create Security Documentation
**Priority:** Medium
**Complexity:** Simple
**Effort:** 1 day
**Files:** `docs/SECURITY.md` (new)

**Contents:**
- Threat model for email delivery
- TLS certificate validation policy
- DNSSEC considerations
- Rate limiting to prevent abuse
- Input validation boundaries
- Vulnerability reporting process
- Security configuration best practices

**Dependencies:** None
**Source:** CODE_REVIEW_2025-11-10.md Section 5.1

---

### 🟢 0.12 Create Deployment Guide
**Priority:** Medium
**Complexity:** Simple
**Effort:** 2 days
**Files:** `docs/DEPLOYMENT.md` (new)

**Contents:**
- System requirements
- Configuration best practices
- TLS certificate setup
- Monitoring setup (metrics, logs)
- Performance tuning guide
- Backup and recovery procedures
- Troubleshooting common issues
- Production readiness checklist

**Dependencies:** None
**Source:** CODE_REVIEW_2025-11-10.md Section 5.2

---

### 🟢 0.13 Add Integration Test Suite
**Priority:** High
**Complexity:** Medium
**Effort:** 3-5 days
**Files:** `empath-delivery/tests/` (new)

**Test Categories:**
- End-to-end delivery flow with TLS
- Spool deletion failure scenarios
- DNS cache expiration
- TLS requirement enforcement
- Group address handling
- Timeout handling
- Error scenarios

**Dependencies:** 4.2 (Mock SMTP server)
**Source:** CODE_REVIEW_2025-11-10.md Section 4.1

---

### 🔵 0.14 Implement Delivery Strategy Pattern
**Priority:** Low (Extensibility)
**Complexity:** Medium
**Effort:** 2-3 days
**Files:** `empath-delivery/src/strategy.rs` (new)

**Implementation:**
```rust
#[async_trait]
pub trait DeliveryStrategy: Send + Sync {
    async fn deliver(&self, context: &Context, config: &DomainConfig)
        -> Result<(), DeliveryError>;
    fn supports_domain(&self, domain: &str) -> bool;
}

pub struct SmtpDeliveryStrategy { /* ... */ }
pub struct LmtpDeliveryStrategy { /* ... */ }  // Future
pub struct WebhookDeliveryStrategy { /* ... */ }  // Future
```

**Dependencies:** 0.3 (Message type), 0.7 (SmtpTransaction extraction)
**Source:** CODE_REVIEW_2025-11-10.md Section 2.3, TODO.md 6.5

---

## Phase 1: Production Foundation (Weeks 1-2)

### 🔴 1.1 Persistent Delivery Queue
**Priority:** Critical
**Complexity:** Medium
**Effort:** 2-3 days
**Files:** `empath-delivery/src/backends/file.rs` (new)

**Current Issue:** In-memory queue loses all state on restart, causing:
- Message delivery loss on crashes
- Lost retry counts and status tracking
- No audit trail

**Implementation:**
- Create `QueueBackend` trait abstraction
- Implement `FileQueueBackend` with atomic operations (similar to spool)
- Store queue entries as JSON: `{queue_dir}/{status}/{next_attempt_timestamp}_{message_id}_{domain}.json`
- Rebuild in-memory index on startup
- Future: Add SQLite backend for better query performance

**Dependencies:** None

---

### ✅ 1.2 Real DNS MX Lookups
**Priority:** Critical
**Complexity:** Medium
**Effort:** 1-2 days
**Status:** ✅ **COMPLETED** (2025-11-09)

**Implementation:**
- ✅ Added `hickory-resolver` dependency
- ✅ Implemented MX record resolution with priority sorting
- ✅ Handle missing MX (fallback to A/AAAA records per RFC 5321)
- ✅ Custom error types with temporary/permanent failure detection
- ✅ LRU cache with TTL respect (300 entries)
- ✅ Comprehensive test coverage including integration tests

**Files Modified:**
- `empath-delivery/src/dns.rs` (new, 418 lines)
- `empath-delivery/src/lib.rs`

**Dependencies:** None

---

### 🟢 1.2.1 DNSSEC Validation
**Priority:** Medium
**Complexity:** Medium
**Effort:** 2-3 days
**Files:** `empath-delivery/src/dns.rs`

**Current Issue:** No DNSSEC validation for DNS responses, vulnerable to DNS spoofing

**Implementation:**
- Enable DNSSEC validation in hickory-resolver
- Configure trusted root keys
- Add validation status to MailServer results
- Log DNSSEC failures for monitoring
- Make DNSSEC enforcement configurable (warn vs. fail)

**Configuration:**
```ron
// In empath.config.ron
Empath (
    // ... other config ...
    delivery: (
        dns: (
            dnssec: (
                enabled: true,
                enforce: false,  // Log warnings instead of failing
            ),
        ),
    ),
)
```

**Benefits:**
- Protection against DNS spoofing attacks
- Improved security for mail delivery
- Compliance with modern security standards

**Dependencies:** 1.2 (Real DNS MX Lookups)

---

### ✅ 1.3 Typed Error Handling with thiserror
**Priority:** High
**Complexity:** Simple
**Effort:** 1 day
**Status:** ✅ **COMPLETED** (2025-11-09)

**Implementation:**
- ✅ Added new error.rs module with DeliveryError, PermanentError, TemporaryError, SystemError
- ✅ Updated all function signatures to use `Result<T, DeliveryError>`
- ✅ Convert DnsError to DeliveryError via From trait
- ✅ Categorize SMTP response codes (5xx = permanent, 4xx = temporary)
- ✅ Removed anyhow dependency from empath-delivery
- ✅ Updated DNS resolver to return Result<T, String>
- ✅ Fixed clippy warnings (manual range contains)

**Benefits:**
- Clear distinction between permanent, temporary, and system errors
- Pattern matching on specific error types for better retry logic
- Type-safe error propagation throughout the delivery pipeline
- Better error messages and debugging information

**Files Modified:**
- `empath-delivery/src/error.rs` (new, 238 lines)
- `empath-delivery/src/lib.rs`
- `empath-delivery/src/dns.rs`
- `empath-common/src/error.rs` (new, 216 lines)
- `empath-smtp/src/error.rs`
- `empath-smtp/src/client/error.rs`
- `empath-spool/src/error.rs` (new, 182 lines)

**Dependencies:** None

---

### ✅ 1.4 Exponential Backoff for Retries
**Priority:** High
**Complexity:** Simple
**Effort:** 1 day
**Status:** ✅ **COMPLETED** (2025-11-11)

**Implementation:**
- ✅ Configurable exponential backoff with base delay, max delay, and jitter factor
- ✅ Formula: `delay = min(base * 2^(attempts - 1), max_delay) * (1 ± jitter)`
- ✅ Default: 60s base, 86400s (24h) max, 0.2 (±20%) jitter
- ✅ Message expiration configuration (optional, default: never expire)
- ✅ Added `DeliveryInfo.queued_at` and `DeliveryInfo.next_retry_at` timestamps
- ✅ Added `DeliveryStatus::Expired` for expired messages
- ✅ Process queue only retries messages when it's time (respects `next_retry_at`)
- ✅ Comprehensive tests for backoff calculation, jitter, expiration, and retry scheduling
- ✅ Updated `empathctl` CLI to handle `Expired` status

**Configuration Fields:**
```ron
delivery: (
    base_retry_delay_secs: 60,        // Default: 60 seconds (1 minute)
    max_retry_delay_secs: 86400,      // Default: 86400 seconds (24 hours)
    retry_jitter_factor: 0.2,         // Default: 0.2 (±20%)
    message_expiration_secs: 604800,  // Optional: 7 days (default: None)
)
```

**Retry Schedule (with defaults):**
- Attempt 1: ~1 minute (48-72s with jitter)
- Attempt 2: ~2 minutes (96-144s with jitter)
- Attempt 3: ~4 minutes (192-288s with jitter)
- Attempt 4: ~8 minutes
- ...
- Max: 24 hours between attempts

**Files Modified:**
- `empath-delivery/src/lib.rs` (exponential backoff implementation + tests)
- `empath-delivery/Cargo.toml` (added `rand` dependency)
- `empath/bin/empathctl.rs` (added `Expired` status support)
- `empath.config.ron` (documented new configuration options)

**Dependencies:** 1.3 (DeliveryError categorization) ✅

---

### ✅ 1.5 Graceful Shutdown Handling
**Priority:** High
**Complexity:** Medium
**Effort:** 1-2 days
**Status:** ✅ **COMPLETED** (2025-11-11)

**Implementation:**
- ✅ Wait for any in-flight delivery to complete with 30s timeout
- ✅ Persist queue state to disk before exit
- ✅ Track processing state with atomic flag
- ✅ Integrated with existing tokio shutdown signal system
- ✅ Comprehensive logging of shutdown progress
- ✅ Integration tests for graceful shutdown behavior

**Shutdown Behavior:**
When a shutdown signal (SIGTERM/SIGINT) is received:
1. Stop accepting new work (scan/process ticks)
2. Wait for current delivery to complete (max 30 seconds)
3. Save queue state to disk for CLI access
4. Exit cleanly

In-flight deliveries that don't complete within the 30s timeout are marked as pending and will be retried on restart.

**Files Modified:**
- `empath-delivery/src/lib.rs` (graceful shutdown logic + tests)

**Dependencies:** 1.1 (Persistent queue) - ⚠️ Partially implemented (queue state persistence exists, but not full persistent queue backend)

---

## Phase 2: Observability & Operations (Weeks 3-4)

### 🟡 2.1 Structured Metrics Collection
**Priority:** High
**Complexity:** Medium
**Effort:** 2-3 days
**Files:** `empath-delivery/src/metrics.rs` (new)

**Implementation:** Prometheus metrics

**Key Metrics:**
- `empath_delivery_attempts_total{status}` - Counter by success/failure
- `empath_delivery_duration_seconds{domain}` - Histogram
- `empath_delivery_queue_size{status}` - Gauge by status
- `empath_delivery_active_connections{server}` - Gauge
- `empath_smtp_errors_total{code}` - Counter by SMTP code
- `empath_dns_lookup_duration_seconds` - Histogram

**Dependencies:** None

---

### 🟡 2.2 Connection Pooling for SMTP Client
**Priority:** High
**Complexity:** Medium
**Effort:** 2-3 days
**Files:** `empath-delivery/src/connection_pool.rs` (new)

**Current Issue:** Creates new connection for every message

**Benefits:**
- Reuse connections to same MX server
- Support SMTP pipelining
- Reduce TLS handshake overhead
- Better resource utilization

**Configuration:**
```ron
// In empath.config.ron
Empath (
    // ... other config ...
    delivery: (
        connection_pool: (
            max_per_server: 5,
            idle_timeout: "5m",
            connect_timeout: "30s",
        ),
    ),
)
```

**Dependencies:** None

---

### 🟡 2.3 Comprehensive Test Suite
**Priority:** High
**Complexity:** Medium
**Effort:** 2-3 days
**Files:** `empath-delivery/tests/` (new directory)

**Current Issue:** Zero test coverage for 600+ lines of critical code

**Test Categories:**
- **Unit Tests:**
  - Queue operations (enqueue, dequeue, status updates)
  - Retry counting and max attempts
  - Domain extraction from recipients
  - Multi-recipient grouping

- **Integration Tests:**
  - End-to-end delivery flow with mock SMTP server
  - Retry with backoff timing
  - Concurrent queue access
  - Queue persistence and recovery

- **Property Tests:**
  - Domain extraction never panics
  - Queue operations maintain consistency

**Dependencies:** Mock SMTP server (see 4.2)

---

### 🟡 2.4 Health Check Endpoints
**Priority:** High
**Complexity:** Simple
**Effort:** 1 day
**Files:** `empath-delivery/src/health.rs` (new)

**Implementation:**
```rust
pub struct HealthChecker {
    spool_controller: Arc<Controller>,
    delivery_queue: Arc<DeliveryQueue>,
}

// Endpoints:
// GET /health - Basic liveness
// GET /health/ready - Readiness check
// GET /health/detailed - Full status + metrics
```

**Checks:**
- Spool directory accessible
- Queue depth within thresholds
- Delivery success rate
- DNS resolver responsive

**Dependencies:** 2.1 (Metrics)

---

## Phase 3: Advanced Features (Weeks 5-8)

### 🟢 3.1 Parallel Delivery Processing
**Priority:** Medium
**Complexity:** Medium
**Effort:** 2 days
**Files:** `empath-delivery/src/worker.rs` (new)

**Current Issue:** Sequential processing limits throughput

**Implementation:**
```rust
pub struct DeliveryWorkerPool {
    workers: Vec<JoinHandle<()>>,
    semaphore: Arc<Semaphore>,  // Limit concurrency
}
```

**Configuration:**
```ron
// In empath.config.ron
Empath (
    // ... other config ...
    delivery: (
        workers: (
            count: 10,
            max_concurrent: 100,
        ),
    ),
)
```

**Dependencies:** 2.2 (Connection pooling)

---

### ✅ 3.2 Per-Domain Configuration
**Priority:** Medium
**Complexity:** Medium
**Effort:** 2 days
**Status:** ✅ **COMPLETED** (2025-11-09)

**Implementation:**
- ✅ Created `empath-delivery/src/domain_config.rs` (189 lines)
- ✅ Support for MX override (testing)
- ✅ Per-domain TLS certificate validation control
- ✅ Integration with delivery processor
- ✅ Comprehensive test coverage

**Configuration:**
```ron
// In empath.config.ron
delivery: (
    domains: {
        "test.example.com": (
            mx_override: "localhost:1025",
            accept_invalid_certs: true,
        ),
    },
)
```

**Use Cases:**
- Testing (override MX to local SMTP server)
- Compliance (enforce TLS for certain domains)
- Development (accept invalid certificates for test domains)

**Files Modified:**
- `empath-delivery/src/domain_config.rs` (new, 189 lines)
- `empath-delivery/src/lib.rs`
- `empath.config.ron`

**Dependencies:** None

---

### 🟢 3.3 Rate Limiting per Domain
**Priority:** High
**Complexity:** Medium
**Effort:** 2 days
**Files:** `empath-delivery/src/rate_limiter.rs` (new)

**Current Issue:** No rate limiting can lead to:
- Being flagged as spam source
- Blacklisting by recipient servers
- Violating recipient policies (Gmail, etc.)

**Implementation:** Token bucket algorithm per domain

**Configuration:**
```ron
// In empath.config.ron
Empath (
    // ... other config ...
    delivery: (
        rate_limits: (
            global_per_second: 100,
            per_domain_per_second: 10,
        ),
    ),
)
```

**Dependencies:** 3.2 (Per-domain config)

---

### 🟢 3.4 Delivery Status Notifications (DSN)
**Priority:** Medium
**Complexity:** Complex
**Effort:** 3-5 days
**Files:** `empath-delivery/src/bounce.rs` (new)

**Implementation:** RFC 3461/3464 compliance

**Features:**
- Bounce messages for permanent failures (5xx)
- Delay notifications after 4+ hours
- Success notifications (if requested)
- Include original headers and diagnostic info

**Dependencies:** 1.3 (Typed errors)

---

### ✅ 3.5 Queue Management CLI/API
**Priority:** Medium
**Complexity:** Medium
**Effort:** 2-3 days
**Status:** ✅ **COMPLETED** (2025-11-10)

**Implementation:**
- ✅ Comprehensive CLI tool with clap framework
- ✅ Queue state persisted to bincode file (/tmp/spool/queue_state.bin)
- ✅ Atomic queue state updates every 30 seconds
- ✅ Freeze marker file-based pause mechanism
- ✅ Human-readable output with timestamps and age formatting
- ✅ JSON output format support for programmatic access

**Commands:**
```bash
empathctl queue list --status=failed     # List failed messages
empathctl queue view <message-id>        # View message details
empathctl queue retry <message-id>       # Retry failed delivery
empathctl queue delete <message-id> --yes  # Delete message
empathctl queue freeze                   # Pause delivery
empathctl queue unfreeze                 # Resume delivery
empathctl queue stats --watch --interval 2  # Live stats
```

**Features:**
- List messages with optional status filtering
- View detailed message info including envelope and attempt history
- Retry failed/pending messages with force option
- Delete messages from queue and spool with confirmation
- Freeze/unfreeze queue processing
- Real-time statistics with watch mode

**Files Modified:**
- `empath/src/bin/empathctl.rs` (new, 663 lines)
- `empath-delivery/src/lib.rs`
- `empath-spool/src/config.rs`
- `empath-spool/src/controller.rs`
- `CLAUDE.md`

**Dependencies:** None (implemented with bincode serialization)

---

### 🟢 3.6 Audit Logging
**Priority:** Medium
**Complexity:** Simple
**Effort:** 1-2 days
**Files:** `empath-delivery/src/audit.rs` (new)

**Implementation:** JSON Lines format

```json
{
  "timestamp": 1234567890,
  "message_id": {"timestamp": 1234567890, "id": 42},
  "sender": "user@example.com",
  "recipients": ["recipient@example.com"],
  "mx_server": "mx.example.com",
  "outcome": "delivered",
  "smtp_response": "250 OK",
  "duration_ms": 1234,
  "attempt_number": 1
}
```

**Benefits:**
- Compliance and debugging
- Delivery pattern analysis
- Problem domain identification

**Dependencies:** None

---

## Phase 4: Rust-Specific Improvements

### 🟡 4.1 Replace Manual Future Boxing with RPITIT
**Priority:** High
**Complexity:** Low
**Effort:** 1-2 hours
**Files:** `empath-spool/src/spool.rs`

**Current Issue:** Manual `Pin<Box<dyn Future>>` in Spool trait

**Implementation:** Use Return Position Impl Trait in Traits (stable in Rust 2024)
```rust
pub trait Spool: Send + Sync {
    async fn spool_message(&self, message: &Message) -> anyhow::Result<()>;
}
```

**Benefits:**
- Eliminates allocations
- Cleaner API
- Better ergonomics

**Dependencies:** None

---

### 🟡 4.2 Mock SMTP Server for Testing
**Priority:** Medium
**Complexity:** Medium
**Effort:** 2 days
**Files:** `empath-smtp/src/testing/mock_server.rs` (new)

**Implementation:**
```rust
pub struct MockSmtpServer {
    listener: TcpListener,
    behavior: Arc<RwLock<ServerBehavior>>,
}

pub enum ServerBehavior {
    AcceptAll,
    RejectMailFrom,
    RejectRcptTo(String),
    TemporaryError,
    SlowResponse(Duration),
    Disconnect,
}
```

**Benefits:**
- Deterministic tests
- Fast execution (no network)
- Test error conditions safely

**Dependencies:** None

---

### 🟡 4.3 Use DashMap Instead of Arc<RwLock<HashMap>>
**Priority:** Medium
**Complexity:** Low
**Effort:** 1 hour
**Files:** `empath-delivery/src/lib.rs`

**Current Issue:** RwLock serializes all queue access

**Implementation:**
```rust
use dashmap::DashMap;

pub struct DeliveryQueue {
    queue: Arc<DashMap<SpooledMessageId, DeliveryInfo>>,
}
```

**Benefits:**
- Lock-free per-key operations
- Better concurrency under load
- Lower latency for queue operations

**Dependencies:** Add `dashmap` crate dependency

---

### 🟡 4.4 Domain Newtype for Type Safety
**Priority:** Medium
**Complexity:** Low
**Effort:** 2-3 hours
**Files:** `empath-delivery/src/domain.rs` (new)

**Current Issue:** Raw `String` for domains lacks validation

**Implementation:**
```rust
#[derive(Debug, Clone, Hash, Eq, PartialEq, Serialize, Deserialize)]
pub struct Domain(String);

impl Domain {
    pub fn parse(s: &str) -> Result<Self, DomainError> {
        // Validate domain format
        // Prevent empty strings
        // Check length limits
    }
}
```

**Benefits:**
- Prevents invalid domain strings
- API clarity
- Compile-time guarantees

**Dependencies:** None

---

### 🟡 4.5 Structured Concurrency with tokio::task::JoinSet
**Priority:** Medium
**Complexity:** Medium
**Effort:** Half day
**Files:** `empath-delivery/src/lib.rs`

**Current Issue:** No clear task lifecycle management

**Implementation:**
```rust
pub async fn process_queue_parallel(&self, max_concurrent: usize) -> Result<()> {
    let mut join_set = JoinSet::new();
    // Spawn tasks, track completion, handle errors
}
```

**Benefits:**
- Better concurrency control
- Graceful shutdown support
- Structured error aggregation

**Dependencies:** None

---

### 🟡 4.6 Replace u64 Timestamps with SystemTime
**Priority:** Medium
**Complexity:** Medium
**Effort:** Half day
**Files:** Multiple

**Current Issue:** Raw `u64` timestamps error-prone

**Implementation:**
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Timestamp(#[serde(with = "timestamp_serde")] SystemTime);

impl Timestamp {
    pub fn age(&self) -> Duration { /* ... */ }
    pub fn should_retry(&self, interval: Duration) -> bool { /* ... */ }
}
```

**Benefits:**
- Type safety
- Prevents epoch arithmetic bugs
- Better API

**Dependencies:** None

---

## Phase 5: Production Operations

### 🟢 5.1 Circuit Breakers per Domain
**Priority:** Medium
**Complexity:** Medium
**Effort:** 2 days
**Files:** `empath-delivery/src/circuit_breaker.rs` (new)

**Implementation:**
```rust
pub struct CircuitBreaker {
    state: Arc<RwLock<CircuitState>>,
}

enum CircuitState {
    Closed,         // Normal operation
    Open,           // Failing, reject requests
    HalfOpen,       // Testing recovery
}
```

**Configuration:**
```ron
// In empath.config.ron
Empath (
    // ... other config ...
    delivery: (
        circuit_breaker: (
            failure_threshold: 5,      // Open after 5 failures
            timeout: "60s",            // Stay open for 60s
            half_open_max_calls: 3,    // Test with 3 calls
        ),
    ),
)
```

**Benefits:**
- Prevents cascading failures
- Automatic recovery testing
- Resource protection

**Dependencies:** None

---

### 🟢 5.2 Configuration Hot Reload
**Priority:** Medium
**Complexity:** Medium
**Effort:** 1 day
**Files:** `empath-delivery/src/config.rs`

**Current Issue:** Restart required for config changes

**Implementation:** Watch config file with `notify` crate

**Benefits:**
- Zero-downtime updates
- Faster debugging iteration
- Dynamic rate limit adjustment

**Dependencies:** None

---

### 🟢 5.3 TLS Policy Enforcement
**Priority:** Medium
**Complexity:** Simple
**Effort:** 1 day
**Files:** `empath-delivery/src/tls.rs` (new)

**Implementation:**
```rust
pub enum TlsPolicy {
    Opportunistic,  // Use if available
    Required,       // Fail if unavailable
    Disabled,       // Plain only
}
```

**Configuration:**
```ron
// In empath.config.ron
Empath (
    // ... other config ...
    delivery: (
        tls: (
            default_policy: "opportunistic",
            domain_overrides: [
                (
                    domains: ["secure.example.com"],
                    policy: "required",
                ),
            ],
        ),
    ),
)
```

**Dependencies:** 3.2 (Per-domain config)

---

### 🟢 5.4 Enhanced Tracing with Spans
**Priority:** Medium
**Complexity:** Medium
**Effort:** 1 day
**Files:** Multiple

**Current Issue:** Basic tracing, no request context

**Implementation:**
```rust
#[traced(instrument(
    skip(self, message),
    fields(
        message_id = %message.id,
        domain = %recipient_info.domain,
        attempt = attempt_count,
    )
))]
async fn deliver_message(&self, ...) -> Result<()> {
    // Automatic context propagation
}
```

**Benefits:**
- Distributed tracing support
- Better debugging with context
- OpenTelemetry integration

**Dependencies:** None

---

## Phase 6: Advanced Optimizations (Low Priority)

### 🔵 6.1 Message Data Streaming for Large Messages
**Priority:** Low
**Complexity:** Complex
**Effort:** 3-5 days

**Current Issue:** Loads entire message into memory

**Implementation:** Stream large messages to reduce memory footprint

**Benefits:**
- Handle multi-GB messages
- Reduced memory pressure

**Dependencies:** None

---

### 🔵 6.2 DKIM Signing Support
**Priority:** Low
**Complexity:** Very Complex
**Effort:** 5-7 days

**Implementation:** RFC 6376 compliance

**Dependencies:** None

---

### 🔵 6.3 Priority Queuing
**Priority:** Low
**Complexity:** Medium
**Effort:** 2-3 days

**Implementation:** Multi-level queue (Critical, High, Normal, Low)

**Use Cases:**
- Transactional emails (password resets) - Critical
- Bulk/marketing - Low

**Dependencies:** 1.1 (Queue abstraction)

---

### 🔵 6.4 Batch Processing and SMTP Pipelining
**Priority:** Low
**Complexity:** Medium
**Effort:** 2-3 days

**Implementation:** Group messages by MX server, reuse connections

**Benefits:**
- Reduced connection overhead
- Higher throughput

**Dependencies:** 2.2 (Connection pooling)

---

### 🔵 6.5 Delivery Strategy Pattern
**Priority:** Low
**Complexity:** Complex
**Effort:** 5-7 days

**Implementation:** Plugin-based delivery strategies

**Strategies:**
- SMTP (current)
- HTTP webhooks
- LMTP (local delivery)
- Custom routing logic

**Dependencies:** 1.1 (Trait abstractions)

---

### 🔵 6.6 Message Deduplication
**Priority:** Low
**Complexity:** Simple
**Effort:** 1 day

**Implementation:** LRU cache tracking recent deliveries

**Benefits:**
- Prevents double delivery during retries
- Better user experience

**Dependencies:** None

---

### 🔵 6.7 Property-Based Testing with proptest
**Priority:** Low
**Complexity:** Medium
**Effort:** 2 days

**Implementation:**
```rust
proptest! {
    #[test]
    fn domain_extraction_never_panics(email in ".*@.*") {
        let _ = extract_domain(&email);
    }
}
```

**Benefits:**
- Finds edge cases
- Fuzzing-like testing

**Dependencies:** None

---

### 🔵 6.8 Load Testing Framework
**Priority:** Low
**Complexity:** Medium
**Effort:** 2-3 days

**Target Metrics:**
- 1000 messages/minute sustained
- < 100ms p95 delivery latency
- < 500MB memory for 10k queued messages

**Dependencies:** 4.2 (Mock SMTP server)

---

### ✅ 6.9 Benchmarks with criterion
**Priority:** Low
**Complexity:** Simple
**Effort:** 1 day
**Status:** ✅ **COMPLETED** (2025-11-08)

**Implementation:**
- ✅ Added Criterion.rs 0.5 as dev dependency
- ✅ Configured benchmark profile with debug info for profiling
- ✅ Comprehensive SMTP benchmarks (command parsing, FSM transitions, context operations)
- ✅ Comprehensive spool benchmarks (message creation, bincode serialization, ULID operations)
- ✅ HTML reports at target/criterion/report/index.html
- ✅ All benchmarks pass strict clippy checks

**SMTP Benchmarks:**
- Command parsing (HELO, MAIL FROM, RCPT TO, etc.)
- ESMTP parameter parsing with perfect hash map
- FSM state transitions
- Full SMTP transaction sequences
- Context creation and initialization

**Spool Benchmarks:**
- Message creation and builder pattern
- Bincode serialization/deserialization (1KB - 1MB)
- ULID generation and parsing
- In-memory spool operations (write, read, list, delete)
- Full message lifecycle

**Files Modified:**
- `empath-smtp/benches/smtp_benchmarks.rs` (new, 374 lines)
- `empath-spool/benches/spool_benchmarks.rs` (new, 386 lines)
- `empath-smtp/Cargo.toml`
- `empath-spool/Cargo.toml`
- `Cargo.toml` (workspace config)
- `CLAUDE.md` (benchmarking documentation)

**Benefits:**
- Quantify performance improvements (e.g., 80% clone reduction tracked)
- Prevent regressions with baseline comparisons
- Identify performance hotspots

**Dependencies:** None

---

## Documentation Improvements

### 📚 API Documentation
**Priority:** Medium
**Effort:** 2-3 days

- Add comprehensive rustdoc comments
- Include usage examples
- Document error conditions
- Create architecture diagrams
- Publish to docs.rs

---

### 📚 Operational Runbook
**Priority:** Medium
**Effort:** 2 days

Create `OPERATIONS.md` with:
- Deployment procedures
- Configuration guide
- Monitoring setup
- Troubleshooting scenarios
- Performance tuning
- Backup/recovery procedures

---

### 📚 Integration Examples
**Priority:** Low
**Effort:** 2-3 days

- Embedding empath in another app
- Custom delivery pipeline
- Prometheus metrics integration
- Custom queue backend

---

## Summary by Phase

### Phase 0: Code Review Follow-ups ✅ **MOSTLY COMPLETED**
**Code Quality & Security:**
- ✅ TLS certificate validation (0.1) - **COMPLETED**
- ✅ SMTP operation timeouts (0.2) - **COMPLETED**
- Context/Message layer violation (0.3) - **TODO**
- ✅ String cloning optimization (0.4) - **COMPLETED**
- DNS cache mutex contention (0.5) - **TODO**
- NoVerifier compile-time guard (0.6) - **TODO**
- ✅ Extract SmtpTransaction (0.7) - **COMPLETED**
- Spool deletion retry (0.8) - **TODO**
- DNSSEC validation (0.9) - **TODO**
- MX randomization (0.10) - **TODO**
- Security documentation (0.11) - **TODO**
- Deployment guide (0.12) - **TODO**
- Integration test suite (0.13) - **TODO**
- Delivery strategy pattern (0.14) - **TODO**

**Progress: 4/14 completed (29%)**

### Phase 1: Production Foundation (2-3 weeks)
**Must-Have for Deployment:**
1. Persistent delivery queue (1.1) - **TODO**
2. ✅ Real DNS MX lookups (1.2) - **COMPLETED**
3. ✅ Typed error handling (1.3) - **COMPLETED**
4. ✅ Exponential backoff (1.4) - **COMPLETED**
5. ✅ Graceful shutdown (1.5) - **COMPLETED**

**Progress: 4/5 completed (80%)**

### Phase 2: Observability (2-3 weeks)
**Operational Readiness:**
1. Structured metrics (2.1)
2. Connection pooling (2.2)
3. Comprehensive tests (2.3)
4. Health checks (2.4)

### Phase 3: Advanced Features (4-6 weeks)
**Production Excellence:**
- Parallel processing (3.1) - **TODO**
- ✅ Per-domain config (3.2) - **COMPLETED**
- Rate limiting (3.3) - **TODO**
- DSN/bounces (3.4) - **TODO**
- ✅ Queue management (3.5) - **COMPLETED**
- Audit logging (3.6) - **TODO**

**Progress: 2/6 completed (33%)**

### Phase 4: Rust Improvements (2-3 weeks)
**Code Quality:**
- RPITIT refactoring (4.1)
- Mock SMTP server (4.2)
- DashMap migration (4.3)
- Domain newtype (4.4)
- Structured concurrency (4.5)

### Phase 5: Operations (2-3 weeks)
**Reliability:**
- Circuit breakers (5.1)
- Config hot reload (5.2)
- TLS enforcement (5.3)
- Enhanced tracing (5.4)

### Phase 6: Optimizations (Backlog)
**Future Enhancements:**
- All items in section 6.x
- ✅ Benchmarks with criterion (6.9) - **COMPLETED**

**Progress: 1/9 completed (11%)**

---

## Total Estimated Effort

- **Critical (Production Ready):** 2-3 weeks (1-2 developers)
- **High Priority:** 2-3 weeks
- **Medium Priority:** 6-8 weeks
- **Low Priority:** 8-12 weeks

**Full implementation:** 4-6 months with 1-2 developers

---

## Configuration Example (Future State)

```ron
// empath.config.ron - Full configuration with all future features
Empath (
    spool: (
        path: "/var/spool/empath",
    ),
    delivery: (
        // Queue backend
        queue_backend: "file",  // or "sqlite", "postgres"
        queue_path: "/var/lib/empath/queue",

        // Retry configuration
        max_attempts: 25,
        retry_base_delay: "30s",
        retry_max_delay: "24h",
        retry_multiplier: 2.0,
        retry_jitter_factor: 0.2,

        // Concurrency
        max_parallel_deliveries: 100,
        max_connections_per_host: 5,

        // DNS
        dns: (
            cache_ttl: "5m",
            timeout: "10s",
        ),

        // TLS
        tls: (
            default_policy: "opportunistic",
        ),

        // Rate limiting
        rate_limits: (
            default: 100,  // per minute per domain
        ),

        // Per-domain configuration
        domains: {
            "gmail.com": (
                max_connections: 10,
                rate_limit: 200,
                require_tls: true,
            ),
            "example.com": (
                mx_override: "localhost:1025",  // For testing
            ),
        },
    ),
    metrics: (
        enabled: true,
        listen_addr: "127.0.0.1:9090",
    ),
    health: (
        enabled: true,
        listen_addr: "127.0.0.1:8080",
    ),
    audit: (
        enabled: true,
        log_path: "/var/log/empath/audit.jsonl",
        rotate_daily: true,
    ),
)
```

---

## Getting Started

**Recent Progress (Completed):**
- ✅ Graceful shutdown handling (1.5)
- ✅ Real DNS MX lookups (1.2)
- ✅ Typed error handling with thiserror (1.3)
- ✅ Per-domain configuration (3.2)
- ✅ Queue management CLI (3.5)
- ✅ SMTP operation timeouts (0.2)
- ✅ String cloning optimization (0.4)
- ✅ SMTP transaction refactoring (0.7)
- ✅ Comprehensive benchmarking (6.9)

**Immediate Next Steps** (highest ROI, ~3-4 days remaining for Phase 1):
1. Implement persistent queue (1.1) - 2-3 days
2. Implement exponential backoff (1.4) - 1 day

**After Phase 1** (remaining critical items):
3. Fix Context/Message layer violation (0.3) - 1 day
4. Add spool deletion retry mechanism (0.8) - 2 hours

After completing Phase 1, the system will be production-ready for basic mail delivery.

---

**Last Updated:** 2025-11-11 (graceful shutdown implemented)
**Contributors:** code-reviewer, architect-review, rust-expert, refactoring-specialist agents
**Code Review:** See CODE_REVIEW_2025-11-10.md for comprehensive analysis
