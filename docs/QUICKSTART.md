# Empath Quickstart (5 minutes)

This is the fastest path to run Empath locally. For the full guide, see `docs/ONBOARDING.md`.

## Prerequisites

- Rust nightly (see `rust-toolchain.toml`)
- `just` (optional, but handy)

## 1) Build

```bash
cargo build
```

## 2) Run the MTA

```bash
cargo run --bin empath -- empath.config.ron
```

## 3) Verify with the control CLI

```bash
cargo run --bin empathctl -- system ping
cargo run --bin empathctl -- system status
```

## 4) Send a test email

```bash
telnet localhost 1025
```

Or, if you have `just`:

```bash
just docker-test-email
```

## Optional: observability stack

```bash
just docker-up
```

## Next steps

- `docs/ONBOARDING.md` for the full development workflow
- `docs/DEPLOYMENT.md` for production guidance
