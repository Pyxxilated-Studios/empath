# Empath TODO (Concise)

Last updated: 2025-12-04 (rewritten for an actionable view). See `docs/COMPLETED.md` for history.

Legend: P0 (critical) → P3 (low). Status: [ ] open, [wip] in progress, [x] done.

## P1 — Focus Now
- [ ] NEW-WASM-02: `empath-wasm-guest` helper crate (proc macro + host bindings). Effort ~3d. Success: ergonomic Context API, reject/accept mapping, example module rewritten, docs/migration.
- [ ] NEW-WASM-03: Multi-language WASM examples (Go/TinyGo, C/Emscripten, AssemblyScript). Effort ~4d. Success: each builds <100KB, README with toolchain steps, tests/CI hook.
- [x] NEW-WASM-13: WASM coverage parity. Recently added: timeout enforcement test, memory-limit enforcement (integration), event error-path tests, context mutation edge cases, poisoned mutex regression, invalid SMTP code clamping, memory growth limit unit test.
- [ ] NEW-18: Load & stress testing. Use k6/Locust; measure throughput (1k/10k/100k msg/day), CPU/mem/disk. Add docs + (optional) CI smoke.
- [ ] 2.2 Connection pooling for SMTP client. Pool per-domain with idle cleanup, health check, metrics (hits/misses/size).
- [ ] 0.12 Control socket commands. Remaining: config reload, log level adjust, connection stats, rate-limit adjust.
- [ ] NEW-19: Disaster recovery procedures. Spool/queue/config backup-restore, RTO/RPO, `empathctl backup/restore`, tested scenarios.
- [ ] NEW-22: Capacity planning metrics. Add process CPU/mem/fd, spool disk stats, network bytes/connections, GC if applicable, Grafana dashboard.
- [wip] NEW-09: Newtype safety. Added validated newtypes (`EmailAddress`, `ServerId`, `BannerHostname`) in `empath-common`; SMTP session now uses `BannerHostname` for config/tests. Remaining: propagate to envelope sender/recipients and delivery identifiers.
- [wip] NEW-10: Nightly feature stability plan. Added `docs/NIGHTLY_FEATURES.md`; stable CI lane (`stable-check`) added; low-friction gates removed (`result_option_map_or_default`, `iter_advance_by`, `ascii_char`, `once_cell_try`, `bstr`); remaining: associated_type_defaults, slice_pattern, fn_traits, unboxed_closures.

## P2 — Next Up / Blocked
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
