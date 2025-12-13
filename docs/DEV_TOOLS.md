# Developer Tooling

This repository ships a few helper targets and scripts that keep contributors productive and aligned with strict tooling requirements.

## Environment Doctor (`just doctor`)

`just doctor` (see `justfile:103-111`) runs `./scripts/doctor.sh`, which tests your local toolchain before you start working. It verifies:

- Rust + Cargo are installed, on the nightly toolchain, and at least `rustc 1.93.0-nightly`.
- `just` is available (the primary task runner).
- Optional speedups such as `mold` are present (Linux only).
- Cargo helpers: `cargo-nextest`, `cargo-deny`, `cargo-audit`, and `cargo-watch`.
- Docker + `docker-compose` availability, plus a running daemon.
- Git hooks are installed (`.git/hooks/pre-commit` exists).

Each check logs a success/warning/error with colored output so you know exactly which dependency needs attention before running format/lint/test suites.

## Targeted Formatting + Tests (`just check-changed`)

`just check-changed` (see `justfile:257-259`) runs `scripts/check-changed.sh test`. The script:

1. Requires `jq` on your PATH.
2. Computes which workspace crates have changed relative to `HEAD` using `git diff`.
3. Maps each changed file back to its crate via `cargo metadata` (lines 18-30 in `scripts/check-changed.sh`).
4. Runs `cargo fmt`, `cargo clippy`, and `cargo test` only for those crates (or the full workspace if nothing obvious changed).

Use the `lint` mode (`./scripts/check-changed.sh lint`) for just formatting/lint checks. The script is handy for fast pre-commit validation when you only touched a few crates in this large workspace.

## Pre-commit Hooks (`scripts/install-hooks.sh`)

Run `./scripts/install-hooks.sh` once after cloning to create a `.git/hooks/pre-commit` hook that:

- Runs `cargo fmt --check`.
- Runs `cargo clippy --all-targets --all-features -- -D warnings`.

The script also makes the hook executable and prints guidance on permanently bypassing it (`git commit --no-verify`) only in emergencies.

## Why these tools exist

- `just doctor` ensures everyone has the nightly toolchain + helpers before running `just dev` or `just ci`, reducing “it works on my laptop” issues.
- `just check-changed` lets you run the strict fmt/clippy/test trio without waiting for the entire workspace, keeping commits snappy.
- `install-hooks.sh` enforces formatting + linting automatically so every pushed commit already satisfies strict lint rules.

When you see a new helper command in the `justfile`, document it here so future contributors understand what’s available.
