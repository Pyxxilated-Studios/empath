# Empath MTA Examples

This directory contains practical examples demonstrating how to use and extend Empath MTA.

## Quick Start

1. **Send an email**: [smtp-client/send_email.sh](#smtp-client-example)
2. **Configure Empath**: [config/](#configuration-examples)
3. **Write a plugin**: [modules/](#module-examples)

---

## SMTP Client Example

**Directory:** `smtp-client/`

Demonstrates how to send email through Empath using a basic SMTP client.

```bash
# Start Empath (terminal 1)
just run

# Send test email (terminal 2)
./examples/smtp-client/send_email.sh
```

See [smtp-client/README.md](smtp-client/README.md) for details.

---

## Configuration Examples

**Directory:** `config/`

Pre-configured examples for common deployment scenarios:

| Example | Description | Use Case |
|---------|-------------|----------|
| `minimal.ron` | Bare minimum configuration | Quick testing |
| `development.ron` | Full-featured with modules | Local development |
| `production.ron` | Production-ready setup | Deployment |
| `docker.ron` | Docker-optimized | Containers |

### Quick Start

```bash
# Copy example config
cp examples/config/minimal.ron empath.config.ron

# Run with example config
just run
```

See [config/README.md](config/README.md) for detailed explanations.

---

## Module Examples

**Directory:** `modules/` (references `empath-ffi/examples/`)

Learn how to extend Empath with custom plugins:

| Module | Language | Purpose |
|--------|----------|---------|
| `spam_filter.c` | C | Example spam filtering |
| `rate_limiter.c` | C | Rate limiting by IP |
| `auth_check.c` | C | Authentication validation |

### Building Modules

```bash
# Build Empath library first
just build

# Build example module
cd examples/modules
gcc spam_filter.c -fpic -shared -o libspam_filter.so -I../../target -L../../target/debug -lempath

# Configure Empath to load it
# Add to empath.config.ron:
# modules: [
#     (type: "SharedLibrary", name: "./examples/modules/libspam_filter.so"),
# ]
```

See [modules/README.md](modules/README.md) for the complete module development guide.

---

## Example Scenarios

### Scenario 1: Local Development

**Goal:** Test SMTP functionality locally

```bash
# 1. Use development config
cp examples/config/development.ron empath.config.ron

# 2. Start Empath
just run

# 3. Send test email
./examples/smtp-client/send_email.sh

# 4. Check queue
just queue-list

# 5. View logs
# RUST_LOG=debug already set in development.ron
```

### Scenario 2: Docker Deployment

**Goal:** Run Empath in Docker with observability

```bash
# 1. Start full stack (uses docker.ron config)
just docker-up

# 2. Send email via Docker
just docker-test-email

# 3. View in Grafana
just docker-grafana  # http://localhost:3000 (admin/admin)

# 4. Check metrics
# Prometheus: http://localhost:9090
```

### Scenario 3: Custom Spam Filter

**Goal:** Add spam filtering via custom module

```bash
# 1. Build Empath
just build

# 2. Build spam filter module
cd examples/modules
make spam-filter  # Uses provided Makefile

# 3. Configure Empath to load module
# Edit empath.config.ron:
# modules: [
#     (type: "SharedLibrary", name: "./examples/modules/libspam_filter.so"),
# ]

# 4. Run and test
just run
./examples/smtp-client/send_spam.sh  # Should be rejected
```

---

## Testing Examples

All examples include test scripts:

```bash
# Test SMTP client
cd examples/smtp-client
./test.sh

# Test configurations
cd examples/config
./validate_all.sh

# Test modules
cd examples/modules
./test_modules.sh
```

---

## Further Reading

- [docs/ONBOARDING.md](../docs/ONBOARDING.md) - New developer guide
- [docs/ARCHITECTURE.md](../docs/ARCHITECTURE.md) - System architecture
- [CONTRIBUTING.md](../CONTRIBUTING.md) - Contribution guidelines
- [CLAUDE.md](../CLAUDE.md) - Detailed implementation guide

---

## Contributing Examples

Have a useful example? We'd love to include it!

1. Create your example in the appropriate directory
2. Add a README explaining what it does
3. Include a test script
4. Submit a pull request

See [CONTRIBUTING.md](../CONTRIBUTING.md) for guidelines.
