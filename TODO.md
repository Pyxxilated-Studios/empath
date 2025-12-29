# Empath TODO (Concise)

Last updated: 2025-12-29. See `docs/COMPLETED.md` for history and `docs/BACKLOG.md` for post-1.0 items.

Legend: P0 (critical) → P3 (low). Status: [ ] open, [wip] in progress, [x] done.

## P0 — Security Before Production
- [x] SEC-01: Metrics endpoint auth. Add token or mTLS auth for metrics ingestion, update configs, and document deployment guidance.
- [x] SEC-02: Control endpoint auth defaults. Enable control socket auth in production configs and add docs/tests to prevent unauthenticated control in prod.

## P1 — Operations & Observability
- [x] OPS-01: Grafana panel for DMARC sampling. Add a dashboard panel for `empath.smtp.dmarc.samples.total` with sender/IP/policy filters.
- [x] OPS-02: Config reload safety net. Add control command output that reports reloadable vs restart-required fields (single canonical list) and keep docs in sync.

## P2 — Delivery Improvements
- [x] DEL-01: Binding selection transparency. Add control endpoint to show resolved binding + TLS policy per domain (preview mode).
- [x] DEL-02: TLS policy guardrails. Add warnings/logs when TLS is disabled at global or binding scope.

## P3 — Post-1.0 Candidates
Tracked in `docs/BACKLOG.md`.

## Notes
- Prefer `just test-nextest` locally; `just ci` before PRs.
- Keep clippy clean (pedantic/nursery). ASCII only unless required.
