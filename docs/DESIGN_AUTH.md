# Authentication Infrastructure Design

**Status**: Design Phase
**Tasks**: 0.27 (Control Socket Auth) + 0.28 (Metrics Auth)
**Author**: Claude (Auto-generated)
**Date**: 2025-11-16

## Overview

This document outlines the authentication infrastructure for securing Empath's control socket and metrics endpoint.

## Requirements

### Control Socket (Task 0.27)
- **Current State**: Unix domain socket with filesystem permissions (mode 0600)
- **Gap**: No token-based authentication; any process with file access can send commands
- **Required**: Token-based authentication using bearer tokens
- **Use Case**: Multi-user systems where filesystem permissions aren't sufficient

### Metrics Endpoint (Task 0.28)
- **Current State**: OTLP metrics pushed to collector (http://localhost:4318/v1/metrics)
- **Gap**: No authentication on metrics export
- **Required**: API key authentication for metrics endpoint access
- **Use Case**: Restrict access to metrics data in production

## Design Principles

1. **Optional by Default**: Authentication should be opt-in to maintain development ergonomics
2. **Fail Secure**: When enabled, authentication failures must reject requests
3. **Configurable**: Tokens/keys via config file (not hardcoded)
4. **Minimal Performance Impact**: Auth checks should be fast
5. **Clear Audit Trail**: Log all authentication events (success and failure)

## Configuration Schema

### Control Socket Authentication

Add new optional field to `Empath` struct:

```rust
// In empath/src/controller.rs
pub struct Empath {
    // ... existing fields ...

    /// Control socket authentication configuration
    #[serde(alias = "control_auth", default)]
    control_auth: Option<ControlAuthConfig>,
}
```

New type in `empath-control`:

```rust
// In empath-control/src/auth.rs
use serde::Deserialize;

/// Authentication configuration for control socket
#[derive(Debug, Clone, Deserialize)]
pub struct ControlAuthConfig {
    /// Enable or disable authentication
    ///
    /// When disabled, all requests are allowed (relies on filesystem permissions)
    /// When enabled, requests must include valid bearer token
    #[serde(default)]
    pub enabled: bool,

    /// Valid bearer tokens (SHA-256 hashes for security)
    ///
    /// Tokens are hashed with SHA-256 before storage to prevent token leakage
    /// from config file dumps or backups.
    ///
    /// To generate a token hash:
    /// ```bash
    /// echo -n "your-secret-token" | sha256sum
    /// ```
    #[serde(default)]
    pub token_hashes: Vec<String>,
}

impl Default for ControlAuthConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            token_hashes: Vec::new(),
        }
    }
}
```

**Configuration Example**:

```ron
Empath(
    // ... existing config ...

    control_socket: "/tmp/empath.sock",

    // Optional: Enable control socket authentication
    control_auth: (
        enabled: true,
        token_hashes: [
            // SHA-256 hash of "admin-token-12345"
            "5e884898da28047151d0e56f8dc6292773603d0d6aabbdd62a11ef721d1542d8",
            // SHA-256 hash of "read-only-token"
            "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08",
        ],
    ),
)
```

### Metrics Authentication

Add new field to `MetricsConfig`:

```rust
// In empath-metrics/src/config.rs
#[derive(Debug, Clone, Deserialize)]
pub struct MetricsConfig {
    // ... existing fields ...

    /// Authentication configuration for metrics export
    #[serde(default)]
    pub auth: MetricsAuthConfig,
}

/// Authentication configuration for metrics endpoint
#[derive(Debug, Clone, Deserialize)]
pub struct MetricsAuthConfig {
    /// Enable or disable authentication
    ///
    /// When disabled, metrics are exported without authentication
    /// When enabled, OTLP requests must include API key header
    #[serde(default)]
    pub enabled: bool,

    /// Valid API key (SHA-256 hash)
    ///
    /// API key is hashed with SHA-256 before storage.
    /// The OTLP exporter will send the plaintext key in the header:
    /// `Authorization: Bearer <api-key>`
    ///
    /// To generate an API key hash:
    /// ```bash
    /// echo -n "your-api-key" | sha256sum
    /// ```
    #[serde(default)]
    pub api_key_hash: Option<String>,
}

impl Default for MetricsAuthConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            api_key_hash: None,
        }
    }
}
```

**Configuration Example**:

```ron
Empath(
    // ... existing config ...

    metrics: (
        enabled: true,
        endpoint: "http://localhost:4318/v1/metrics",

        // Optional: Enable metrics authentication
        auth: (
            enabled: true,
            // SHA-256 hash of "metrics-api-key-xyz"
            api_key_hash: "fcde2b2edba56bf408601fb721fe9b5c338d10ee429ea04fae5511b68fbf8fb9",
        ),
    ),
)
```

## Protocol Changes

### Control Socket Protocol

**Request Format** (with authentication):

```rust
// In empath-control/src/protocol.rs

/// Authenticated request wrapper
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthenticatedRequest {
    /// Bearer token (plaintext - validated against hashed config)
    pub token: Option<String>,

    /// The actual command request
    pub request: Request,
}
```

**Wire Protocol**:

1. Client sends: `AuthenticatedRequest { token: Some("admin-token-12345"), request: ... }`
2. Server validates token hash matches config
3. Server processes request or returns `Unauthorized` error

**Error Handling**:

```rust
// In empath-control/src/error.rs

#[derive(Debug, thiserror::Error)]
pub enum ControlError {
    // ... existing variants ...

    #[error("Authentication required but no token provided")]
    NoTokenProvided,

    #[error("Invalid authentication token")]
    InvalidToken,

    #[error("Authentication is disabled")]
    AuthDisabled,
}
```

### Metrics Protocol

**OTLP Headers**:

The OpenTelemetry exporter will include the API key in the Authorization header:

```
POST /v1/metrics HTTP/1.1
Host: localhost:4318
Authorization: Bearer metrics-api-key-xyz
Content-Type: application/x-protobuf
```

The OTLP collector (running outside Empath) will validate the API key.

**Configuration** (in Docker/Kubernetes):

```yaml
# docker-compose.yml
services:
  otel-collector:
    # ...
    environment:
      - API_KEY_HASH=fcde2b2edba56bf408601fb721fe9b5c338d10ee429ea04fae5511b68fbf8fb9
```

## Implementation Plan

### Phase 1: Control Socket Authentication (Task 0.27)

**Files to Create**:
1. `empath-control/src/auth.rs` - Authentication types and validation logic
2. `empath-control/src/middleware.rs` - Authentication middleware for request handling

**Files to Modify**:
1. `empath-control/src/lib.rs` - Export new auth types
2. `empath-control/src/server.rs` - Integrate auth middleware
3. `empath-control/src/protocol.rs` - Add `AuthenticatedRequest` type
4. `empath-control/src/error.rs` - Add auth error variants
5. `empath-control/src/client.rs` - Support token in requests
6. `empath/src/controller.rs` - Add `control_auth` field
7. `empath/bin/empathctl.rs` - Support `--token` flag

**Validation Logic**:

```rust
// In empath-control/src/auth.rs

use sha2::{Sha256, Digest};
use hex::encode;

impl ControlAuthConfig {
    /// Validate a bearer token against configured hashes
    pub fn validate_token(&self, token: &str) -> bool {
        if !self.enabled {
            return true; // Auth disabled, allow all
        }

        // Hash the provided token
        let mut hasher = Sha256::new();
        hasher.update(token.as_bytes());
        let hash = encode(hasher.finalize());

        // Check if hash matches any configured hash
        self.token_hashes.iter().any(|h| h == &hash)
    }

    /// Check if authentication is required
    pub fn requires_auth(&self) -> bool {
        self.enabled
    }
}
```

### Phase 2: Metrics Authentication (Task 0.28)

**Files to Modify**:
1. `empath-metrics/src/config.rs` - Add `MetricsAuthConfig`
2. `empath-metrics/src/lib.rs` - Configure OTLP exporter with auth header
3. `Cargo.toml` - Add `sha2` and `hex` dependencies

**OTLP Configuration**:

```rust
// In empath-metrics/src/lib.rs

use opentelemetry_otlp::WithExportConfig;

pub fn init_metrics(config: &MetricsConfig) -> Result<(), MetricsError> {
    if !config.enabled {
        return Ok(());
    }

    let mut exporter = opentelemetry_otlp::new_exporter()
        .http()
        .with_endpoint(&config.endpoint);

    // Add authentication header if enabled
    if config.auth.enabled {
        if let Some(api_key) = &config.auth.api_key_hash {
            // Note: We send the *plaintext* key, not the hash
            // The config stores the hash for validation at the collector
            let headers = HashMap::from([
                ("authorization".to_string(), format!("Bearer {}", api_key)),
            ]);
            exporter = exporter.with_headers(headers);
        }
    }

    // ... rest of exporter setup ...
}
```

**Note**: The metrics auth is simpler because validation happens at the OTLP collector, not in Empath. Empath just needs to send the API key.

## Security Considerations

### Token/Key Storage

**Problem**: Storing plaintext tokens in config is insecure
**Solution**: Store SHA-256 hashes; validate by hashing incoming tokens

**Trade-offs**:
- ✅ Config file leaks don't expose tokens
- ✅ Backups/logs don't contain secrets
- ❌ Requires external tool to generate hashes
- ❌ Cannot rotate tokens without config change

### Token Generation Best Practices

**Recommendations**:
1. Use cryptographically secure random tokens (32+ bytes)
2. Generate tokens outside Empath (e.g., `openssl rand -hex 32`)
3. Hash tokens before adding to config
4. Rotate tokens periodically (manual process)

**Example Workflow**:

```bash
# Generate secure token
TOKEN=$(openssl rand -hex 32)
echo "Token: $TOKEN"

# Generate hash for config
HASH=$(echo -n "$TOKEN" | sha256sum | awk '{print $1}')
echo "Hash for config: $HASH"

# Add hash to empath.config.ron
# Use TOKEN when calling empathctl
empathctl --token "$TOKEN" system status
```

### Audit Logging

**All authentication events are logged**:

```rust
// In empath-control/src/server.rs

// Success
tracing::info!(
    user = %user,
    uid = %uid,
    authenticated = true,
    "Control socket authentication successful"
);

// Failure
tracing::warn!(
    client_addr = ?client_addr,
    authenticated = false,
    reason = "invalid token",
    "Control socket authentication failed"
);
```

### Environment Variables (Alternative)

**Future Enhancement**: Support environment variable for tokens to avoid config file storage

```ron
control_auth: (
    enabled: true,
    token_hashes: [
        "$ENV:EMPATH_ADMIN_TOKEN_HASH",
    ],
)
```

This would be implemented in a future task.

## Testing Strategy

### Unit Tests

1. **Token Validation**:
   - Valid token → authentication succeeds
   - Invalid token → authentication fails
   - Auth disabled → all requests allowed
   - Empty token list → all requests denied

2. **Hash Generation**:
   - Same input → same hash
   - Different input → different hash

### Integration Tests

1. **Control Socket**:
   - Authenticated request with valid token → command executes
   - Authenticated request with invalid token → error
   - Unauthenticated request when auth enabled → error
   - Unauthenticated request when auth disabled → command executes

2. **Metrics**:
   - Export with valid API key → metrics sent
   - Export with invalid API key → collector rejects (tested externally)

### E2E Tests

1. Start Empath with auth enabled
2. Send empathctl command with token → succeeds
3. Send empathctl command without token → fails
4. Check audit logs for authentication events

## Performance Impact

**Expected Overhead**:
- SHA-256 hashing: ~1-2µs per request (negligible)
- Token comparison: O(n) where n = number of valid tokens (typically < 10)
- Total auth overhead: < 10µs per request

**Measurement**: Add tracing span around auth validation to measure actual impact.

## Rollout Plan

1. **Development**: Implement authentication, default disabled
2. **Testing**: Comprehensive unit + integration tests
3. **Documentation**: Update CLAUDE.md, SECURITY.md, README.md
4. **Release**: Announce in changelog with migration guide
5. **Adoption**: Recommend enabling in production deployments

## Future Enhancements

**Beyond This Task**:

1. **Token Rotation**: Automatic token rotation with grace period
2. **RBAC**: Role-based access control (admin vs read-only tokens)
3. **Token Expiry**: Time-based token expiration
4. **OAuth/OIDC**: Integration with external identity providers
5. **mTLS**: Mutual TLS for control socket
6. **Audit Log Export**: Structured audit logs to SIEM

These are tracked in TODO.md as separate tasks.

## Success Criteria

### Task 0.27 (Control Socket Auth)
- ✅ `ControlAuthConfig` type with SHA-256 token hashing
- ✅ Token validation in control socket server
- ✅ `empathctl --token` flag for authentication
- ✅ Audit logging of all auth events
- ✅ Configuration example in docs
- ✅ Unit + integration tests

### Task 0.28 (Metrics Auth)
- ✅ `MetricsAuthConfig` type
- ✅ OTLP exporter configured with Authorization header
- ✅ Configuration example in docs
- ✅ Documentation of collector-side validation
- ✅ Unit tests for config loading

## References

- **RFC 6750**: OAuth 2.0 Bearer Token Usage (inspiration for token format)
- **OWASP**: Secure token storage best practices
- **OpenTelemetry**: OTLP Authentication specification
- **SHA-256**: NIST FIPS 180-4 standard
