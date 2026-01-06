# Empath TODO (Concise)

Last updated: 2026-01-05. See `docs/COMPLETED.md` for history and `docs/BACKLOG.md` for post-1.0 items.

Legend: P0 (critical) → P4 (low). Status: [ ] open, [wip] in progress, [x] done.

## P1 — Code Duplication (DRY)

- [x] DRY-01: Extract module error handling helper. Created `handle_module_error()` in `empath-ffi/src/modules/mod.rs` to consolidate Python/WASM error handling. (`empath-ffi/src/modules/mod.rs:44-124`)
- [x] DRY-02: Extract mutation recording logic. Move `module_context_snapshot()`, `record_mutation_records()`, `record_mutation_failure()` to `empath-common/src/mutation_audit.rs`. (`empath-common/src/mutation_audit.rs`)
- [x] DRY-03: Create header mutation helper. Created `mutate_headers<T, F>()` for FFI header functions. (`empath-ffi/src/lib.rs:546-584`)

## P2 — Type Safety (TYPE)

- [x] TYPE-01: Create `ModuleContextSnapshot` type. Replace `Option<(String, String)>` with named struct + `From<&ModuleContext>`. (`empath-common/src/context.rs`)
- [x] TYPE-02: Centralize context key constants. Create public `context_keys` module with namespaced constants (`session::TLS`, `transaction::SPF_RESULT`). (`empath-common/src/context_keys.rs`)

## P3 — Code Quality (REFACTOR)

- [x] REFACTOR-01: Optimize context update path. Already implemented via `emit_inline_in_place` for FailClosed mode without isolation. (`empath-ffi/src/modules/mod.rs:479-528`)
- [x] REFACTOR-02: Add FFI failure logging. Added `internal!(level = TRACE, ...)` to key FFI functions. (`empath-ffi/src/lib.rs`)

## P4 — Documentation (DOC)

- [x] DOC-01: Updated `CONTRIBUTING.md` with Miri compatibility section. (`CONTRIBUTING.md:256-291`)
- [x] DOC-02: Document delivery module integration status. Clarified that delivery events are observation-only (modules can observe but not intercept/reject delivery). (`docs/ARCHITECTURE.md:439-456`)

## Notes

- All refactorings are behavioral-preserving (no new features)
- Prefer `just ci` before PRs; keep clippy clean (pedantic/nursery)
- Run tests after each task to verify no regressions
