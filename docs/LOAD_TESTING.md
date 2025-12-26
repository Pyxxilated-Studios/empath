# Load and Stress Testing

This guide captures a repeatable load test approach for Empath. It focuses on
SMTP send throughput, resource usage (CPU/memory/disk), and queue behavior.

## Goals

- Measure steady-state throughput at 1k, 10k, and 100k messages per day.
- Record CPU, memory, disk usage, and queue depth under each load profile.
- Verify that delivery latency and error rates stay within acceptable limits.

## Prerequisites

- Build binaries: `just build` (or `cargo build --release`).
- Start Empath with a config that points to a local or test SMTP sink.
- Prefer an ephemeral spool (e.g., `/tmp/spool`) for repeatable tests.
- Optional: start the observability stack for metrics dashboards:
  `just docker-up`.

## Throughput Targets

Use these approximations to translate daily volume into a steady send rate:

- 1k/day ~= 0.012 msgs/sec
- 10k/day ~= 0.12 msgs/sec
- 100k/day ~= 1.16 msgs/sec

For short runs, pick a rate that matches the desired daily volume and sustain
it for 10-30 minutes to capture steady-state behavior.

## Locust (SMTP)

Locust uses Python to drive SMTP sends. The provided `locustfile.py` sends a
single message per task using `smtplib`.

### Install

```bash
python -m pip install locust
```

### Run (headless)

```bash
SMTP_HOST=127.0.0.1 \
SMTP_PORT=2525 \
SMTP_FROM=loadtest@example.com \
SMTP_TO=sink@example.com \
MESSAGE_BODY_BYTES=1024 \
WAIT_MIN_SEC=0.5 \
WAIT_MAX_SEC=1.5 \
locust -f scripts/loadtest/locustfile.py --headless --users 1 --spawn-rate 1 --run-time 15m
```

### Rate tuning

Approximate send rate is:

```
messages_per_second ~= users / average_wait_seconds
```

For 100k/day (1.16 msg/s), start with `users=1` and `WAIT_MIN_SEC=0.5`,
`WAIT_MAX_SEC=1.5`, then adjust until the observed rate matches your target.

### Environment variables

- `SMTP_HOST` / `SMTP_PORT` - Target SMTP listener.
- `SMTP_FROM` / `SMTP_TO` - Envelope addresses.
- `SMTP_HELO` - Optional EHLO name.
- `SMTP_STARTTLS` - Set to `1` to enable STARTTLS.
- `SMTP_USERNAME` / `SMTP_PASSWORD` - Optional auth.
- `MESSAGE_BODY_BYTES` - Body size (approximate).
- `WAIT_MIN_SEC` / `WAIT_MAX_SEC` - Think time between sends.

## k6 (SMTP via extension)

k6 does not include SMTP natively. Use the `xk6-smtp` extension if you prefer
k6-based load testing. Build a custom k6 binary with the SMTP extension and
use a script that sends one SMTP message per iteration. Keep the same targets
as the Locust section and capture rate/latency with the same metrics.

## What to record

- Send rate (msgs/sec) and average latency.
- CPU usage, memory usage, and file descriptor count.
- Spool disk I/O and queue depth.
- Delivery error rate and retry counts.

## Optional CI smoke

For a lightweight smoke test, run Locust headless with a single user and a
short duration (30-60 seconds). This should be non-blocking and only validate
that the SMTP path is responsive.
