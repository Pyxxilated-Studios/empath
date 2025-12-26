# Empath Operator Runbook

Minimal, action-oriented guide for day-to-day operations.

## Fast Status Checks
- Control ping/status: `empathctl system status`
- Queue health: `empathctl queue stats --watch`
- Spool health: `empathctl spool stats`
- DNS cache: `empathctl dns list-cache`
- SMTP capabilities: `empathctl system capabilities`

## Restart / Lifecycle
- Systemd: `sudo systemctl restart empath` (or your service name); check `journalctl -u empath -f`.
- Docker stack: `just docker-down && just docker-up` (keeps spool on disk).
- Foreground binary: send `SIGTERM` (Ctrl+C) and restart `cargo run --bin empath -- empath.config.ron`.

## Common Control Actions
- Drain backlog: `empathctl queue process-now`
- Inspect a message: `empathctl queue view <id> --no-body`
- Retry failed: `empathctl queue retry <id> --force`
- Delete terminal items: `empathctl spool cleanup-completed`
- Clear/refresh DNS: `empathctl dns clear-cache` / `empathctl dns refresh <domain>`

## Common Issues & Fixes
- Cannot connect to control socket: ensure Empath is running and `--control-socket` matches (default `/tmp/empath.sock`).
- Oldest message age growing: run `queue stats --watch`, trigger `process-now`, check spool disk space, and review rate limits.
- STARTTLS errors: confirm domain `require_tls` settings and cert paths; for test-only self-signed flows set per-domain `accept_invalid_certs: true`.
- Spool disk pressure: `empathctl spool stats` then `spool cleanup-completed`; expand `spool.path` if needed.
- DNS anomalies: `dns list-cache` and `dns refresh <domain>`; remove overrides if misrouted.

## Logs & Filtering
- Logs are JSON by default; set `RUST_LOG` or `LOG_LEVEL` (e.g., `RUST_LOG=empath=info,empath_delivery=debug`).
- For focused tracing: `RUST_LOG=empath_smtp=debug,empath_delivery=info`.
- OTLP exports honor `OTEL_EXPORTER_OTLP_ENDPOINT` or the config file endpoint; leave unset to log locally only.
