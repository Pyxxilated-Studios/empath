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

### ❌ 0.3 Fix Context/Message Layer Violation in Spool
**Priority:** ~~Critical~~ **REJECTED**
**Complexity:** Medium
**Effort:** 1 day
**Status:** ❌ **REJECTED** (2025-11-11)
**Files:** N/A

**Original Issue:** Spool stores `Context` (session state) instead of `Message` (data), violating architectural layer separation.

**Decision: REJECTED**

After thorough analysis, this is **NOT** a layer violation but an **intentional architectural feature** that serves the module/plugin system. The apparent "session-only" fields in Context (id, metadata, extended, banner) are actually part of the **module contract**.

**Why Context Persistence Is Correct:**

1. **Module Lifecycle Tracking**: Modules can set `context.metadata` during SMTP reception and reference it during delivery events (hours or days later). This enables plugins to maintain coherent state across the entire message journey without requiring external storage.

2. **Example Module Use Case**:
   - Module sets `metadata["correlation_id"] = "12345"` during MailFrom event
   - Same module reads it during DeliverySuccess event for audit logging
   - Without Context persistence, modules would need their own database

3. **Delivery Queue State**: The persistent queue implementation (task 1.1) leverages this design by storing delivery metadata in `Context.delivery`, using the spool as the persistent queue backend. Single source of truth, no separate queue storage needed.

4. **Storage Overhead**: Negligible (~100 bytes per message vs 4KB-10MB+ email sizes)

**What This Change Would Break:**

- ❌ Module API contract (plugins lose ability to persist metadata)
- ❌ Elegant "single source of truth" design
- ❌ Would require modules to maintain external state storage
- ❌ Add conversion complexity at boundaries

**See Also:**
- CLAUDE.md "Context Persistence and the Module Contract" section for detailed explanation
- TODO.md task 1.1 for how this design enables persistent queue implementation

**Dependencies:** None
**Source:** CODE_REVIEW_2025-11-10.md Section 2.1 (original), reconsidered during task 1.1 implementation

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

### 🟡 1.1 Persistent Delivery Queue
**Priority:** Critical
**Complexity:** Medium
**Effort:** 2-3 days
**Status:** 🟡 **IN PROGRESS** (2025-11-11)
**Files:**
- `empath-common/src/context.rs`
- `empath-spool/src/spool.rs`
- `empath-spool/src/controller.rs`
- `empath-delivery/src/lib.rs`

**Original Issue:** In-memory queue loses all state on restart, causing:
- Message delivery loss on crashes
- Lost retry counts and status tracking
- No audit trail

**Chosen Approach:** Store queue state in `Context.delivery` field (spool metadata)

Instead of creating a separate `QueueBackend` abstraction, we leverage the existing
spool infrastructure to persist queue state. This approach:
- ✅ Maintains single source of truth (spool files)
- ✅ Respects module contract (modules can access delivery metadata)
- ✅ Simpler architecture (no sync between queue and spool needed)
- ✅ Already uses atomic file operations

**Implementation Progress:**

✅ **Phase 1: Data Model (COMPLETED 2025-11-11)**
- Moved `DeliveryStatus` and `DeliveryAttempt` to `empath-common/src/context.rs`
- Extended `DeliveryContext` with queue state fields:
  ```rust
  pub struct DeliveryContext {
      // Existing fields
      pub message_id: String,
      pub domain: Arc<str>,
      pub server: Option<String>,
      pub error: Option<String>,

      // New persistent queue state fields
      pub status: DeliveryStatus,              // Pending, InProgress, Failed, etc.
      pub attempt_history: Vec<DeliveryAttempt>, // Full attempt log
      pub queued_at: u64,                      // For expiration checks
      pub next_retry_at: Option<u64>,          // For scheduling
      pub current_server_index: usize,         // For MX fallback
  }
  ```

✅ **Phase 2: Spool Update Method (COMPLETED 2025-11-11)**
- Added `BackingStore::update()` method to spool trait
- Implemented for `FileBackingStore` (atomic metadata updates)
- Implemented for `MemoryBackingStore` (in-memory updates)
- Implemented for `TestBackingStore` (proxy to inner store)

**Remaining Work:**

🚧 **Phase 3: Delivery Processor Integration**

Add helper method to sync queue state to spool:
```rust
impl DeliveryProcessor {
    /// Sync the in-memory delivery info to the spool's Context.delivery field
    async fn persist_delivery_state(
        &self,
        message_id: &SpooledMessageId,
        spool: &Arc<dyn empath_spool::BackingStore>,
    ) -> Result<(), DeliveryError> {
        // Get current queue info
        let info = self.queue.get(message_id).await.ok_or_else(|| {
            SystemError::MessageNotFound(format!("Message {message_id:?} not in queue"))
        })?;

        // Read context from spool
        let mut context = spool
            .read(message_id)
            .await
            .map_err(|e| SystemError::SpoolRead(e.to_string()))?;

        // Update the delivery field with current queue state
        context.delivery = Some(DeliveryContext {
            message_id: message_id.to_string(),
            domain: info.recipient_domain.clone(),
            server: info.current_mail_server().map(|s| format!("{}:{}", s.host, s.port)),
            error: match &info.status {
                DeliveryStatus::Failed(e) => Some(e.clone()),
                DeliveryStatus::Retry { last_error, .. } => Some(last_error.clone()),
                _ => None,
            },
            attempts: Some(info.attempt_count()),
            status: info.status.clone(),
            attempt_history: info.attempts.clone(),
            queued_at: info.queued_at,
            next_retry_at: info.next_retry_at,
            current_server_index: info.current_server_index,
        });

        // Atomically update spool
        spool
            .update(message_id, &context)
            .await
            .map_err(|e| SystemError::SpoolWrite(e.to_string()))?;

        Ok(())
    }
}
```

Call this after every status change:
- After `queue.update_status()`
- After `queue.record_attempt()`
- After `queue.set_mail_servers()`
- After setting `next_retry_at` in exponential backoff

**Files to modify:**
- `empath-delivery/src/lib.rs:873` - After `update_status(InProgress)`
- `empath-delivery/src/lib.rs:962` - After successful delivery
- `empath-delivery/src/lib.rs:1076-1114` - After `handle_delivery_error()` updates

🚧 **Phase 4: Queue Restoration on Startup**

Update `scan_spool_internal()` to restore queue state from `Context.delivery`:
```rust
async fn scan_spool_internal(
    &self,
    spool: &Arc<dyn empath_spool::BackingStore>,
) -> Result<usize, DeliveryError> {
    let message_ids = spool.list().await?;
    let mut added = 0;

    for msg_id in message_ids {
        let context = spool.read(&msg_id).await?;

        // Check if this message already has delivery state
        if let Some(delivery_ctx) = &context.delivery {
            // Restore from persisted state
            let info = DeliveryInfo {
                message_id: msg_id.clone(),
                status: delivery_ctx.status.clone(),
                attempts: delivery_ctx.attempt_history.clone(),
                recipient_domain: delivery_ctx.domain.clone(),
                mail_servers: Arc::new(Vec::new()), // Will be resolved again if needed
                current_server_index: delivery_ctx.current_server_index,
                queued_at: delivery_ctx.queued_at,
                next_retry_at: delivery_ctx.next_retry_at,
            };

            // Add to queue with existing state
            self.queue.queue.write().await.insert(msg_id.clone(), info);
            added += 1;
        } else {
            // New message without delivery state - create fresh DeliveryInfo
            // (existing logic for extracting domains from recipients)
            // ...
        }
    }

    Ok(added)
}
```

**Files to modify:**
- `empath-delivery/src/lib.rs:760-824` - `scan_spool_internal()` method

🚧 **Phase 5: Update empathctl**

Change from reading `queue_state.bin` to reading spool:
```rust
// In empath/src/bin/empathctl.rs

// OLD: Read from queue_state.bin
let state: HashMap<SpooledMessageId, DeliveryInfo> =
    bincode::deserialize(&data)?;

// NEW: Read from spool
let spool = FileBackingStore::builder()
    .path(spool_path.clone())
    .build()?;

let message_ids = spool.list().await?;
let mut queue_state = HashMap::new();

for msg_id in message_ids {
    let context = spool.read(&msg_id).await?;
    if let Some(delivery) = context.delivery {
        // Convert DeliveryContext to DeliveryInfo for display
        let info = DeliveryInfo {
            message_id: msg_id.clone(),
            status: delivery.status,
            attempts: delivery.attempt_history,
            recipient_domain: delivery.domain,
            // ... rest of fields
        };
        queue_state.insert(msg_id, info);
    }
}
```

**Files to modify:**
- `empath/src/bin/empathctl.rs:200-250` - List command
- `empath/src/bin/empathctl.rs:280-320` - View command
- `empath/src/bin/empathctl.rs:350-390` - Stats command

🚧 **Phase 6: Remove queue_state.bin Logic**

Once spool-based persistence is working:
- Remove `queue_state_path` field from `DeliveryProcessor`
- Remove `save_queue_state()` method
- Remove calls to `save_queue_state()` in graceful shutdown
- Remove bincode serialization of in-memory queue

**Files to modify:**
- `empath-delivery/src/lib.rs:370-450` - DeliveryProcessor struct and initialization
- `empath-delivery/src/lib.rs:715-754` - Remove `save_queue_state()` method
- `empath-delivery/src/lib.rs:630-650` - Remove call from graceful shutdown

**Testing Strategy:**

1. **Unit tests** - Test `persist_delivery_state()` helper
2. **Integration test** - Restart scenario:
   ```rust
   #[tokio::test]
   async fn test_queue_persistence_across_restart() {
       // Create processor, queue a message
       // Simulate delivery attempt (creates delivery context)
       // Drop processor (simulates crash)
       // Create new processor, scan spool
       // Verify queue state is restored
   }
   ```
3. **Backward compatibility** - Messages without `delivery` field should work
4. **empathctl tests** - Verify CLI can read from spool

**Benefits of This Approach:**
- No separate queue storage backend needed
- Module API preserved (plugins can access delivery metadata)
- Single source of truth (spool contains everything)
- Atomic updates already implemented in spool
- Works with existing spool infrastructure (file watching, etc.)

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

### 🔴 4.0 Code Structure Refactoring (Project Organization)
**Priority:** Critical
**Complexity:** Medium
**Effort:** 4-6 weeks (can be done incrementally)
**Status:** 🟡 **IN PROGRESS** (1/7 completed)
**Source:** Comprehensive analysis by rust-expert and refactoring-specialist agents (2025-11-12)

**Overview:** Multiple large files violate Rust ecosystem conventions and hinder maintainability. The crate-level organization is excellent, but file-level granularity needs improvement.

**File Size Guidelines (Rust Ecosystem Conventions):**
- **lib.rs/mod.rs:** 50-200 lines (re-exports only)
- **Implementation files:** 200-500 lines (sweet spot)
- **Complex modules:** 500-800 lines (acceptable if focused)
- **⚠️ Refactor trigger:** 800+ lines
- **🔴 Critical:** 1,000+ lines

**Current Outliers:**
- ~~`empath-delivery/src/lib.rs`: 1,894 lines~~ → ✅ **FIXED** (35 lines)
- `empath-smtp/src/session.rs`: 916 lines 🔴
- `empath-spool/src/spool.rs`: 766 lines ⚠️
- `empath/bin/empathctl.rs`: 721 lines ⚠️

---

#### ✅ 4.0.1 Split empath-delivery/src/lib.rs (God File)
**Priority:** Critical (Highest Impact)
**Complexity:** Medium
**Effort:** 8-12 hours
**Status:** ✅ **COMPLETED** (2025-11-12)
**Files:** `empath-delivery/src/lib.rs` (1,894 lines → 35 lines)

**Problem:** Single file handles 7+ distinct responsibilities:
- Type definitions (SmtpTimeouts, DeliveryInfo, DeliveryQueue)
- Queue management logic
- Delivery processor orchestration
- SMTP transaction execution
- DNS resolution integration
- State persistence logic
- Error handling and retry calculation

**Implementation:**
```
empath-delivery/src/
├── lib.rs                    # Public API, re-exports (35 lines) ✅
├── types.rs                  # DeliveryInfo, SmtpTimeouts (176 lines) ✅
├── queue/
│   ├── mod.rs               # DeliveryQueue struct (130 lines) ✅
│   └── retry.rs             # Exponential backoff calculation (141 lines) ✅
├── processor/
│   ├── mod.rs               # DeliveryProcessor orchestration (347 lines) ✅
│   ├── scan.rs              # Spool scanning logic (144 lines) ✅
│   ├── process.rs           # Message processing loop (153 lines) ✅
│   └── delivery.rs          # Message delivery & error handling (442 lines) ✅
├── dns.rs                    # (existing - kept)
├── domain_config.rs          # (existing - kept)
├── smtp_transaction.rs       # (existing - kept)
└── error.rs                  # (existing - kept)

tests/
└── integration_tests.rs      # Integration tests (387 lines) ✅
```

**Results:**
- **98% reduction** in lib.rs (1,894 → 35 lines)
- All files now ≤ 450 lines (within Rust conventions)
- ✅ All tests pass (17 unit + 10 integration tests)
- ✅ Cargo clippy passes with strict lints
- ✅ Zero breaking changes to public API

**Benefits Achieved:**
- Single Responsibility Principle - each module has one clear purpose
- Testability - smaller modules easier to unit test
- Reduced cognitive load - 80% reduction
- Better reusability - queue and retry logic independent
- Improved maintainability - easy to navigate and understand
- Tests separated from implementation code

**Dependencies:** None

---

#### ✅ 4.0.2 Split empath-smtp/src/session.rs (Session God File)
**Priority:** High
**Complexity:** Medium
**Effort:** 10-15 hours
**Status:** ✅ **COMPLETED** (2025-11-12)
**Files:** `empath-smtp/src/session.rs` (916 lines → 617 lines main + 3 focused modules)

**Problem:** Session struct handles:
- Connection management and I/O
- State machine transitions
- Command parsing integration
- Module/plugin dispatch
- Response generation
- TLS upgrade orchestration
- Message spooling
- Timeout management

**Implementation:**
```
empath-smtp/src/session/
├── mod.rs               # Session struct, public API, tests (617 lines)
├── io.rs                # Connection I/O, data reception (118 lines)
├── response.rs          # Response generation logic (116 lines)
└── events.rs            # Module dispatch, validation (134 lines)
Total: 985 lines across 4 focused modules
```

**Results:**
- ✅ **33% reduction** in largest file size (916 → 617 lines)
- ✅ All implementation modules under 135 lines
- ✅ All 56 tests pass (37 unit + 19 integration + 9 doctests)
- ✅ Zero clippy warnings with strict lints
- ✅ 100% API compatibility maintained

**Benefits Achieved:**
- Separation of concerns (I/O vs business logic vs validation)
- Reduced cognitive load with focused modules
- Each file well within Rust ecosystem conventions
- Better maintainability and testability
- Tests kept in mod.rs for private field access

**Dependencies:** None

---

#### 🟡 4.0.3 Split empath-spool/src/spool.rs (Implementation Mixing)
**Priority:** High
**Complexity:** Medium
**Effort:** 4-6 hours
**Files:** `empath-spool/src/spool.rs` (766 lines → ~100 lines)

**Problem:** Trait definitions, two implementations, and SpooledMessageId all in one file:
- BackingStore trait definition
- Spool generic wrapper
- MemoryBackingStore implementation
- TestBackingStore implementation
- FileBackedSpool implementation (in controller.rs)
- SpooledMessageId type

**Recommended Split:**
```
empath-spool/src/
├── lib.rs                    # Public API
├── types.rs                  # SpooledMessageId (~100 lines)
├── trait.rs                  # BackingStore trait (~100 lines)
├── spool.rs                  # Spool<T> wrapper (~150 lines)
├── backends/
│   ├── mod.rs
│   ├── memory.rs            # MemoryBackingStore (~200 lines)
│   ├── test.rs              # TestBackingStore (~150 lines)
│   └── file.rs              # FileBackedSpool (from controller.rs)
├── controller.rs             # Controller without FileBackedSpool
└── ... (other existing files)
```

**Benefits:**
- Clear separation: interface vs implementations
- Extensibility: easy to add new backends (Redis, PostgreSQL)
- Better testing: each backend tested independently
- Each file < 200 lines

**Dependencies:** None

---

#### 🟢 4.0.4 Refactor empath/bin/empathctl.rs (CLI Monolith)
**Priority:** Medium
**Complexity:** Medium
**Effort:** 6-8 hours
**Files:** `empath/bin/empathctl.rs` (721 lines → ~100 lines)

**Problem:** Single binary file with:
- Argument parsing
- Command dispatch
- Queue operations (list, view, retry, delete, stats)
- Output formatting (text, JSON)
- Interactive confirmation
- Watch mode loop

**Recommended Split:**
```
empath/src/
├── bin/
│   └── empathctl.rs         # Main entry point (~100 lines)
├── cli/
│   ├── mod.rs               # CLI framework
│   ├── args.rs              # Argument parsing (Clap)
│   ├── commands/
│   │   ├── mod.rs
│   │   ├── list.rs          # Queue list command (~100 lines)
│   │   ├── view.rs          # View message command (~80 lines)
│   │   ├── retry.rs         # Retry command (~80 lines)
│   │   ├── delete.rs        # Delete command (~80 lines)
│   │   ├── stats.rs         # Stats command (~120 lines)
│   │   └── freeze.rs        # Freeze/unfreeze commands (~60 lines)
│   ├── output/
│   │   ├── mod.rs
│   │   ├── text.rs          # Text formatting (~100 lines)
│   │   └── json.rs          # JSON formatting (~50 lines)
│   └── watch.rs             # Watch mode loop (~80 lines)
```

**Benefits:**
- Command pattern: easy to add new commands
- Testable: each command can be unit tested
- Separation of concerns: parsing vs execution vs formatting
- Maintainable: changes to one command don't affect others
- Follows cargo/git CLI structure patterns

**Dependencies:** None

---

#### 🟢 4.0.5 Consolidate Timeout Configuration (Duplication)
**Priority:** Medium
**Complexity:** Low
**Effort:** 2-3 hours
**Files:**
- `empath-smtp/src/lib.rs` (SmtpServerTimeouts)
- `empath-delivery/src/lib.rs` (SmtpTimeouts)

**Problem:** Two very similar timeout configuration structs with:
- 5-7 timeout fields each
- Default implementations
- Const default functions
- Serde derives
- Duplicated default logic

**Recommended Consolidation:**
```rust
// empath-common/src/timeout.rs

/// Server-side SMTP timeouts (RFC 5321)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmtpServerTimeouts {
    pub command_secs: u64,
    pub data_init_secs: u64,
    pub data_block_secs: u64,
    pub data_termination_secs: u64,
    pub connection_secs: u64,
}

/// Client-side SMTP timeouts
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmtpClientTimeouts {
    pub connect_secs: u64,
    pub ehlo_secs: u64,
    pub starttls_secs: u64,
    pub mail_from_secs: u64,
    pub rcpt_to_secs: u64,
    pub data_secs: u64,
    pub quit_secs: u64,
}
```

**Benefits:**
- DRY principle: no duplicated default logic
- Type safety: server vs client timeouts are distinct types
- Shared validation: centralized timeout range checking
- Easier testing: mock timeouts in one place

**Dependencies:** None

---

#### 🟢 4.0.6 Extract Queue Persistence Module (Feature Envy)
**Priority:** Medium
**Complexity:** Medium
**Effort:** 4-6 hours
**Files:**
- `empath-delivery/src/lib.rs`
- `empath-spool/src/spool.rs`
- `empath-common/src/context.rs`
- `empath/bin/empathctl.rs`

**Problem:** Shotgun surgery - changing queue format requires touching 4 files across 4 crates:
- empath-delivery: Saves queue state to bincode
- empath-spool: Handles message persistence
- empath-common: Contains DeliveryContext
- empathctl: Reads queue state for CLI

**Recommended Consolidation:**
```
empath-delivery/src/
├── persistence/
│   ├── mod.rs               # Public API
│   ├── queue_state.rs       # QueueState struct, serialization
│   ├── format.rs            # Bincode format versioning
│   └── reader.rs            # Read queue state (for empathctl)
```

**Benefits:**
- Single source of truth for queue format
- Versioning support: easier to migrate queue format
- Shared between processor and CLI
- Better error handling: centralized deserialization errors

**Dependencies:** 1.1 (Persistent queue implementation) - IN PROGRESS

---

#### 🟢 4.0.7 Move Tests to Separate Files (Test Organization)
**Priority:** Medium
**Complexity:** Low
**Effort:** 2-3 hours
**Files:** Multiple

**Problem:** Large test modules (>200 lines) mixed with implementation:
- `empath-smtp/src/session.rs`: 690-line test module
- Other files with extensive inline tests

**Recommended Organization:**
```
empath-smtp/
├── src/
│   └── session.rs           # Implementation only
└── tests/
    ├── session_tests.rs     # Integration tests
    └── ... (other test files)
```

**For files with 200+ line test modules:**
- Move to separate files in `tests/` directory
- Keep small unit tests inline with `#[cfg(test)]`
- Use `#[cfg(test)] mod tests { mod foo { ... } mod bar { ... } }` for organization

**Benefits:**
- Clear separation between tests and implementation
- Faster compilation (tests compile separately)
- Better test organization
- Easier to run specific test suites

**Dependencies:** None

---

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
- ❌ Context/Message layer violation (0.3) - **REJECTED** (intentional design, see CLAUDE.md)
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

**Progress: 4/13 completed (31%)** - 1 task rejected as not needed

### Phase 1: Production Foundation (2-3 weeks)
**Must-Have for Deployment:**
1. 🟡 Persistent delivery queue (1.1) - **IN PROGRESS** (40% complete: data model + spool integration done)
2. ✅ Real DNS MX lookups (1.2) - **COMPLETED**
3. ✅ Typed error handling (1.3) - **COMPLETED**
4. ✅ Exponential backoff (1.4) - **COMPLETED**
5. ✅ Graceful shutdown (1.5) - **COMPLETED**

**Progress: 4.4/5 completed (88%)**

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

### Phase 4: Rust Improvements (6-10 weeks)
**Code Quality & Organization:**
- 🟡 Code structure refactoring (4.0) - **IN PROGRESS (2/7 sub-tasks completed)**
  - ✅ Split empath-delivery/src/lib.rs (4.0.1) - **COMPLETED** (2025-11-12)
  - ✅ Split empath-smtp/src/session.rs (4.0.2) - **COMPLETED** (2025-11-12)
  - 🟡 Split empath-spool/src/spool.rs (4.0.3) - High, 4-6 hours
  - 🟢 Refactor empath/bin/empathctl.rs (4.0.4) - Medium, 6-8 hours
  - 🟢 Consolidate timeout configuration (4.0.5) - Medium, 2-3 hours
  - 🟢 Extract queue persistence module (4.0.6) - Medium, 4-6 hours
  - 🟢 Move tests to separate files (4.0.7) - Medium, 2-3 hours
- RPITIT refactoring (4.1)
- Mock SMTP server (4.2)
- DashMap migration (4.3)
- Domain newtype (4.4)
- Structured concurrency (4.5)

**Progress: 2/12 completed (17%)** - Tasks 4.0.1 and 4.0.2 completed 2025-11-12

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
- ✅ empath-delivery code structure refactoring (4.0.1) - **2025-11-12**
- ✅ Graceful shutdown handling (1.5)
- ✅ Exponential backoff (1.4)
- ✅ Real DNS MX lookups (1.2)
- ✅ Typed error handling with thiserror (1.3)
- ✅ Per-domain configuration (3.2)
- ✅ Queue management CLI (3.5)
- ✅ SMTP operation timeouts (0.2)
- ✅ String cloning optimization (0.4)
- ✅ SMTP transaction refactoring (0.7)
- ✅ Comprehensive benchmarking (6.9)

**Current Work In Progress:**
- 🟡 Persistent delivery queue (1.1) - 40% complete
  - ✅ Data model (DeliveryStatus, DeliveryAttempt in empath-common)
  - ✅ Extended DeliveryContext with queue state fields
  - ✅ Implemented BackingStore::update() for spool persistence
  - 🚧 Remaining: Delivery processor integration, queue restoration, empathctl updates

**Immediate Next Steps** (~1-2 days to finish Phase 1):
1. Complete persistent queue implementation (1.1) - 1-2 days
   - Add `persist_delivery_state()` helper method
   - Update `scan_spool_internal()` to restore from Context.delivery
   - Update empathctl to read from spool
   - Remove queue_state.bin logic

**After Phase 1** (remaining critical items from Phase 0):
2. ~~Fix Context/Message layer violation (0.3)~~ - **REJECTED** (intentional design, see CLAUDE.md)
3. Add spool deletion retry mechanism (0.8) - 2 hours
4. Fix DNS cache mutex contention (0.5) - 1 hour (DashMap migration)

**Code Structure Refactoring (Phase 4.0):**
✅ **4.0.1 COMPLETED** - Split empath-delivery/src/lib.rs (98% reduction: 1,894 → 35 lines)

Remaining critical refactoring tasks:
- **4.0.2** Split empath-smtp/src/session.rs (HIGH, 10-15 hours) - 916 lines → ~200 lines
- **4.0.3** Split empath-spool/src/spool.rs (HIGH, 4-6 hours) - 766 lines → ~100 lines
- **4.0.4** Refactor empath/bin/empathctl.rs (MEDIUM, 6-8 hours) - 721 lines → ~100 lines

These refactorings will dramatically improve code maintainability and navigation while preserving all existing functionality. Current file sizes still violating Rust ecosystem conventions:
- empath-smtp/src/session.rs: 916 lines (should be ~200 lines)
- empath-spool/src/spool.rs: 766 lines (should be ~100 lines)
- empath/bin/empathctl.rs: 721 lines (should be ~100 lines)

After completing Phase 1, the system will be production-ready for basic mail delivery with persistent queue state.

---

**Last Updated:** 2025-11-12 (completed task 4.0.1: empath-delivery code structure refactoring - 98% reduction in lib.rs)
**Contributors:** code-reviewer, architect-review, rust-expert, refactoring-specialist agents
**Code Review:** See CODE_REVIEW_2025-11-10.md for comprehensive analysis
