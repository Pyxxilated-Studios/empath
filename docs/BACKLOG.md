# Empath MTA - Post-1.0 Backlog

> **Last Updated**: 2025-11-20
> **Total Backlog Items**: 15

This file contains tasks deferred to post-1.0 release. These are enhancements and optimizations that are not required for production deployment.

---

## Phase 6: Advanced Features (Post-1.0)

### 🔵 6.1 Message Data Streaming
**Priority**: Low
**Effort**: 1 week
**Status**: Deferred to post-1.0
**Tags**: performance, memory

**Problem**: Large message bodies loaded entirely into memory. Can cause memory pressure with large attachments (50MB+ emails).

**Solution**: Stream message data instead of buffering.

**Success Criteria**:
- [ ] Stream SMTP DATA command to spool (no in-memory buffer)
- [ ] Stream spool reads during delivery
- [ ] Memory usage <10MB per message regardless of size
- [ ] Benchmark: Handle 100MB attachments without OOM
- [ ] Configurable buffer size for small messages (optimization)

**Dependencies**: Requires spool trait changes (breaking change)

---

### 🔵 6.2 DKIM Signing Support
**Priority**: Low
**Effort**: 1 week
**Status**: Deferred to post-1.0
**Tags**: deliverability, compliance

**Problem**: No DKIM signing for outbound messages. Impacts deliverability to strict domains (Gmail, Outlook).

**Solution**: Implement DKIM signing (RFC 6376) for outbound messages.

**Success Criteria**:
- [ ] DKIM signing for outbound messages
- [ ] Per-domain DKIM key configuration
- [ ] Configurable signing headers (From, To, Subject, Date, etc.)
- [ ] Private key storage (file-based or external KMS)
- [ ] DKIM signature verification in tests
- [ ] Documentation: Key generation, DNS TXT record setup

**Dependencies**: None (can add independently)

---

### 🔵 6.3 Priority Queuing
**Priority**: Low
**Effort**: 3-5 days
**Status**: Deferred to post-1.0
**Tags**: delivery, prioritization

**Problem**: All messages treated equally. High-priority messages (password resets, security alerts) delayed by bulk mail.

**Solution**: Implement message priority levels for expedited delivery.

**Success Criteria**:
- [ ] Priority levels: Critical, High, Normal, Low, Bulk
- [ ] Priority configured per message (SMTP extension or header)
- [ ] Delivery queue processes high-priority messages first
- [ ] Per-priority rate limiting (don't starve bulk mail)
- [ ] Metrics: delivery latency by priority
- [ ] Tests verify priority ordering

**Dependencies**: Requires queue structure changes

---

### 🔵 6.4 Batch Processing and SMTP Pipelining
**Priority**: Low
**Effort**: 1 week
**Status**: Deferred to post-1.0
**Tags**: performance, delivery

**Problem**: SMTP commands sent one-at-a-time with wait for response. Latency overhead for distant servers.

**Solution**: Implement SMTP pipelining (RFC 2920) for improved throughput.

**Success Criteria**:
- [ ] PIPELINING extension support (outbound client)
- [ ] Batch MAIL FROM + RCPT TO commands
- [ ] Error handling for pipelined commands
- [ ] Performance benchmark: >30% throughput improvement
- [ ] Configurable: disable pipelining for broken servers
- [ ] Tests with mock servers (accept/reject pipelining)

**Dependencies**: Requires SMTP client refactoring

---

### 🔵 6.7 Extended Property-Based Testing
**Priority**: Low
**Effort**: 2-3 days
**Status**: Deferred to post-1.0
**Note**: SMTP property testing complete (NEW-13), this is for DNS/delivery

**Problem**: Property tests only cover SMTP command parsing. DNS and delivery logic not fuzzed.

**Solution**: Extend proptest coverage to DNS and delivery layers.

**Success Criteria**:
- [ ] Property tests for DNS response parsing
- [ ] Property tests for retry schedule calculation
- [ ] Property tests for rate limiter token bucket
- [ ] Property tests for circuit breaker state machine
- [ ] Fuzz testing integration (cargo fuzz) - optional

**Dependencies**: None

---

## Enhancements (Nice to Have)

### 🔵 0.14 / 1.2.1 - DNSSEC Validation
**Priority**: Low (Deferred - Premature)
**Effort**: 2 days
**Status**: Deferred to post-1.0
**Tags**: dns, security

**Expert Review**: Premature - no DNSSEC infrastructure in most deployments. Defer until core reliability proven.

**Problem**: No DNSSEC validation for MX record lookups. Vulnerable to DNS poisoning attacks.

**Solution**: Enable DNSSEC validation in resolver and log validation status.

**Success Criteria**:
- [ ] DNSSEC validation enabled in Hickory DNS resolver
- [ ] Log DNSSEC validation status (secure, insecure, bogus)
- [ ] Metrics: dnssec_validation_failures_total
- [ ] Configurable: fail delivery on DNSSEC failure (strict mode)
- [ ] Documentation: DNSSEC deployment guide

---

### 🔵 NEW-09 - Newtype Pattern Extension for Type Safety
**Priority**: Low
**Effort**: 2-3 days
**Status**: Deferred to post-1.0
**Tags**: rust, type-safety, refactoring

**Problem**: Task 4.4 created `Domain` newtype, but other string types lack compile-time safety: `EmailAddress`, `ServerId`, `BannerHostname`.

**Solution**: Create newtypes for email addresses, server IDs, hostnames.

**Success Criteria**:
- [ ] `EmailAddress` newtype with validation (contains '@')
- [ ] `ServerId` newtype for MX server addresses
- [ ] `BannerHostname` newtype for SMTP banners
- [ ] Zero runtime overhead (#[repr(transparent)])
- [ ] Compile-time prevention of domain/email confusion bugs

---

### 🔵 NEW-20 - TLS Upgrade Abstraction
**Priority**: Low (Post-1.0)
**Effort**: 1-2 days
**Tags**: refactoring, tls

**Problem**: TLS upgrade logic inline in session handler (special-case). Violates Liskov Substitution Principle.

**Solution**: Extract TLS upgrade into proper state transition handler.

**Success Criteria**:
- [ ] TLS upgrade as state transition (not inline special-case)
- [ ] Reusable abstraction for other protocol upgrades
- [ ] Tests verify context preservation across upgrade
- [ ] No behavioral changes (refactoring only)

**Dependencies**: Wait until Phase 4 complete (avoid scope creep)

---

## Developer Experience (Low Priority)

### 🔵 7.13 - sccache for Distributed Build Caching
**Priority**: Low
**Effort**: 1 hour
**Status**: Deferred
**Tags**: dx, performance

**Note**: Build caching already exists via `actions/cache@v4` in CI. This task is for distributed cache sharing across PRs.

**Problem**: CI builds compile from scratch per PR (cache only within same PR). Wastes CI minutes.

**Solution**: Implement sccache for distributed build caching across all CI jobs.

**Success Criteria**:
- [ ] sccache configured in Gitea Actions
- [ ] CI build time reduced >50% on cache hit
- [ ] Local sccache setup documented in CONTRIBUTING.md
- [ ] S3/Redis backend for cache storage (persistent)

---

### 🔵 7.14 - Documentation Tests
**Priority**: Low
**Effort**: 1-2 days
**Status**: Already runs in CI, this is for expanding coverage
**Tags**: documentation, testing

**Note**: `cargo test --doc` already runs in CI (line 88 of test.yml). This task is to ensure all code examples are testable.

**Problem**: Some code examples in CLAUDE.md, README.md not covered by doc tests.

**Solution**: Expand doc test coverage to all documentation files.

**Success Criteria**:
- [ ] All code examples in CLAUDE.md tested via doc tests
- [ ] All code examples in README.md tested
- [ ] CONTRIBUTING.md examples tested
- [ ] CI fails if doc examples broken
- [ ] `#![doc = include_str!("../README.md")]` pattern used

---

### 🔵 7.17 - Quickstart Guide
**Priority**: Low (downgraded)
**Effort**: 30 minutes
**Status**: Deferred
**Tags**: documentation, dx

**Note**: ONBOARDING.md already exists with comprehensive 15-minute setup guide. This is for a scannable 5-minute version.

**Problem**: ONBOARDING.md is comprehensive but not scannable. Need literal copy-paste guide.

**Solution**: Create QUICKSTART.md with 5-minute path.

**Success Criteria**:
- [ ] Single page with copy-paste commands
- [ ] Links to ONBOARDING.md for details
- [ ] Takes <5 minutes for experienced developer
- [ ] Covers: clone, install, build, test, run

---

### 🔵 7.24 - Performance Profiling Guide
**Priority**: Low (will become Medium when performance optimization starts)
**Effort**: 2 hours
**Status**: Deferred
**Tags**: dx, performance, documentation

**Problem**: Performance claims ("90% reduction") not reproducible. No profiling workflow docs.

**Solution**: Create `docs/PROFILING.md` with comprehensive profiling guide.

**Success Criteria**:
- [ ] CPU profiling with flamegraph (cargo flamegraph)
- [ ] Memory profiling with dhat
- [ ] Benchmark baseline comparison workflow
- [ ] Common hot paths documented
- [ ] justfile commands added (profile-cpu, profile-mem)
- [ ] Interpreting results guide

---

## Operational Enhancements (Medium Priority - Post Launch)

### 🟢 5.2 - Configuration Hot Reload
**Priority**: Medium
**Effort**: 2-3 days
**Status**: Deferred until post-Phase 4
**Tags**: operations, configuration

**Problem**: Configuration changes require full restart - downtime and queue state loss.

**Solution**: Implement configuration hot reload via control socket or file watcher.

**Success Criteria**:
- [ ] Reload via `empathctl config reload`
- [ ] Validate config before applying (rollback on error)
- [ ] Log all config changes with diff
- [ ] Tests verify reload without service disruption
- [ ] Safe to reload: timeouts, rate limits, circuit breaker thresholds
- [ ] Unsafe to reload: listeners, spool path, TLS keys (require restart)

**Dependencies**: Wait for Phase 4 (stable architecture first)

---

### 🟢 5.3 - TLS Policy Enforcement
**Priority**: Medium
**Effort**: 2-3 days
**Status**: Deferred
**Tags**: security, delivery

**Problem**: No TLS policy enforcement - can deliver via plaintext to sensitive domains.

**Solution**: Implement configurable TLS policies per domain (Opportunistic, Required, Disabled).

**Success Criteria**:
- [ ] TLS policy: Opportunistic (try TLS, fall back to plaintext)
- [ ] TLS policy: Required (fail if TLS unavailable)
- [ ] TLS policy: Disabled (never use TLS - testing only)
- [ ] Per-domain policy overrides
- [ ] Metrics: tls_handshake_failures_total{domain,policy}
- [ ] Delivery fails permanently if Required policy violated

---

## Backlog Management

**Review Cadence**: Quarterly (or after each major release)

**Promotion Criteria**: Move to active TODO.md if:
1. Production deployment reveals need
2. User/operator requests feature
3. Becomes prerequisite for other work
4. Technical debt impacts velocity

**Archival Criteria**: Move to archive if:
1. No longer relevant (technology/requirements changed)
2. Superseded by alternative approach
3. Community consensus to drop feature

---

**See also**: [TODO.md](../TODO.md) for active tasks, [COMPLETED.md](./COMPLETED.md) for completed work.
