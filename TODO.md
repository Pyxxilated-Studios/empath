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

## To Prioritise
- [ ] Override Wasm/Python/C code outputs to go through tracing logging, e.g. print/printf function overrides

## P1 — Performance (PERF)

- [ ] PERF-01: Queue scan O(n) optimization. Add `DashSet<SpooledMessageId>` for active messages only. (`empath-delivery/src/processor/process.rs:250-308`, `empath-delivery/src/queue/mod.rs`)
- [ ] PERF-02: Eliminate state clone per command. Use `std::mem::take` for move semantics. (`empath-smtp/src/session/io.rs:111`)
- [ ] PERF-03: RwLock clone for readonly config. Extract fields or use `Arc<RwLock<Arc<Config>>>`. (`empath-delivery/src/processor/process.rs:22,79,138`)
- [ ] PERF-04: Context metadata round-trip. Add `Context::clone_for_storage()`. (`empath-spool/src/backends/memory.rs:143`, `empath-common/src/context.rs`)

## P1 — Code Duplication (DRY) — Continued

- [x] DRY-04: FFI CStr conversion helper. Created `with_cstr()` and `cstr_from_ptr()` helpers in `empath-ffi/src/lib.rs:60-85`. Refactored 20+ occurrences.
- [x] DRY-05: Header validation helper. Created `parse_header_pair()` in `empath-ffi/src/lib.rs:366-394`. Consolidated header name/value parsing with logging.
- [x] DRY-06: String sanitization consolidation. Consolidated into `From<&[u8]>` which sanitizes null bytes. (`empath-ffi/src/ffi_string.rs:51-61`)

## P2 — Large File Refactoring (SPLIT)

- [ ] SPLIT-01: Split `control_handler.rs` (1,791 lines). Extract to `empath/src/control/` module. (`empath/src/control_handler.rs`)
- [ ] SPLIT-02: Split `dns.rs` (1,740 lines). Extract to `empath-delivery/src/dns/` module. (`empath-delivery/src/dns.rs`)
- [ ] SPLIT-03: Split `transaction_handler.rs` (1,242 lines). Extract to `empath-smtp/src/transaction/` module. (`empath-smtp/src/transaction_handler.rs`)

## P3 — Idiomatic Rust (IDIOM)

- [ ] IDIOM-01: Iterator patterns. Replace manual `.next()` loops with `.take(n)` adaptors. (`empath-delivery/src/processor/process.rs:402-412`)
- [ ] IDIOM-02: FFI string error handling. Replace `unwrap_or_default()` with debug assertion. (`empath-ffi/src/ffi_string.rs:64`)

## P1 — Bare Function Refactoring (STRUCT)

- [ ] STRUCT-01: Refactor `address_parser.rs`. Create `AddressParser` unit struct, move 13 bare functions to impl block. (`empath-core/src/address_parser.rs`)
- [ ] STRUCT-02: Refactor `core.rs`. Move 8 `validate_*` functions into `Core` impl. (`empath-ffi/src/modules/core.rs`)
- [ ] STRUCT-03: Refactor `modules/mod.rs`. Move 10+ emit helpers into `ModuleRegistry` impl. (`empath-ffi/src/modules/mod.rs`)
- [ ] STRUCT-04: Refactor `wasm/loader.rs`. Create `WasmRuntime` struct or expand `Wasm` impl. (`empath-ffi/src/modules/wasm/loader.rs`)

## P2 — Bare Function Refactoring (STRUCT) — Continued

- [ ] STRUCT-05: Refactor `message_api.rs`. Move parsing helpers into `MessageParts` impl. (`empath-core/src/message_api.rs`)
- [ ] STRUCT-06: Refactor `dsn.rs`. Move DSN building helpers into `DsnGenerator` impl. (`empath-delivery/src/dsn.rs`)

## P1 — Inline ModuleInterface Delegations (INLINE)

- [x] INLINE-01: Inline `Metrics::emit()` delegation. Move bare function body into trait method. (`empath-ffi/src/modules/metrics.rs`)
- [x] INLINE-02: Inline `Shared` delegations. Move `init_shared()`, `emit_shared()` into trait methods. (`empath-ffi/src/modules/library.rs`)
- [x] INLINE-03: Inline `Python` delegations. Move `init_module()`, `emit_event()` into trait methods. (`empath-ffi/src/modules/python/mod.rs`)
- [x] INLINE-04: Inline `Wasm` delegations. Move `init_module()`, `emit_event()` into trait methods. (`empath-ffi/src/modules/wasm/mod.rs`)
- [x] INLINE-05: Inline `Core::emit()` delegations. Move 8 `validate_*()` functions into match arms. (`empath-ffi/src/modules/core.rs`)

## Notes

- All refactorings are behavioral-preserving (no new features)
- Prefer `just ci` before PRs; keep clippy clean (pedantic/nursery)
- Run tests after each task to verify no regressions
