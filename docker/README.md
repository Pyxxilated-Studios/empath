# Docker Development Environment

Full observability stack for Empath MTA development, including SMTP server with FFI module examples, OpenTelemetry metrics collection, Prometheus monitoring, and Grafana visualization.

## Quick Start

```bash
# From repository root
just docker-up      # Start all services
just docker-logs    # View Empath logs
just docker-down    # Stop all services
```

## Services

| Service | Port | Purpose | URL |
|---------|------|---------|-----|
| **Empath MTA** | 1025 | SMTP server | smtp://localhost:1025 |
| **Grafana** | 3000 | Metrics visualization | http://localhost:3000 |
| **Prometheus** | 9090 | Metrics storage/query | http://localhost:9090 |
| **OTEL Collector** | 4318, 8889 | Metrics collection | http://localhost:4318 (OTLP), http://localhost:8889 (metrics) |

**Grafana Credentials:** admin / admin

## Features

### FFI Example Modules

The Docker image includes pre-built FFI example modules demonstrating the plugin system:

- **libexample.so** - Validation listener with custom logging
  - Logs SMTP transaction events (CONNECT, MAIL FROM, RCPT TO, DATA, etc.)
  - Demonstrates context manipulation and metadata storage

- **libevent.so** - Event listener for connection lifecycle
  - Tracks connection opened/closed events
  - Monitors delivery attempts and successes
  - Shows delivery context access (domain, server, status)

These modules are automatically loaded via the `docker/empath.config.ron` configuration file.

## Directory Structure

```
docker/
├── README.md                 # This file
├── Dockerfile                # Production multi-stage build
├── Dockerfile.dev            # Development build with FFI modules
├── compose.dev.yml           # Docker Compose configuration
├── empath.config.ron         # Empath config with FFI modules enabled
├── otel-collector.yml        # OpenTelemetry Collector configuration
├── prometheus.yml            # Prometheus scrape configuration
└── grafana/                  # Grafana provisioning
    └── provisioning/
        ├── dashboards/       # Pre-configured dashboards
        └── datasources/      # Prometheus datasource config
```

## Manual Commands

### Starting the Stack

```bash
# From repository root
docker-compose -f docker/compose.dev.yml up -d
```

### Viewing Logs

```bash
# All services
docker-compose -f docker/compose.dev.yml logs -f

# Empath only
docker-compose -f docker/compose.dev.yml logs -f empath

# OTEL Collector only
docker-compose -f docker/compose.dev.yml logs -f otel-collector
```

### Stopping the Stack

```bash
# Stop all services
docker-compose -f docker/compose.dev.yml down

# Stop and remove volumes (clean slate)
docker-compose -f docker/compose.dev.yml down -v
```

### Rebuilding

```bash
# Rebuild Empath image (e.g., after code changes)
docker-compose -f docker/compose.dev.yml build empath

# Rebuild and restart
docker-compose -f docker/compose.dev.yml up -d --build empath
```

## Configuration

### Empath Configuration

The `empath.config.ron` file in this directory is specifically configured for Docker:

- SMTP listeners on ports 1025 and 1026
- Spool directory: `/tmp/spool/empath` (mounted as volume)
- FFI modules loaded from `/empath/modules/`
- Metrics endpoint: `http://otel-collector:4318/v1/metrics`

### Customizing

To modify the configuration:

1. Edit `docker/empath.config.ron`
2. Restart the Empath service:
   ```bash
   docker-compose -f docker/compose.dev.yml restart empath
   ```

To add custom modules:

1. Place your compiled `.so` files in a directory
2. Mount the directory in the compose file:
   ```yaml
   volumes:
     - ./my-modules:/custom/modules
   ```
3. Update `empath.config.ron` to load from `/custom/modules/`

## Observability Stack

### Metrics Flow

```
Empath MTA → OTLP/HTTP → OTEL Collector → Prometheus → Grafana
```

### Available Metrics

- SMTP command counters (HELO, MAIL FROM, RCPT TO, DATA, etc.)
- Connection tracking (opened, closed, active)
- Delivery attempts and outcomes (success, failure, retry)
- Queue statistics (size, age)
- Module events and validation results

### Grafana Dashboards

Pre-configured dashboards are automatically provisioned:

- **Empath MTA Overview** - High-level system metrics
  - SMTP throughput
  - Connection statistics
  - Delivery success rates
  - Queue health

### Prometheus Queries

Access Prometheus at http://localhost:9090 to run custom queries:

```promql
# SMTP commands per second
rate(smtp_commands_total[5m])

# Active connections
smtp_connections_active

# Delivery success rate
rate(delivery_success_total[5m]) / rate(delivery_attempts_total[5m])
```

## Testing the Stack

### Send a Test Email

```bash
# Using telnet
telnet localhost 1025
EHLO test.local
MAIL FROM:<sender@test.com>
RCPT TO:<recipient@example.com>
DATA
Subject: Test Email

This is a test email.
.
QUIT

# Using swaks (if installed)
swaks --to recipient@example.com --from sender@test.com --server localhost:1025
```

### View Module Logs

The FFI example modules log to stdout, viewable in the Empath container logs:

```bash
docker-compose -f docker/compose.dev.yml logs -f empath
```

You should see output from both modules:
- Example module: JSON-formatted transaction logs
- Event module: Connection and delivery event messages

## Troubleshooting

### Services Not Starting

```bash
# Check service status
docker-compose -f docker/compose.dev.yml ps

# View all logs
docker-compose -f docker/compose.dev.yml logs

# Restart specific service
docker-compose -f docker/compose.dev.yml restart <service-name>
```

### Metrics Not Appearing in Grafana

1. Check OTEL Collector is receiving metrics:
   ```bash
   curl http://localhost:8889/metrics
   ```

2. Verify Prometheus is scraping:
   - Visit http://localhost:9090/targets
   - Ensure OTEL exporter target is "UP"

3. Check Grafana datasource:
   - Visit http://localhost:3000/datasources
   - Test the Prometheus connection

### Port Conflicts

If port 1025, 3000, 9090, etc. are already in use:

1. Edit `docker/compose.dev.yml`
2. Change the port mapping (e.g., `"2025:1025"` for SMTP)
3. Restart the stack

## Production Deployment

For production deployments:

1. Use `Dockerfile` (production build) instead of `Dockerfile.dev`
2. Disable example FFI modules in configuration
3. Configure proper TLS certificates
4. Set appropriate resource limits in compose file
5. Use secrets management for sensitive configuration
6. Enable authentication on metrics endpoints (see TODO.md tasks 0.27, 0.28)

See the main repository `CLAUDE.md` for detailed deployment guidance.

## Development Workflow

Typical development cycle:

```bash
# 1. Start the stack
just docker-up

# 2. Make code changes in your editor

# 3. Rebuild and restart Empath
just docker-build
just docker-restart

# 4. View logs to test changes
just docker-logs

# 5. Check metrics in Grafana
just docker-grafana

# 6. Clean up when done
just docker-down
```

## Additional Resources

- [Empath Documentation](../CLAUDE.md)
- [OpenTelemetry Collector Docs](https://opentelemetry.io/docs/collector/)
- [Prometheus Documentation](https://prometheus.io/docs/)
- [Grafana Documentation](https://grafana.com/docs/)
