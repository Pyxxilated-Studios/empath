# Security Policy

This document describes the security features, threat model, and best practices for deploying and operating Empath MTA.

## Table of Contents

- [Vulnerability Reporting](#vulnerability-reporting)
- [Threat Model](#threat-model)
- [Security Features](#security-features)
  - [Transport Layer Security (TLS)](#transport-layer-security-tls)
  - [Timeouts and Resource Limits](#timeouts-and-resource-limits)
  - [Input Validation](#input-validation)
  - [Authentication and Authorization](#authentication-and-authorization)
  - [Audit Logging](#audit-logging)
  - [DNS Security](#dns-security)
- [Configuration Best Practices](#configuration-best-practices)
- [Known Limitations](#known-limitations)
- [Security Roadmap](#security-roadmap)

---

## Vulnerability Reporting

**We take security vulnerabilities seriously.** If you discover a security issue in Empath MTA, please report it responsibly.

### Reporting Process

1. **DO NOT** open a public GitHub issue for security vulnerabilities
2. Email security reports to: **security@empath-mta.org** (or create a private security advisory on GitHub)
3. Include:
   - Description of the vulnerability
   - Steps to reproduce
   - Potential impact
   - Suggested fix (if available)
4. Allow up to 48 hours for initial response
5. We will coordinate disclosure timeline with you

### Supported Versions

| Version | Supported          |
|---------|--------------------|
| 0.0.x   | :white_check_mark: |

**Note:** Empath is currently in early development (version 0.0.x). Security patches will be released as point releases.

---

## Threat Model

### Attack Surfaces

Empath MTA exposes the following attack surfaces:

1. **SMTP Server (Port 25/587/465)**
   - **Threat:** Malicious clients sending crafted SMTP commands
   - **Mitigation:** Input validation, state machine enforcement, timeouts
   - **Risk Level:** HIGH (exposed to internet)

2. **Control Socket (Unix Domain Socket)**
   - **Threat:** Unauthorized administrative access
   - **Mitigation:** Filesystem permissions (mode 0600) + optional token-based authentication
   - **Risk Level:** LOW (with authentication enabled) / MEDIUM (filesystem permissions only)

3. **SMTP Client (Outbound Delivery)**
   - **Threat:** Man-in-the-Middle attacks, certificate spoofing
   - **Mitigation:** TLS certificate validation (enabled by default)
   - **Risk Level:** MEDIUM (network-dependent)

4. **Dynamic Module Loading**
   - **Threat:** Malicious shared libraries loaded at runtime
   - **Mitigation:** File permissions, run as non-root user, code review
   - **Risk Level:** HIGH (if enabled)

5. **Spool Directory (Filesystem)**
   - **Threat:** Unauthorized access to message content
   - **Mitigation:** Filesystem permissions, encryption at rest (external)
   - **Risk Level:** HIGH (contains sensitive email data)

### Threat Scenarios

#### 1. Denial of Service (DoS)

**Scenario:** Attacker floods SMTP server with connections or sends malformed commands to exhaust resources.

**Mitigations:**
- **Connection timeouts:** Maximum 30-minute session lifetime (configurable)
- **Command timeouts:** RFC 5321 compliant timeouts (5 minutes per command)
- **Message size limits:** SIZE extension with configurable maximum (default: 25MB)
- **Queue size limits:** Health check fails when queue exceeds threshold (default: 10,000 messages)
- **State-aware timeouts:** Prevents slowloris attacks (3-minute timeout between data chunks)

**Residual Risk:** Application-layer DoS still possible. Recommend external rate limiting (iptables, fail2ban).

#### 2. Man-in-the-Middle (MITM)

**Scenario:** Attacker intercepts SMTP traffic to read or modify messages in transit.

**Mitigations:**
- **STARTTLS support:** Upgrade plaintext connections to TLS (RFC 3207)
- **Certificate validation:** All outbound connections validate certificates by default
- **TLS metadata logging:** Protocol version and cipher suite logged to context
- **Security warnings:** WARN-level logs when certificate validation is disabled

**Residual Risk:** Downgrade attacks if STARTTLS is optional. Recommend `require_tls` per-domain flag (roadmap).

#### 3. Email Spoofing / Spam Relay

**Scenario:** Attacker uses Empath as an open relay to send spam or spoofed messages.

**Mitigations:**
- **No authentication yet:** Empath does not currently implement SMTP AUTH (roadmap item)
- **Module system:** Implement SPF/DKIM validation via custom modules
- **IP-based restrictions:** Use firewall rules to limit SMTP access

**Residual Risk:** HIGH. Empath is **NOT** recommended for public-facing SMTP without authentication.

#### 4. Message Content Injection

**Scenario:** Attacker injects malicious headers or body content via crafted SMTP commands.

**Mitigations:**
- **SMTP command parsing:** Zero-allocation parsing with strict RFC 5321 compliance
- **ESMTP parameter validation:** Duplicate detection, type checking, SIZE=0 rejection
- **State machine enforcement:** Type-safe state transitions prevent invalid command sequences
- **Email address validation:** RFC 5321 compliant parsing via mailparse library

**Residual Risk:** LOW for SMTP protocol. Header injection still possible if modules don't validate content.

#### 5. Privilege Escalation via Control Socket

**Scenario:** Unprivileged user gains administrative access to MTA via control socket.

**Mitigations:**
- **Unix domain socket:** Local access only (no network exposure)
- **Restrictive permissions:** Mode 0600 (owner read/write only)
- **Token-based authentication:** Optional SHA-256 hashed bearer tokens (enabled by default in production)
- **Multiple token support:** Different access levels possible (admin vs read-only tokens)
- **Stale socket detection:** Prevents socket hijacking from crashed processes
- **Comprehensive audit logging:** All control commands logged with user/UID and authentication status

**Residual Risk:** LOW (with authentication enabled). Filesystem permissions alone provide MEDIUM protection.

---

## Security Features

### Transport Layer Security (TLS)

#### Server-Side TLS (SMTP Reception)

**Implementation:** `empath-smtp/src/connection.rs:116-171`

**Features:**
- **STARTTLS support:** Upgrades plaintext connections to TLS on-demand (RFC 3207)
- **TLS library:** tokio-rustls with no client authentication
- **Metadata preservation:** Connection context preserved across TLS upgrade
- **Protocol logging:** TLS version and cipher suite stored in context metadata

**Configuration Example:**
```ron
extensions: [
    {
        "starttls": {
            "key": "/etc/empath/tls/private.key",
            "certificate": "/etc/empath/tls/certificate.crt",
        }
    }
]
```

**Certificate Formats Supported:**
- PKCS#1
- PKCS#8
- SEC1

**Post-Upgrade Metadata:**
```json
{
  "tls": "true",
  "protocol": "TLSv1.3",
  "cipher": "TLS_AES_256_GCM_SHA384"
}
```

---

#### Client-Side TLS (Delivery)

**Implementation:** `empath-delivery/src/smtp_transaction.rs`

**Certificate Validation Policy:**

Empath uses a **two-tier configuration system** for certificate validation:

1. **Global Default (Secure):**
   ```ron
   delivery: (
       accept_invalid_certs: false,  // Validate all certificates
   )
   ```

2. **Per-Domain Override:**
   ```ron
   delivery: (
       accept_invalid_certs: false,

       domains: {
           "test.example.com": (
               accept_invalid_certs: true,   // Override for testing ONLY
           ),
           "secure.example.com": (
               accept_invalid_certs: false,  // Explicitly require valid certs
           ),
       },
   )
   ```

**Priority:** Per-domain setting > Global setting

**Security Warnings:**

When certificate validation is disabled, Empath logs:
```
WARN  SECURITY WARNING: TLS certificate validation is disabled for this connection
      domain=test.example.com server=192.168.1.100:25
```

**⚠️ Production Recommendation:**

**NEVER** set `accept_invalid_certs: true` in production environments. This setting is **ONLY** for:
- Local development with self-signed certificates
- Integration testing with mock SMTP servers
- Staging environments with internal CAs

**TLS Negotiation (RFC 3207 Compliant):**
- **Opportunistic TLS:** Attempts STARTTLS if advertised, gracefully falls back if fails
- **Required TLS:** Delivery fails if TLS cannot be negotiated (per-domain `require_tls` flag - roadmap)
- **Retry logic:** Reconnects without TLS if opportunistic STARTTLS fails (RFC 3207 Section 4.1)

---

### Timeouts and Resource Limits

#### Server-Side Timeouts (RFC 5321 Compliant)

**Implementation:** `empath-smtp/src/session/mod.rs`

Empath implements **state-aware timeouts** that follow RFC 5321 Section 4.5.3.2 recommendations:

**Configuration:**
```ron
timeouts: (
    command_secs: 300,          // 5 minutes - regular commands (EHLO, MAIL FROM, RCPT TO, etc.)
    data_init_secs: 120,        // 2 minutes - DATA command response
    data_block_secs: 180,       // 3 minutes - between data chunks during message reception
    data_termination_secs: 600, // 10 minutes - processing after final dot (.)
    connection_secs: 1800,      // 30 minutes - maximum total session lifetime
)
```

**How It Works:**

1. **State-aware selection:** Timeout automatically selected based on current SMTP state
   - `Reading` state: Uses `data_block_secs` (prevents slowloris attacks)
   - `Data` state: Uses `data_init_secs`
   - `PostDot` state: Uses `data_termination_secs`
   - All other states: Uses `command_secs`

2. **Connection lifetime enforcement:** Checked on every iteration of session loop

3. **Graceful shutdown:** Connection closed with timeout error when exceeded

**Security Benefits:**
- ✅ Prevents slowloris attacks (clients sending data very slowly)
- ✅ Prevents resource exhaustion from hung connections
- ✅ Mitigates DoS vulnerabilities from clients holding resources indefinitely
- ✅ Protects against misbehaving SMTP clients

---

#### Client-Side Timeouts (Delivery)

**Implementation:** `empath-delivery/src/types.rs`

**Configuration:**
```ron
smtp_timeouts: (
    connect_secs: 30,      // Initial connection establishment
    ehlo_secs: 30,         // EHLO/HELO commands
    starttls_secs: 30,     // STARTTLS command and TLS upgrade
    mail_from_secs: 30,    // MAIL FROM command
    rcpt_to_secs: 30,      // RCPT TO command (per recipient)
    data_secs: 120,        // DATA command and message transmission (longer for large messages)
    quit_secs: 10,         // QUIT command (best-effort after successful delivery)
)
```

**QUIT Timeout Behavior:**

Since QUIT occurs after successful delivery, timeout errors are logged but **do not** fail the delivery:
```rust
if let Err(e) = tokio::time::timeout(quit_timeout, client.quit()).await {
    tracing::warn!("QUIT command timed out after successful delivery: {e}");
}
```

---

#### Message Size Limits

Empath enforces message size limits at **three** validation points:

**1. SIZE Extension Advertisement (RFC 1870):**

```ron
extensions: [
    { "size": 25000000 }  // 25MB limit advertised to clients
]
```

**2. SIZE Parameter Validation (MAIL FROM):**

When client declares size via `MAIL FROM: <addr> SIZE=12345`:
- Validated against advertised maximum **before** accepting message
- Response: `552 5.2.3 Declared message size exceeds maximum`
- Location: `empath-ffi/src/modules/core.rs:51-67`

**3. Actual Message Size Validation (DATA):**

During message body reception:
- Checked **BEFORE** extending buffer (prevents overflow vulnerability)
- Uses `saturating_add` to prevent integer overflow on 32-bit systems
- Response: `552 Exceeded Storage Allocation`
- Location: `empath-smtp/src/session/io.rs:54-74`

**Security Note:** Validation BEFORE buffer extension prevents memory exhaustion attacks.

---

#### Queue Size Limits

**Implementation:** `empath-health/src/checker.rs`

Empath integrates with Kubernetes health checks to prevent queue overflow:

**Configuration:**
```ron
health: (
    max_queue_size: 10000,  // Readiness probe fails if queue exceeds this
)
```

**Behavior:**
1. Kubernetes readiness probe fails when queue size > threshold
2. Pod removed from service endpoints (stops accepting new traffic)
3. Auto-recovery when queue drains below threshold
4. Prevents accepting new traffic when system is overwhelmed

---

### Input Validation

#### SMTP Command Parsing

**Implementation:** `empath-smtp/src/command.rs`

**Security Features:**
- ✅ Case-insensitive command matching
- ✅ Zero-allocation prefix matching (performance)
- ✅ RFC 5321 compliant email address parsing (via mailparse library)
- ✅ ESMTP parameter parsing with validation
- ✅ State machine enforcement (type-safe transitions)

---

#### ESMTP Parameter Validation

**Implementation:** `empath-smtp/src/command.rs:64-122`

**Validation Rules:**

1. **Duplicate Detection:**
   - Same parameter cannot appear twice
   - Example: `MAIL FROM:<addr> SIZE=1000 SIZE=2000` → REJECTED

2. **SIZE Parameter Validation:**
   - Must be numeric
   - Cannot be zero (`SIZE=0` → REJECTED)
   - Case-insensitive (`size=`, `SIZE=`, `SiZe=` all valid)

3. **Performance:**
   - Perfect hash map for O(1) known parameter lookup
   - Known parameters: SIZE, BODY, AUTH, RET, ENVID, SMTPUTF8

**Example Validation:**
```rust
// Special validation for SIZE parameter
if key_normalized == "SIZE" {
    if let Ok(size_val) = value.parse::<usize>() {
        if size_val == 0 {
            return Err(String::from("SIZE=0 is not allowed"));
        }
    } else {
        return Err(format!("Invalid SIZE value: {value}"));
    }
}
```

**Test Coverage:**
- ✅ SIZE=0 rejection
- ✅ Malformed SIZE values
- ✅ Duplicate SIZE parameters
- ✅ Case-insensitive parsing

Location: `empath-smtp/src/command.rs:489-565`

---

### Authentication and Authorization

#### Control Socket Security

**Implementation:** `empath-control/src/server.rs`

Empath's control socket provides runtime management via Unix domain socket IPC.

**Security Features:**

1. **Unix Domain Socket (Local IPC Only):**
   - No network exposure by default
   - Path: `/tmp/empath.sock` (configurable)

2. **Restrictive Permissions:**
   ```rust
   #[cfg(unix)]
   {
       let mut perms = metadata.permissions();
       perms.set_mode(0o600); // Owner read/write only
       tokio::fs::set_permissions(&socket_path, perms).await?;
   }
   ```

3. **Stale Socket Detection:**
   - Tests if socket is active before binding
   - Removes stale sockets from crashed processes
   - Returns error if active instance already running

4. **Connection Timeout:**
   - 30 seconds per control request
   - Prevents resource exhaustion

**Multi-User Access (Production):**

For environments requiring group access:
```bash
# After Empath starts, set permissions:
chown empath:admin /var/run/empath.sock
chmod 660 /var/run/empath.sock  # Group read/write
```

5. **Token-Based Authentication (Optional):**
   ```ron
   control_auth: (
       enabled: true,
       token_hashes: [
           // SHA-256 hash of "your-secret-token"
           "4c5dc9b7708905f77f5e5d16316b5dfb425e68cb326dcd55a860e90a7707031e",
       ],
   )
   ```

**Implementation Details:**
- Tokens stored as SHA-256 hashes (not plaintext)
- Multiple tokens supported for different access levels
- Client sends plaintext token, server validates against hash
- Authentication failures logged with warnings

**Generating Tokens:**
```bash
# Generate secure token
TOKEN=$(openssl rand -hex 32)

# Generate hash for config
echo -n "$TOKEN" | sha256sum
```

**Client Usage:**
```rust
use empath_control::ControlClient;

let client = ControlClient::new("/tmp/empath.sock")
    .with_token("your-secret-token");

let response = client.send_request(request).await?;
```

**Deployment Recommendations:**
- ✅ Enable authentication in production multi-user environments
- ✅ Use strong cryptographically random tokens (32+ bytes)
- ✅ Rotate tokens periodically
- ✅ Use different tokens for different administrators
- ❌ Don't commit token hashes to version control
- ❌ Don't share tokens across environments

---

#### Metrics Endpoint Security

**Architecture:** Empath **pushes** metrics to OpenTelemetry Collector (no exposed HTTP endpoint).

**Configuration:**
```ron
metrics: (
    enabled: true,
    endpoint: "http://otel-collector:4318/v1/metrics",
    api_key: "your-metrics-api-key",  // Optional API key authentication
)
```

**Security Model:**
- Empath makes **outbound** connections only (no listening HTTP server)
- OTLP Collector can then expose to Prometheus
- Securing Prometheus/Grafana is deployment-specific

**API Key Authentication (Optional):**

When an API key is configured, Empath sends it in the `Authorization: Bearer <key>` header with all OTLP requests. The collector must be configured to validate the key.

**OTLP Collector Configuration Example:**
```yaml
# otel-collector-config.yaml
receivers:
  otlp:
    protocols:
      http:
        auth:
          authenticator: bearertokenauth

extensions:
  bearertokenauth:
    scheme: "Bearer"
    tokens:
      - token: "your-metrics-api-key"

service:
  extensions: [bearertokenauth]
  pipelines:
    metrics:
      receivers: [otlp]
      exporters: [prometheus]
```

**Security Considerations:**
- ⚠️ API key stored in **plaintext** in config (required for OTLP protocol)
- ✅ Use environment variable substitution for production:
  ```bash
  export METRICS_API_KEY="your-secret-key"
  # Reference in config: api_key: "$ENV:METRICS_API_KEY" (future)
  ```
- ✅ Or use Kubernetes secrets mounting
- ✅ Rotate API keys periodically
- ✅ Use unique keys per environment (dev/staging/prod)

**Deployment Recommendations:**
- Enable API key authentication for production deployments
- Use network segmentation as defense-in-depth
- Monitor collector logs for authentication failures
- Implement rate limiting at the collector level

---

### Audit Logging

#### Control Command Audit Trail

**Implementation:** `empath/src/control_handler.rs`

All control commands are **automatically logged** with structured data for accountability and compliance.

**Information Logged:**

| Field | Description | Example |
|-------|-------------|---------|
| **command** | Command type and details | `DNS:ClearCache` |
| **user** | User executing command (`$USER`) | `alice` |
| **uid** | User ID (Unix only) | `1000` |
| **timestamp** | Automatic via tracing | `2025-11-15T10:30:45Z` |
| **result** | Success or error details | `completed successfully` |

**Log Format Example:**
```
2025-11-15T10:30:45Z INFO  Control command: DNS user=alice uid=1000 command=ClearCache
2025-11-15T10:30:45Z INFO  DNS command completed successfully user=alice uid=1000
```

**Audit Events Covered:**
- DNS cache operations (clear, refresh, list, overrides)
- System status queries (ping, status)
- Queue management (list, view, delete, retry, stats, process-now)

**Security Benefits:**
- ✅ **Accountability:** Track who performed administrative actions
- ✅ **Forensics:** Investigate security incidents or configuration changes
- ✅ **Compliance:** Meet audit requirements for mail systems
- ✅ **Monitoring:** Detect unauthorized access attempts

**Log Configuration:**
```bash
# Enable audit logging
export RUST_LOG=empath=info

# For detailed audit trails, log to file
export RUST_LOG=empath_control=info,empath=info
```

---

#### SMTP Transaction Logging

**Implementation:** Throughout codebase via `tracing` framework

**Events Logged:**
- Connection opened/closed (peer address, duration)
- TLS upgrade events (protocol, cipher)
- Command reception (command, parameters)
- Validation failures (error, rejection reason)
- Spool operations (message ID, size)
- Error responses (4xx/5xx codes)

**Example:**
```
2025-11-15T10:32:15Z INFO  Connection opened peer=192.168.1.100:54321
2025-11-15T10:32:16Z INFO  TLS upgraded protocol=TLSv1.3 cipher=TLS_AES_256_GCM_SHA384
2025-11-15T10:32:18Z INFO  Message spooled id=01JCXYZ... size=4523 bytes
```

---

### DNS Security

**Implementation:** `empath-delivery/src/dns.rs`

#### DNS Cache with TTL Bounds

**Security Features:**

1. **TTL-Based Expiration:**
   - Respects DNS record TTLs from authoritative servers
   - Min TTL: 60 seconds (prevents excessive queries)
   - Max TTL: 3600 seconds (ensures eventual refresh)
   - Prevents stale cache poisoning

2. **Lock-Free Concurrent Caching:**
   - Uses DashMap for thread-safe access
   - No lock contention during lookups
   - Atomic TTL expiration checks

3. **MX Record Validation:**
   - Proper MX priority ordering (lower = higher priority)
   - Fallback to A/AAAA records if no MX
   - Port specification support

**Configuration:**
```ron
dns: (
    min_ttl_secs: 60,      // Minimum cache TTL (prevents query flood)
    max_ttl_secs: 3600,    // Maximum cache TTL (ensures freshness)
)
```

**Cache Management:**

Via control socket:
```bash
# View current cache with TTLs
empathctl dns list-cache

# Manually refresh domain
empathctl dns refresh example.com

# Clear entire cache (nuclear option)
empathctl dns clear-cache
```

**Known Limitations:**
- ⚠️ **DNSSEC validation not yet implemented** (see roadmap)
- Cache poisoning still possible via upstream resolver

---

## Configuration Best Practices

### Production Deployment Checklist

#### 1. TLS Configuration

**❌ INSECURE:**
```ron
delivery: (
    accept_invalid_certs: true,  // NEVER in production!
)
```

**✅ SECURE:**
```ron
delivery: (
    accept_invalid_certs: false,  // Validate all certificates

    domains: {
        // Only override for explicitly trusted internal domains
        "internal.company.com": (
            mx_override: "relay.company.com:25",
            accept_invalid_certs: false,  // Still require valid certs
        ),
    },
)
```

**TLS Certificate Setup:**
```bash
# Install certificates
sudo mkdir -p /etc/empath/tls
sudo cp server.key /etc/empath/tls/private.key
sudo cp server.crt /etc/empath/tls/certificate.crt
sudo chown -R empath:empath /etc/empath/tls
sudo chmod 600 /etc/empath/tls/private.key
sudo chmod 644 /etc/empath/tls/certificate.crt
```

---

#### 2. File System Permissions

**Spool Directory:**
```bash
# Create spool with restrictive permissions
sudo mkdir -p /var/spool/empath
sudo chown -R empath:empath /var/spool/empath
sudo chmod 700 /var/spool/empath  # Owner only
```

**Control Socket:**
```bash
# Create socket directory
sudo mkdir -p /var/run/empath
sudo chown empath:empath /var/run/empath
sudo chmod 755 /var/run/empath

# Socket permissions set automatically by Empath (0600)
# For multi-user access, adjust after startup:
sudo chown empath:admin /var/run/empath.sock
sudo chmod 660 /var/run/empath.sock
```

**Configuration File:**
```bash
# Protect config file (may contain sensitive paths)
sudo chown empath:empath /etc/empath/empath.config.ron
sudo chmod 600 /etc/empath/empath.config.ron
```

---

#### 3. Run as Non-Root User

**Create dedicated user:**
```bash
sudo useradd -r -s /bin/false -d /var/spool/empath empath
```

**Systemd Service (Example):**
```ini
[Unit]
Description=Empath MTA
After=network.target

[Service]
Type=simple
User=empath
Group=empath
ExecStart=/usr/local/bin/empath /etc/empath/empath.config.ron
Restart=on-failure
RestartSec=5s

# Security hardening
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=strict
ProtectHome=true
ReadWritePaths=/var/spool/empath /var/run/empath

[Install]
WantedBy=multi-user.target
```

---

#### 4. Network Security

**Firewall Rules (iptables example):**
```bash
# Allow SMTP only from trusted networks
sudo iptables -A INPUT -p tcp --dport 25 -s 192.168.1.0/24 -j ACCEPT
sudo iptables -A INPUT -p tcp --dport 25 -j DROP

# Rate limiting (optional)
sudo iptables -A INPUT -p tcp --dport 25 -m connlimit --connlimit-above 50 -j REJECT
sudo iptables -A INPUT -p tcp --dport 25 -m recent --set
sudo iptables -A INPUT -p tcp --dport 25 -m recent --update --seconds 60 --hitcount 20 -j DROP
```

**Recommendation:** Use fail2ban for automated IP blocking based on SMTP errors.

---

#### 5. Logging and Monitoring

**Log to File:**
```bash
# Systemd journal (automatic)
journalctl -u empath -f

# Or configure file logging via tracing-subscriber
export RUST_LOG=info
export RUST_LOG_FILE=/var/log/empath/empath.log
```

**Monitor for Security Events:**
```bash
# Watch for security warnings
journalctl -u empath | grep -i "security warning"

# Monitor failed deliveries (potential DoS or misconfiguration)
journalctl -u empath | grep -i "delivery failed"

# Track control socket access
journalctl -u empath | grep "Control command"
```

---

#### 6. Backup and Recovery

**Spool Backup:**
```bash
# Daily backup of spool directory
sudo tar -czf /backup/empath-spool-$(date +%Y%m%d).tar.gz /var/spool/empath

# Restore
sudo tar -xzf /backup/empath-spool-20250115.tar.gz -C /
```

**Queue State Backup:**
```bash
# Queue state is auto-saved to spool/queue_state.bin
# Backup includes delivery retry schedules
sudo cp /var/spool/empath/queue_state.bin /backup/queue_state-$(date +%Y%m%d).bin
```

---

## Known Limitations

### Current Security Gaps

1. **No SMTP Authentication (AUTH)**
   - **Impact:** Cannot restrict who can send mail
   - **Workaround:** Use IP-based firewall rules
   - **Roadmap:** Task 0.28 (high priority)
   - **Risk:** HIGH if exposed to untrusted networks

2. **No Token-Based Control Socket Authentication**
   - **Impact:** Control socket security relies on filesystem permissions only
   - **Workaround:** Restrict socket directory permissions
   - **Roadmap:** Task 0.13 (medium priority)
   - **Risk:** MEDIUM in multi-user environments

3. **No Rate Limiting Enforcement**
   - **Impact:** Configuration exists but not enforced per-domain
   - **Workaround:** Use external rate limiting (iptables, fail2ban)
   - **Roadmap:** Task 3.3 (medium priority)
   - **Risk:** MEDIUM (DoS via excessive connections)

4. **No DNSSEC Validation**
   - **Impact:** DNS queries not cryptographically verified
   - **Workaround:** Use trusted recursive resolvers
   - **Roadmap:** Task 1.2.1 (medium priority)
   - **Risk:** LOW (requires compromised DNS resolver)

5. **Module Loading Security**
   - **Impact:** Malicious shared libraries can execute arbitrary code
   - **Workaround:** Only load trusted modules, run as non-root
   - **Roadmap:** Module signature verification (long-term)
   - **Risk:** HIGH if untrusted modules loaded

6. **No SPF/DKIM/DMARC Validation**
   - **Impact:** Cannot validate sender authenticity
   - **Workaround:** Implement via custom modules
   - **Roadmap:** Phase 6 (DKIM signing task 6.2)
   - **Risk:** MEDIUM (phishing/spoofing potential)

---

## Security Roadmap

### Phase 1: Authentication (High Priority)

| Task | Description | Effort | Status |
|------|-------------|--------|--------|
| 0.27 | **Control socket token auth** | 2-3 days | **✅ Complete** |
| 0.28 | **Metrics endpoint API key** | 1-2 days | **✅ Complete** |
| - | SMTP AUTH support | 1 week | Planned |

### Phase 2: DNS Security (Medium Priority)

| Task | Description | Effort | Status |
|------|-------------|--------|--------|
| 1.2.1 | DNSSEC validation | 2-3 days | Planned |
| 0.14 | DNS logging and alerting | 1-2 days | Planned |

### Phase 3: Rate Limiting and Resource Control (Medium Priority)

| Task | Description | Effort | Status |
|------|-------------|--------|--------|
| 3.3 | Per-domain rate limiting | 2-3 days | Planned |
| 5.1 | Circuit breakers | 2-3 days | Planned |

### Phase 4: Email Security (Long-Term)

| Task | Description | Effort | Status |
|------|-------------|--------|--------|
| 6.2 | DKIM signing | 1 week | Planned |
| - | SPF validation | 3-5 days | Research |
| - | DMARC reporting | 1 week | Research |

### Phase 5: Module Security (Long-Term)

| Task | Description | Effort | Status |
|------|-------------|--------|--------|
| - | Module signature verification | 1-2 weeks | Research |
| - | Sandboxed module execution | 2-3 weeks | Research |

---

## Security Testing

### Automated Security Tests

Empath includes security-focused tests:

**1. SIZE Validation:**
- `empath-smtp/src/command.rs:489-565`
- Tests SIZE=0 rejection, malformed values, duplicate parameters

**2. Timeout Enforcement:**
- `empath-smtp/src/session/mod.rs:360-398`
- Tests connection lifetime, state-aware timeout selection

**3. TLS Integration:**
- `empath-smtp/tests/client_integration.rs`
- Tests STARTTLS upgrade, certificate validation

**4. Spool Security:**
- Cross-platform path validation (prevents Windows system directory creation)
- 16 comprehensive spool tests

### Manual Security Testing

**Recommended Tools:**

1. **SMTP Fuzzing:**
   ```bash
   # Test malformed commands
   echo "EHLO test\r\nMAIL FROM:<test@example.com> SIZE=abc\r\n" | nc localhost 25

   # Test oversized message
   dd if=/dev/zero bs=1M count=30 | nc localhost 25
   ```

2. **TLS Testing:**
   ```bash
   # Verify certificate validation
   openssl s_client -starttls smtp -connect localhost:25

   # Test with invalid cert (should fail in production)
   openssl s_client -starttls smtp -connect test.example.com:25
   ```

3. **Control Socket Security:**
   ```bash
   # Verify permissions
   ls -la /var/run/empath.sock
   # Should show: srw------- (0600)

   # Test unauthorized access (filesystem permissions)
   sudo -u nobody empathctl system status
   # Should fail with permission denied

   # Test authentication (if enabled)
   empathctl system status  # Without token - should fail
   empathctl --token "wrong-token" system status  # Invalid token - should fail
   empathctl --token "$VALID_TOKEN" system status  # Valid token - should succeed

   # Check authentication logs
   grep "authentication" /var/log/empath.log
   # Should show authentication events with user/UID
   ```

4. **Metrics Authentication:**
   ```bash
   # Monitor OTLP collector logs for auth failures
   docker logs otel-collector | grep "401\|403"

   # Test with invalid API key
   # (Modify config temporarily with wrong key, check collector logs)
   ```

---

## Additional Resources

- **CLAUDE.md:** Development guide with security implementation details
- **TODO.md:** Security roadmap and planned improvements
- **Production Config Example:** `examples/config/production.ron`
- **SMTP RFCs:** RFC 5321 (SMTP), RFC 3207 (STARTTLS), RFC 1870 (SIZE)
- **Security Standards:** OWASP Email Security Cheat Sheet

---

**Document Version:** 1.0
**Last Updated:** 2025-11-16
**Maintainer:** Empath MTA Project
