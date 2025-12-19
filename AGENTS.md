# Repository Guidelines

## Project Structure & Modules
- Workspace is Rust-first with multiple crates at the root (e.g., `empath`, `empath-smtp`, `empath-delivery`, `empath-control`, `empath-spool`, `empath-metrics`, `empath-ffi`, `empath-tracing`), plus shared tooling in `scripts/`, `docker/`, and docs in `docs/`.
- Configuration examples: `empath.config.ron` (local) and `docker/empath.config.ron` (stack).
- Integration/e2e tests live beside crates and in `tests/` (e.g., `e2e_basic`); examples for FFI/wasm are under `examples/` and `empath-examples/`.

## Build, Test, and Development Commands
- `cargo build` / `cargo build --release` — compile workspace; use `just build` as shorthand.
- `cargo run --bin empath -- empath.config.ron` — start the MTA with a config; `cargo run --bin empathctl -- queue stats` for control CLI.
- `just dev` — format, lint, and test; `just ci` — full local CI (lint + fmt-check + test + examples).
- `just test` or `cargo test` — full test suite; `just test-nextest` for faster runs; `just test-e2e` for end-to-end; `cargo bench` / `just bench` for benchmarks.
- Docker stack: `just docker-up` / `just docker-down` to run the observability environment.

## Agent Runbook
- Before reporting completion: run `cargo fmt --all` and `cargo clippy --all-targets --all-features`.
- Always ask before committing or running any tests.

## Coding Style & Naming Conventions
- Rust nightly (see `rust-toolchain.toml`); format with `cargo fmt --all` (imports grouped per `rustfmt.toml`).
- Clippy is strict (`clippy::all + pedantic + nursery`); run via `just lint` and keep warnings at zero.
- Naming: types `PascalCase`, functions/variables `snake_case`, constants `SCREAMING_SNAKE_CASE`; avoid wildcard imports and keep functions concise (<100 lines).
- Document public items with Rustdoc sections for `Errors` and `Panics` when applicable; prefer explicit byte strings (`b"..."`) and `try_from` over lossy casts.

## Testing Guidelines
- Default to unit tests near implementation; crate-scoped integration tests use `tests/` per crate. End-to-end smoke lives in `tests/e2e_basic.rs` (run with `just test-e2e`).
- Add regression tests for bugs and coverage for new behaviors; prefer `cargo nextest run` for speed locally.
- Benchmarks use Criterion; keep baselines in `target/criterion` (see `just bench-baseline-*` helpers).

## Commit & Pull Request Guidelines
- Commit messages follow Conventional Commits (`feat:`, `fix:`, `chore:`, `docs:`); enforced via `committed.toml`.
- Before opening a PR, run `just ci` and ensure docs/config samples are updated when behavior changes.
- PRs should describe motivation, approach, and risks; link issues when relevant. Include screenshots/log snippets for user-facing or operational changes.
- Keep diffs small and focused; note follow-ups in TODOs or issues instead of mixing scope.

## Security & Configuration Tips
- TLS cert/key samples live at the root for local use only; do not commit real secrets. Treat `docker/` configs as development defaults.
- Use `just docker-test-email` to validate SMTP locally; prefer ephemeral queues (`/tmp/spool`) during testing to avoid lingering mail.

You may also want to refer to [CLAUDE.md](./CLAUDE.md) for other relevant information.
