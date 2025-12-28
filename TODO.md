# Empath TODO (Concise)

Last updated: 2025-12-27 (rewritten for an actionable view). See `docs/COMPLETED.md` for history.

Legend: P0 (critical) → P3 (low). Status: [ ] open, [wip] in progress, [x] done.

> **Completed items** are archived in `docs/COMPLETED.md`; TODO now lists only active work.

## P1 — Focus Now
No active P1 items. See P2/P3/New Ideas for candidates.

## P2 — Next Up / Blocked
- [x] NEW-SEC-ARC-01: ARC seal handling. Validate incoming ARC seals, detect replayed seals, and add regression tests to ensure archived ARC chains keep passing through the delivery pipeline.
- [x] NEW-SEC-DMARC-01: DMARC policy enforcement. Track SPF/DKIM/DMARC observability, surface enforcement decisions via metrics/logs, and provide control/debug tools for rejected/filtered messages.
- [x] NEW-SEC-XX: Control socket roles. Add authenticated control sessions with roles (read-only, admin) so sensitive ops like TLS reloads, binding edits, or DR actions require dedicated ACLs.
- [ ] NEW-19 dependency follow-ups: ensure control socket auth (0.13) decision once DR documented.

## P3 — Deferred / Nice to Have
- [ ] 0.13 Control socket auth (multi-user/ACL) — currently satisfied via Unix perms.
- [ ] 0.14 DNSSEC validation/logging — backlog after 1.0.
- [ ] 5.2/5.3 Config hot reload & TLS policy enforcement — post-Phase 4.
- [ ] NEW-WASM-15+16 performance cleanup after macro/context changes (profiling).
- [ ] NEW-19: Disaster recovery procedures. Spool/queue/config backup-restore, RTO/RPO, `empathctl backup/restore`, tested scenarios.

## New Ideas / Quick Wins
- [ ] NEW-EXPE-01: WASM module fuzz harness. Generate randomized SMTP contexts/events against the WASM API to catch panics/resource leaks in validation modules before deployment.
- [ ] NEW-OBS-ARC-02: DMARC sampling dashboard. Surface a sampled subset of DMARC-failed messages (sender, IP, policy) in metrics/logs to help tune enforcement without sifting through entire logs.
- [x] NEW-QUEUE-01: Priority spool tiers. Allow modules or policies to opt messages into priority delivery tiers and adjust routing/delivery pacing based on queue age/throttles.
- [x] NEW-SEC-ROT-01: Secret rotation helpers. Provide tooling (maybe via `just rotate-secrets`) that mirrors control socket TLS reloads but also rotates API tokens, HMAC keys, cert bundles, etc., to streamline ops.
- [x] NEW-TEST-MX-01: E2E multi-MX coverage. Extend the test harness to support multiple MX priority ordering (mock server port injection or deferred harness build).

## Notes
- Prefer `just test-nextest` locally; `just ci` before PRs.
- Keep clippy clean (pedantic/nursery). ASCII only unless required.
