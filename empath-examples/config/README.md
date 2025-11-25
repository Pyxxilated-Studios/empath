# Configuration Examples

This directory contains example configurations for common deployment scenarios.

## Available Configurations

| File | Use Case | Features |
|------|----------|----------|
| `minimal.ron` | Quick testing | Bare minimum, no extensions |
| `development.ron` | Local development | SIZE extension, module support, test domains |
| `production.ron` | Production deployment | TLS, modules, monitoring, security hardening |

## Quick Start

```bash
# Copy example to project root
cp examples/config/minimal.ron empath.config.ron

# Run Empath
just run
```

---

## Configuration Breakdown

### minimal.ron

**Purpose:** Get started quickly with minimal configuration.

**Features:**
- Single SMTP listener on port 1025
- No extensions
- No modules
- Temporary spool directory
- Suitable for: Quick testing, CI/CD

**Usage:**
```bash
cp examples/config/minimal.ron empath.config.ron
just run
```

---

### development.ron

**Purpose:** Full-featured setup for local development.

**Features:**
- SIZE extension (10MB limit)
- STARTTLS support (commented out, requires certs)
- Module loading examples (commented out)
- Test domain MX override (test.example.com → localhost)
- Custom control socket path
- Suitable for: Local development, testing modules, integration tests

**Usage:**
```bash
cp examples/config/development.ron empath.config.ron

# Enable debug logging
RUST_LOG=debug just run
```

**Customization:**
1. Uncomment STARTTLS if you have test certificates
2. Uncomment modules after building them
3. Add more test domains to `domains` section

---

### production.ron

**Purpose:** Production-ready configuration with security hardening.

**Features:**
- Standard SMTP port (25)
- TLS with production certificates
- Multiple security modules (spam filter, rate limiter, DKIM)
- Persistent spool on dedicated partition
- Strict certificate validation
- Control socket with restricted permissions
- SMTP timeout configuration for outbound connections
- Suitable for: Production deployment, high-volume mail servers

**Usage:**
```bash
# Copy to system location
sudo cp examples/config/production.ron /etc/empath/empath.config.ron

# Review and customize
sudo vim /etc/empath/empath.config.ron

# Follow deployment checklist in comments
# ...

# Start service
sudo systemctl start empath
```

**Production Checklist:**

Before deploying to production, complete the checklist at the bottom of `production.ron`.

---

## Configuration Reference

### SMTP Controller

```ron
smtp_controller: (
    listeners: [
        {
            socket: "[::]:1025",           // IPv6/IPv4 bind address
            context: {                      // Custom key-value pairs
                "environment": "production",
            },
            timeouts: (                     // RFC 5321 compliant
                command_secs: 300,
                data_init_secs: 120,
                data_block_secs: 180,
                data_termination_secs: 600,
                connection_secs: 1800,
            ),
            extensions: [                   // SMTP extensions
                { "size": 10000000 },       // SIZE (bytes)
                {
                    "starttls": {           // STARTTLS
                        "key": "path/to/key.pem",
                        "certificate": "path/to/cert.pem",
                    }
                }
            ],
        },
    ],
),
```

**Timeouts Explained:**
- `command_secs`: Regular commands (EHLO, MAIL FROM, RCPT TO, etc.)
- `data_init_secs`: DATA command response wait time
- `data_block_secs`: Time between data chunks during message transmission
- `data_termination_secs`: Processing time after final `.` terminator
- `connection_secs`: Maximum session lifetime

### Modules

```ron
modules: [
    (
        type: "SharedLibrary",
        name: "./path/to/module.so",
        arguments: ["--flag", "value"],
    ),
],
```

**Module Types:**
- `SharedLibrary`: Dynamic library (.so, .dylib, .dll)

**Loading Order:**
Modules are loaded and called in the order listed.

### Spool

```ron
spool: (
    path: "/var/spool/empath",
),
```

**Requirements:**
- Directory must exist
- Write permissions for empath user
- Sufficient disk space for queue

### Delivery

```ron
delivery: (
    scan_interval_secs: 30,         // Spool scan frequency
    process_interval_secs: 10,      // Queue processing frequency
    max_attempts: 25,                // Max delivery attempts before permanent failure
    accept_invalid_certs: false,     // SECURITY: Global TLS cert validation

    // Per-domain configuration
    domains: {
        "example.com": (
            mx_override: "relay.example.com:25",  // Override MX lookup
            accept_invalid_certs: true,             // Per-domain override
        ),
    },

    // SMTP timeout configuration for outbound connections
    smtp_timeouts: (
        connect_secs: 30,
        ehlo_secs: 30,
        starttls_secs: 30,
        mail_from_secs: 30,
        rcpt_to_secs: 30,
        data_secs: 120,
        quit_secs: 10,
    ),
),
```

**Retry Schedule** (exponential backoff):
- Attempt 1: Immediate
- Attempt 2: 30 seconds
- Attempt 3: 2 minutes
- Attempt 4: 8 minutes
- Attempt 5: 30 minutes
- Attempt 6+: 1 hour each
- Max: 25 attempts

**Domain Configuration:**
- `mx_override`: Skip DNS MX lookup, use this server directly
- `accept_invalid_certs`: Per-domain TLS cert validation override

**⚠️ Security Warning:** Only set `accept_invalid_certs: true` for test environments!

### Control Socket

```ron
control_socket: "/tmp/empath.sock",
```

**Usage:**
```bash
# System status
./target/debug/empathctl --control-socket /tmp/empath.sock system status

# Queue management
./target/debug/empathctl --control-socket /tmp/empath.sock queue list

# DNS cache management
./target/debug/empathctl --control-socket /tmp/empath.sock dns list-cache
```

---

## Environment-Specific Configuration

### Local Development

```bash
# Use development config
cp examples/config/development.ron empath.config.ron

# Enable verbose logging
export RUST_LOG=debug

# Run
just run
```

### CI/CD

```bash
# Use minimal config for speed
cp examples/config/minimal.ron empath.config.ron

# Run tests
just ci
```

### Docker

```bash
# Docker config is in docker/empath.config.ron
# Start full stack
just docker-up
```

### Production

```bash
# Copy to system location
sudo cp examples/config/production.ron /etc/empath/empath.config.ron

# Customize for your environment
sudo vim /etc/empath/empath.config.ron

# Create systemd service
sudo cp examples/systemd/empath.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable empath
sudo systemctl start empath
```

---

## Validation

Validate your configuration before running:

```bash
# Check syntax (RON format)
# If config is invalid, you'll see a parse error on startup

# Test with minimal config first
cp examples/config/minimal.ron empath.config.ron
just run

# Then add features incrementally
```

---

## Troubleshooting

**Problem:** `Permission denied` on spool directory

**Solution:**
```bash
mkdir -p /var/spool/empath
chown empath:empath /var/spool/empath
chmod 700 /var/spool/empath
```

**Problem:** `Address already in use` on port 25

**Solution:**
```bash
# Check what's using port 25
sudo lsof -i :25

# Change port in config or stop conflicting service
```

**Problem:** TLS certificates not found

**Solution:**
```bash
# Check certificate paths
ls -la /etc/empath/tls/

# Ensure correct ownership
chown empath:empath /etc/empath/tls/*
chmod 600 /etc/empath/tls/private.key
chmod 644 /etc/empath/tls/certificate.crt
```

**Problem:** Module fails to load

**Solution:**
```bash
# Check module file exists
ls -la ./path/to/module.so

# Check library dependencies
ldd ./path/to/module.so

# Check empath library path
export LD_LIBRARY_PATH=./target/debug:$LD_LIBRARY_PATH
```

---

## Further Reading

- [CLAUDE.md](../../CLAUDE.md) - Complete configuration reference
- [docs/TROUBLESHOOTING.md](../../docs/TROUBLESHOOTING.md) - Common issues
- [docs/ARCHITECTURE.md](../../docs/ARCHITECTURE.md) - System architecture
