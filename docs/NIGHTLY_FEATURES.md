# Nightly Feature Stability Plan

Last updated: 2025-12-04  
Goal: converge to stable Rust before 1.0. Track nightly-only features in use, why they’re needed, and migration/replacement plans. CI should include a stable build job (expected to fail until replacements land) to surface gaps early.

## Summary
- Toolchain: nightly (edition 2024).
- Nightly features in use: `associated_type_defaults`, `slice_pattern`, `fn_traits`, `unboxed_closures`, `bstr`.
- Risk: crates.io publication and downstream users on stable are blocked.
- Plan: replace or guard each feature; add a stable CI lane; complete migration by 1.0.

## Feature Inventory

| Feature | Crate(s) | Usage | Stable alternative / plan |
| --- | --- | --- | --- |
| `associated_type_defaults` | `empath-common` | Trait defaults for protocol abstractions | Inline defaults in impls or use helper traits. |
| `slice_pattern` | `empath-common`, `empath-ffi` | Pattern matching on slices | Convert to manual indexing/slice matches behind stable patterns. |
| `fn_traits`, `unboxed_closures` | `empath-tracing` | Procedural macro helper types implementing `FnOnce` | Replace with explicit functions or newtype wrappers implementing `Fn` manually; avoid `FnOnce` impls if possible. |
| `bstr` | `empath-ffi`, `empath-smtp` | Borrowed byte string patterns | Swap to `bstr` crate’s stable APIs without feature gate or use `&[u8]` utilities. |

## Migration Plan

1. **Tracking**: keep this file current; stable CI job (`stable-check`) added (allowed to fail) to show remaining blockers.
2. **Low-friction swaps (1-2 days)**: DONE — removed `result_option_map_or_default`, `ascii_char`, `iter_advance_by`, `once_cell_try` dependencies. Remaining: `bstr` (gate still on, but crate is stable).
3. **Medium effort (2-3 days)**: `slice_pattern`, `associated_type_defaults`. Refactor pattern matches to manual slicing; move defaults into impls or helper traits.
4. **Higher effort (2-3 days)**: `fn_traits`, `unboxed_closures` in `empath-tracing`. Rework macro support to avoid implementing `FnOnce` via unstable traits; use plain functions or wrapper types.
5. **CI**: `stable-check` runs `cargo +stable build --all --all-targets` (allowed to fail until migration completes). Document expected failures until features are removed.
6. **Timeline**: complete swaps before 1.0 code freeze; remove nightly toolchain pin once stable builds pass; keep edition 2024 features behind `cfg` if needed or backport to 2021 if blockers remain.

## Action Items
- [x] Add `stable-check` CI job (allowed to fail initially).
- [x] Replace low-friction nightly features (step 2) except `bstr` gate.
- [ ] Refactor medium/high effort items (steps 3-4).
- [ ] Remove nightly toolchain requirement and update `rust-toolchain.toml`.
- [ ] Update this doc as features are retired.
