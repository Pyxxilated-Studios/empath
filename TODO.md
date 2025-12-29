# Empath TODO (Concise)

Last updated: 2025-12-29. See `docs/COMPLETED.md` for history and `docs/BACKLOG.md` for post-1.0 items.

Legend: P0 (critical) → P3 (low). Status: [ ] open, [wip] in progress, [x] done.

## P0 — Security Before Production
- [x] SEC-01: Metrics endpoint auth. Add token or mTLS auth for metrics ingestion, update configs, and document deployment guidance.
- [x] SEC-02: Control endpoint auth defaults. Enable control socket auth in production configs and add docs/tests to prevent unauthenticated control in prod.

## P1 — Operations & Observability
- [x] OPS-01: Grafana panel for DMARC sampling. Add a dashboard panel for `empath.smtp.dmarc.samples.total` with sender/IP/policy filters.
- [x] OPS-02: Config reload safety net. Add control command output that reports reloadable vs restart-required fields (single canonical list) and keep docs in sync.

## P1 — Architecture & Safety
- [ ] ARCH-01: Decouple outbound delivery from SMTP server crate. Extract an SMTP client crate or trait (minimal API) and migrate `empath-delivery` to it. Prevent inbound/outbound circular deps.
- [ ] ARCH-02: Isolate module dispatch on SMTP hot path. Add per-event timeouts, metrics, and optional isolation (spawn_blocking or WASM sandbox), with config knobs and failure policy.

## P2 — Delivery Improvements
- [x] DEL-01: Binding selection transparency. Add control endpoint to show resolved binding + TLS policy per domain (preview mode).
- [x] DEL-02: TLS policy guardrails. Add warnings/logs when TLS is disabled at global or binding scope.

## P2 — Core Type Safety
- [ ] ARCH-03: Replace stringly-typed context keys with typed accessors/newtypes for core keys (TLS/auth/SPF/DMARC). Deprecate raw key usage in core crates.
- [ ] ARCH-04: Split `empath-common` into `empath-core` (domain types, no tracing/tokio) + `empath-common` (infra + otel). Gate observability with features.

## P2 — DX & API Cohesion
- [ ] ARCH-05: Introduce `AuditLogger` trait + builder; replace `audit::*` free functions with injected logger (avoid global config).
- [ ] ARCH-06: Replace logging free functions with `LoggingConfigBuilder` + `LoggingHandle` (init + runtime level updates).
- [x] ARCH-07: Convert `record_spool_io` free function into a `SpoolObserver` trait injected via spool/backing store config.
- [ ] ARCH-08: Create `ModuleRegistry` (builder) for `empath-ffi::modules` to replace free `init/dispatch` and outbound binding helpers.
- [ ] ARCH-09: Move DSN/MTA-STS helper free functions onto types (`DsnGenerator`, `MtaStsPolicy`, `MtaStsPolicyCache`) with builders for config.
- [x] ARCH-10: Move AUTH PLAIN parsing into `PlainCredentials::decode` and align auth parsing behind a common `AuthMechanism` trait.
- [ ] ARCH-11: Replace boolean-heavy TLS params with enums/config structs (e.g., `TlsMode`, `CertValidation`, `TlsRequirement`) in `SmtpTransaction::new` and related call sites.
- [ ] ARCH-12: Collapse long arg lists (`SmtpPoolKey::new`, `SmtpPoolRequest::new`, DSN generation) into request/config structs or builders.
- [ ] ARCH-13: Review large structs (`Context`, `DeliveryInfo`, `SmtpPoolConfig`) for sub-struct grouping and builder ergonomics without breaking serde stability.

## Refactor Plan (Concrete Steps)
- [ ] PLAN-01: Inventory SMTP client usage in `empath-delivery` and define a minimal outbound client interface (traits + types) in a new crate (`empath-smtp-client` or `empath-mailer`).
- [ ] PLAN-02: Move outbound SMTP transaction code to the new crate; update `empath-delivery` to depend on it; ensure `empath-smtp` only depends on shared types, not delivery.
- [ ] PLAN-03: Add module dispatch timeouts and cancellation behavior; capture per-event timing metrics and add explicit failure policy (fail-open/fail-closed per event).
- [ ] PLAN-04: Introduce typed context keys (newtypes + getters/setters) in `empath-common`; migrate core crates to use them; keep raw map only for module extensions.
- [ ] PLAN-05: Split `empath-common` into domain vs infra crates; update workspace dependencies; document feature flags and migration guide.
- [ ] PLAN-06: Add targeted tests: timeout behavior, module isolation failure modes, typed-context regressions, and delivery client contract tests.
- [ ] PLAN-07: Add benchmarks for SMTP hot path and delivery queue throughput before/after refactor; gate regressions.

## P3 — Post-1.0 Candidates
Tracked in `docs/BACKLOG.md`.

## Notes
- Prefer `just test-nextest` locally; `just ci` before PRs.
- Keep clippy clean (pedantic/nursery). ASCII only unless required.
