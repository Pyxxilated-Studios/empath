# Empath TODO (Concise)

Last updated: 2025-12-24 (rewritten for an actionable view). See `docs/COMPLETED.md` for history.

Legend: P0 (critical) → P3 (low). Status: [ ] open, [wip] in progress, [x] done.

> **Completed items** are archived in `docs/COMPLETED.md`; TODO now lists only active work.

## P1 — Focus Now
- [ ] NEW-18: Load & stress testing. Use k6/Locust; measure throughput (1k/10k/100k msg/day), CPU/mem/disk. Add docs + (optional) CI smoke.
- [ ] 2.2 Connection pooling for SMTP client. Pool per-domain with idle cleanup, health check, metrics (hits/misses/size).
- [x] NEW-22: Capacity planning metrics. Add process CPU/mem/fd, spool disk stats, network bytes/connections, GC if applicable, Grafana dashboard. (partial: core system metrics exist; docs/dashboard pending)
- [x] NEW-23a: Per-binding retry/backoff. Allow bindings to carry timeout/retry profiles; ensure retries stick to the chosen binding unless explicitly allowed to fail over.
- [x] NEW-23b: Binding health/circuit breaker. Track per-binding connect success/latency; temporarily disable unhealthy bindings and surface status via metrics/control.
- [x] NEW-23c: Outbound binding observability. Per-binding counters and latency histograms (connect/EHLO); add Grafana panels.
- [x] NEW-23d: Control inspection for bindings. Control command to show binding selection for a queued message and toggle/disable a binding at runtime.
- [x] NEW-23e: Module binding ergonomics. Typed binding-name enum in FFI/WASM so modules can list/choose configured bindings; validate names with tests.
- [x] NEW-24: Per-domain bounce classification. Track temp vs perm failures by domain, expose metrics, and feed retry policy tuning.
- [x] NEW-26: Deterministic WASM harness. Provide fixed RNG/time environment to make module tests reproducible in CI.
- [x] NEW-27: Spool I/O observability. Structured logs/metrics for read/write/update/delete latency with percentiles to catch slow disks.
- [x] NEW-28a: Mutation limits & audit. Enforce header/body size/count limits on module mutations, emit audit events per module for header/body changes, and add metrics for mutation successes/failures (ties into NEW-27 observability). (partial: limits/audit types exist; metrics/events wiring pending)
- [x] NEW-28b: Module helper docs/tests. Regression tests for multi-value header order, encoded-word folding, newline normalization, and body append; WASM/FFI examples showing header mutations and multipart text/html helpers; doc snippet for module authors on safe mutations and signature/charset pitfalls. (partial: docs/tests exist, missing specific regression coverage)
- [ ] 0.12a Full config reload. Extend control reload to reapply all config facets in-process (SMTP/controller timeouts, DNS, delivery retry/circuit breaker, metrics/health/audit) or clearly mark restart-required fields; add tests and docs.

## P2 — Next Up / Blocked
- [ ] NEW-WASM-16: Split WIT context (read-only vs mutable or event-specific views) with compatibility plan.
- [ ] NEW-WASM-04: Component Model with WIT interfaces. Optional but aligns with NEW-WASM-16.
- [ ] NEW-21: Trace context propagation verification (blocked on span instrumentation readiness). E2E trace_id preservation SMTP→spool→delivery + JoinSet tasks, docs.
- [ ] NEW-19 dependency follow-ups: ensure control socket auth (0.13) decision once DR documented.
- [ ] NEW-TLS-02: TLS visibility dashboard. Provide a way to inspect which cert/key pairs are currently loaded (thumbprint, expiry, SANs) via control socket or metrics to confirm what the active credentials are.
- [ ] NEW-TLS-03: TLS chain validation watchdog. Periodically verify cert/key files (and bundles) on disk for expiry or misconfiguration, surface metrics/alerts before rotation, and flag mismatches via control/metrics so operators know when reloads will fail.
- [ ] NEW-TLS-04: Dual-stack TLS profiles. Support configuring multiple cert/key pairs per listener (e.g., “default” and “rotation” profiles) and provide control operations to switch active profiles or drain sockets without aborting in-flight connections.
- [ ] NEW-CTL-XX: Binding inspection. Extend the control socket to list outbound bindings and show which binding each queued message currently prefers, optionally allowing runtime enabling/disabling for diagnoses.
- [ ] NEW-SEC-XX: Control socket roles. Add authenticated control sessions with roles (read-only, admin) so sensitive ops like TLS reloads, binding edits, or DR actions require dedicated ACLs.
- [ ] NEW-SEC-DKIM-01: DKIM verification telemetry. Add enforced DKIM signature verification plus metrics/logs for failures, and consider signing support for outbound messages where missing.
- [ ] NEW-SEC-DMARC-01: DMARC policy enforcement. Track SPF/DKIM/DMARC observability, surface enforcement decisions via metrics/logs, and provide control/debug tools for rejected/filtered messages.
- [ ] NEW-SEC-ARC-01: ARC seal handling. Validate incoming ARC seals, detect replayed seals, and add regression tests to ensure archived ARC chains keep passing through the delivery pipeline.
- [ ] NEW-SMTP-XX: Extended command coverage. Add support for RFC commands such as `NOOP`, `VRFY`, `EXPN`, `ETRN`, `TURN` (and any other common extensions besides `BDAT`) so the MTA can pass compatibility checks with legacy clients.
- [ ] NEW-CAP-XX: Capability toggles. Allow modules or control handlers to add/remove advertised SMTP capabilities (e.g., hide `AUTH` until STARTTLS completes) at runtime so operators can enforce policies dynamically without restarting the listener.

## P3 — Deferred / Nice to Have
- [ ] 0.13 Control socket auth (multi-user/ACL) — currently satisfied via Unix perms.
- [ ] 0.14 DNSSEC validation/logging — backlog after 1.0.
- [ ] 5.2/5.3 Config hot reload & TLS policy enforcement — post-Phase 4.
- [ ] NEW-WASM-15+16 performance cleanup after macro/context changes (profiling).
- [ ] NEW-19: Disaster recovery procedures. Spool/queue/config backup-restore, RTO/RPO, `empathctl backup/restore`, tested scenarios.

## Recently Completed (this branch)
_See `docs/COMPLETED.md` for the full archive of resolved work._

## New Ideas / Quick Wins
- [ ] NEW-OBS-ARC-02: DMARC sampling dashboard. Surface a sampled subset of DMARC-failed messages (sender, IP, policy) in metrics/logs to help tune enforcement without sifting through entire logs.
- [ ] NEW-SEC-ROT-01: Secret rotation helpers. Provide tooling (maybe via `just rotate-secrets`) that mirrors control socket TLS reloads but also rotates API tokens, HMAC keys, cert bundles, etc., to streamline ops.
- [ ] NEW-QUEUE-01: Priority spool tiers. Allow modules or policies to opt messages into priority delivery tiers and adjust routing/delivery pacing based on queue age/throttles.
- [ ] NEW-CTL-OBS-01: Control command rate limiting. Throttle control socket commands (reloads, requeues, bindings) per client to prevent automation floods during maintenance windows.
- [ ] NEW-EXPE-01: WASM module fuzz harness. Generate randomized SMTP contexts/events against the WASM API to catch panics/resource leaks in validation modules before deployment.
- [x] NEW-TEST-DSN-01: E2E harness DSN inspection. Expose DSN/bounce messages in the E2E test harness and add a regression test for permanent failure bounces.
- [ ] NEW-TEST-MX-01: E2E multi-MX coverage. Extend the test harness to support multiple MX priority ordering (mock server port injection or deferred harness build).

## Notes
- Prefer `just test-nextest` locally; `just ci` before PRs.
- Keep clippy clean (pedantic/nursery). ASCII only unless required.
