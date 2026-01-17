# Empath TODO (Concise)

Last updated: 2026-01-18. See `docs/COMPLETED.md` for history and `docs/BACKLOG.md` for post-1.0 items.

Legend: P0 (critical) → P4 (low). Status: [ ] open, [wip] in progress.

## To Prioritise

- [ ] Override Wasm/Python/C code outputs to go through tracing logging, e.g. print/printf function overrides
- [ ] Message bifurcation by recipient

## P3 — Idiomatic Rust (IDIOM)

- [ ] IDIOM-02: FFI string error handling. Replace `unwrap_or_default()` with debug assertion. (`empath-ffi/src/ffi_string.rs:64`)

## Notes

- All refactorings are behavioral-preserving (no new features)
- Prefer `just ci` before PRs; keep clippy clean (pedantic/nursery)
- Run tests after each task to verify no regressions
