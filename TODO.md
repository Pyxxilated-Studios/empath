# Empath TODO (Concise)

Last updated: 2026-01-02. See `docs/COMPLETED.md` for history and `docs/BACKLOG.md` for post-1.0 items.

Legend: P0 (critical) → P3 (low). Status: [ ] open, [wip] in progress, [x] done.

## P2 — Code Quality & Performance
- [x] PERF-01: Avoid holding `spool_scan_cursor` lock across `list_delta().await` to prevent scan head-of-line blocking. (`empath-delivery/src/processor/scan.rs`)
- [x] PERF-02: Replace per-call `record_pool_size` full-map scan with per-domain counters or cached sizes. (`empath-delivery/src/smtp_pool.rs`)
- [x] PERF-03: Remove `sys.refresh_all()` in `update_process_metrics` until thread count is implemented or move heavy refresh off async path. (`empath-metrics/src/system.rs`)
- [x] PERF-04: Replace temporary `HashMap<Domain, Vec<String>>` in spool scan with `HashSet<Domain>` (recipients list unused). (`empath-delivery/src/processor/scan.rs`)

## P3 — Test Coverage Gaps
- [x] TEST-01: Add smoke coverage for `empath/bin/empath.rs` entry point. (`empath/bin/empath.rs`)
- [ ] TEST-02: Add smoke/CLI coverage for `empath/bin/empathctl.rs` (control CLI entry point). (`empath/bin/empathctl.rs`)
- [ ] TEST-03: Add focused tests for control path logic (handler + controller). (`empath/src/control_handler.rs`, `empath/src/controller.rs`)
- [ ] TEST-04: Add unit tests for `empath-common` listener/logging utilities. (`empath-common/src/listener.rs`, `empath-common/src/logging.rs`)
- [ ] TEST-05: Add coverage for system metrics + exporter modules. (`empath-metrics/src/system.rs`, `empath-metrics/src/exporter.rs`)
- [ ] TEST-06: Add minimal coverage for FFI modules with 0% coverage (library/metrics/python/validate/wasm). (`empath-ffi/src/modules/library.rs`, `empath-ffi/src/modules/metrics.rs`, `empath-ffi/src/modules/python.rs`, `empath-ffi/src/modules/validate.rs`, `empath-ffi/src/modules/wasm.rs`)

## Notes
- Prefer `just test-nextest` locally; `just ci` before PRs.
- Keep clippy clean (pedantic/nursery). ASCII only unless required.
