# Empath TODO (Concise)

Last updated: 2025-12-06 (rewritten for an actionable view). See `docs/COMPLETED.md` for history.

Legend: P0 (critical) → P3 (low). Status: [ ] open, [wip] in progress, [x] done.

## P1 — Focus Now
- [x] BUG-CTL-AGE: Fix queue/spool age calculations to avoid `u64` underflow when clocks skew or spool timestamps are in the future. Status: control stats now use `SystemTime` with saturation; add regression coverage.
- [x] PERF-QUEUE-CLONE: Stop cloning the entire delivery queue on every scan. Status: processor iterates DashMap in place, pending items cloned only; removed `all_messages()` helper.
- [x] PERF-CTL-STATS: Stream/page control queue/spool listings and avoid reading full messages when not needed. Success: list/stats stay responsive on large queues; body/header reads are optional.
- [ ] NEW-WASM-02: `empath-wasm-guest` helper crate (proc macro + host bindings). Effort ~3d. Success: ergonomic Context API, reject/accept mapping, example module rewritten, docs/migration.
- [ ] NEW-WASM-03: Multi-language WASM examples (Go/TinyGo, C/Emscripten, AssemblyScript). Effort ~4d. Success: each builds <100KB, README with toolchain steps, tests/CI hook.
- [x] NEW-WASM-13: WASM coverage parity. Recently added: timeout enforcement test, memory-limit enforcement (integration), event error-path tests, context mutation edge cases, poisoned mutex regression, invalid SMTP code clamping, memory growth limit unit test.
- [ ] NEW-18: Load & stress testing. Use k6/Locust; measure throughput (1k/10k/100k msg/day), CPU/mem/disk. Add docs + (optional) CI smoke.
- [ ] 2.2 Connection pooling for SMTP client. Pool per-domain with idle cleanup, health check, metrics (hits/misses/size).
- [x] 0.12 Control socket commands. Added config reload (domains/rate limits), log level adjust, connection stats, and rate-limit adjust. Full multi-component reload follow-up noted.
- [ ] NEW-19: Disaster recovery procedures. Spool/queue/config backup-restore, RTO/RPO, `empathctl backup/restore`, tested scenarios.
- [ ] NEW-22: Capacity planning metrics. Add process CPU/mem/fd, spool disk stats, network bytes/connections, GC if applicable, Grafana dashboard.
- [ ] NEW-23: Outbound bindings. Configurable binding definitions (source IP/interface + TLS/helo), mapping rules (static/domain/module override), stored per message for retries/DSN, SMTP client bind support, metrics/control listing, docs + tests.
- [ ] NEW-23a: Per-binding retry/backoff. Allow bindings to carry timeout/retry profiles; ensure retries stick to the chosen binding unless explicitly allowed to fail over.
- [ ] NEW-23b: Binding health/circuit breaker. Track per-binding connect success/latency; temporarily disable unhealthy bindings and surface status via metrics/control.
- [ ] NEW-23c: Outbound binding observability. Per-binding counters and latency histograms (connect/EHLO); add Grafana panels.
- [ ] NEW-23d: Control inspection for bindings. Control command to show binding selection for a queued message and toggle/disable a binding at runtime.
- [ ] NEW-23e: Module binding ergonomics. Typed binding-name enum in FFI/WASM so modules can list/choose configured bindings; validate names with tests.
- [ ] NEW-23f: Binding config linting. Detect unmapped/unknown binding names and missing interfaces at startup; emit warnings/errors early.
- [x] DX-CTL-ZERO-CLONE: Introduce borrowed queue/spool views (DashMap guards or streaming page iterators) so control list/view paths avoid cloning `DeliveryInfo`/contexts and extra `Vec` allocations.
- [x] DX-SPOOL-METADATA: Add a single metadata read path (size/status) to avoid per-message double I/O in spool listings and make filters operate on metadata without fetching bodies.
- [x] DX-CTL-BORROW: Make control protocol filters/IDs borrow where possible (`QueueCommand`/`SpoolCommand` fields via serde borrowing) to avoid String allocations on hot paths.
- [x] DX-CTL-ITER: Add borrowed iterators for delivery queue listings (`list_messages`/`pending_messages`) and update control handlers to avoid cloning `DeliveryInfo` for pagination.
- [ ] NEW-24: Per-domain bounce classification. Track temp vs perm failures by domain, expose metrics, and feed retry policy tuning.
- [x] NEW-25: SMTP STARTTLS policy tests. Added runtime coverage for require/allow/disable STARTTLS negotiation paths (including disabled policy skip) and RON serde coverage for TLS policy config; banner hostname validated via `BannerHostname` newtype.
- [ ] NEW-26: Deterministic WASM harness. Provide fixed RNG/time environment to make module tests reproducible in CI.
- [ ] NEW-27: Spool I/O observability. Structured logs/metrics for read/write/update/delete latency with percentiles to catch slow disks.
- [ ] 0.12a Full config reload. Extend control reload to reapply all config facets in-process (SMTP/controller timeouts, DNS, delivery retry/circuit breaker, metrics/health/audit) or clearly mark restart-required fields; add tests and docs.
- [x] DX-02: Fast inner-loop helpers. Added `scripts/check-changed.sh` and `just lint-quick`/`just check-changed` to run fmt/clippy/tests on changed crates (workspace fallback).
- [x] DOC-05: Operator runbook. Added `docs/OPERATOR_RUNBOOK.md` with restart/health steps, common errors, control commands, and log filters; linked from README.
- [x] NEW-09: Newtype safety. Added validated newtypes (`EmailAddress`, `ServerId`, `BannerHostname`) in `empath-common`; SMTP session now uses `BannerHostname` for config/tests; SPF evaluator now accepts `EmailAddress`; envelope audit logging uses typed sender/recipients and delivery audit paths use `ServerId`.
- [wip] NEW-10: Nightly feature stability plan. Added `docs/NIGHTLY_FEATURES.md`; stable CI lane (`stable-check`) added; low-friction gates removed (`result_option_map_or_default`, `iter_advance_by`, `ascii_char`, `once_cell_try`, `bstr`); remaining: associated_type_defaults, slice_pattern, fn_traits, unboxed_closures.

## P2 — Next Up / Blocked
- [x] DX-CTL-AUDIT: Factor control handler audit user/uid lookup into a helper to remove repeated unsafe blocks and keep audit metadata consistent. Added `audit!` macro in control handler that captures user/uid once and injects into tracing events.
 - [x] DX-CTL-API: Make control handler commands accept `&str` where possible (e.g., retry/delete) to reduce unnecessary `String` allocations.
 - [x] DX-CTL-SELECT: Deduplicate the `tokio::select!` branches in `Empath::run` so health-enabled and disabled flows share the same branch list.
- [ ] NEW-WASM-15: Event registration macro to centralize event definitions and mappings (Ev enum, WIT names, handlers).
- [ ] NEW-WASM-16: Split WIT context (read-only vs mutable or event-specific views) with compatibility plan.
- [ ] NEW-WASM-04: Component Model with WIT interfaces. Optional but aligns with NEW-WASM-16.
- [ ] NEW-21: Trace context propagation verification (blocked on span instrumentation readiness). E2E trace_id preservation SMTP→spool→delivery + JoinSet tasks, docs.
- [ ] NEW-19 dependency follow-ups: ensure control socket auth (0.13) decision once DR documented.

## P3 — Deferred / Nice to Have
- [ ] 0.13 Control socket auth (multi-user/ACL) — currently satisfied via Unix perms.
- [ ] 0.14 DNSSEC validation/logging — backlog after 1.0.
- [ ] 5.2/5.3 Config hot reload & TLS policy enforcement — post-Phase 4.
- [ ] NEW-WASM-15+16 performance cleanup after macro/context changes (profiling).

## Recently Completed (this branch)
- [x] NEW-WASM-17: Module error metrics + tests (`empath-ffi` emit path, `empath-metrics` coverage).
- [x] NEW-WASM-14: Graceful poisoned mutex recovery (host functions log/return defaults) + regression test.
- [x] NEW-WASM-13 (partial): Invalid SMTP code clamping test; memory growth limit test.
- [x] IDEA-DX-01: Cache/reuse built `simple_validator.wasm` in tests (skip `xtask build-wasm` if artifact fresh) to speed CI.
- [x] IDEA-OBS-01: Add log/metric for WASM module validation timeout vs other errors (attribute in module error metrics).

## New Ideas / Quick Wins
- [x] IDEA-WASM-01: Deterministic timeout test harness using a tiny Wasm module that sleeps via busy loop; drive epoch counter to assert host returns timeout code (unblocks NEW-WASM-13 timeout test).

## Notes
- Prefer `just test-nextest` locally; `just ci` before PRs.
- Keep clippy clean (pedantic/nursery). ASCII only unless required.
