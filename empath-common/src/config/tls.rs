//! TLS configuration for SMTP connections.
//!
//! This module consolidates TLS policy and certificate validation settings
//! that were previously scattered across domain-specific and global configs.

use serde::{Deserialize, Serialize};

/// TLS policy for SMTP connections.
///
/// Defines how TLS should be negotiated when connecting to an SMTP server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TlsPolicy {
    /// Attempt TLS via STARTTLS, fall back to plaintext if unavailable.
    ///
    /// This is the default and recommended policy for most deployments.
    /// Follows RFC 3207 Section 4.1: if STARTTLS fails, reconnect without TLS.
    #[default]
    Opportunistic,

    /// Require TLS via STARTTLS, fail delivery if unavailable.
    ///
    /// Use this for domains that require encryption (e.g., healthcare, finance).
    /// Delivery will fail with a permanent error if TLS cannot be established.
    Required,

    /// Never use TLS, always use plaintext.
    ///
    /// **WARNING**: Only use for testing or explicitly non-sensitive domains.
    /// Not recommended for production use.
    Disabled,
}

/// TLS certificate validation policy.
///
/// Controls whether to accept invalid or self-signed certificates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct TlsCertificatePolicy {
    /// Whether to accept invalid TLS certificates (self-signed, expired, etc.).
    ///
    /// **SECURITY WARNING**: Setting this to `true` disables certificate validation
    /// and makes the connection vulnerable to man-in-the-middle attacks.
    ///
    /// Only set to `true` for testing with self-signed certificates.
    ///
    /// Default: `false` (validate certificates)
    #[serde(default)]
    pub accept_invalid_certs: bool,
}

/// Complete TLS configuration for an SMTP connection.
///
/// Combines policy (when to use TLS) with certificate validation settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct TlsConfig {
    /// TLS negotiation policy.
    ///
    /// Default: `Opportunistic`
    #[serde(default)]
    pub policy: TlsPolicy,

    /// Certificate validation policy.
    ///
    /// Default: `accept_invalid_certs = false`
    #[serde(default)]
    pub certificate: TlsCertificatePolicy,
}

impl TlsConfig {
    /// Create a new TLS configuration with opportunistic policy and valid certificates required.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            policy: TlsPolicy::Opportunistic,
            certificate: TlsCertificatePolicy {
                accept_invalid_certs: false,
            },
        }
    }

    /// Create a TLS configuration that requires TLS.
    #[must_use]
    pub const fn required() -> Self {
        Self {
            policy: TlsPolicy::Required,
            certificate: TlsCertificatePolicy {
                accept_invalid_certs: false,
            },
        }
    }

    /// Create a TLS configuration that disables TLS.
    ///
    /// **WARNING**: Only use for testing or explicitly non-sensitive domains.
    #[must_use]
    pub const fn disabled() -> Self {
        Self {
            policy: TlsPolicy::Disabled,
            certificate: TlsCertificatePolicy {
                accept_invalid_certs: false,
            },
        }
    }

    /// Create a TLS configuration for testing with self-signed certificates.
    ///
    /// **WARNING**: Only use in test environments. Do not use in production.
    #[must_use]
    pub const fn insecure() -> Self {
        Self {
            policy: TlsPolicy::Opportunistic,
            certificate: TlsCertificatePolicy {
                accept_invalid_certs: true,
            },
        }
    }

    /// Returns `true` if TLS is required and delivery should fail if unavailable.
    #[must_use]
    pub const fn is_required(&self) -> bool {
        matches!(self.policy, TlsPolicy::Required)
    }

    /// Returns `true` if TLS should never be used.
    #[must_use]
    pub const fn is_disabled(&self) -> bool {
        matches!(self.policy, TlsPolicy::Disabled)
    }

    /// Returns `true` if invalid certificates should be accepted.
    ///
    /// **SECURITY WARNING**: This indicates certificate validation is disabled.
    #[must_use]
    pub const fn accepts_invalid_certs(&self) -> bool {
        self.certificate.accept_invalid_certs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tls_policy_default() {
        let policy = TlsPolicy::default();
        assert_eq!(policy, TlsPolicy::Opportunistic);
    }

    #[test]
    fn test_tls_certificate_policy_default() {
        let policy = TlsCertificatePolicy::default();
        assert!(!policy.accept_invalid_certs);
    }

    #[test]
    fn test_tls_config_default() {
        let config = TlsConfig::default();
        assert_eq!(config.policy, TlsPolicy::Opportunistic);
        assert!(!config.certificate.accept_invalid_certs);
    }

    #[test]
    fn test_tls_config_new() {
        let config = TlsConfig::new();
        assert_eq!(config.policy, TlsPolicy::Opportunistic);
        assert!(!config.accepts_invalid_certs());
    }

    #[test]
    fn test_tls_config_required() {
        let config = TlsConfig::required();
        assert_eq!(config.policy, TlsPolicy::Required);
        assert!(config.is_required());
        assert!(!config.is_disabled());
        assert!(!config.accepts_invalid_certs());
    }

    #[test]
    fn test_tls_config_disabled() {
        let config = TlsConfig::disabled();
        assert_eq!(config.policy, TlsPolicy::Disabled);
        assert!(!config.is_required());
        assert!(config.is_disabled());
    }

    #[test]
    fn test_tls_config_insecure() {
        let config = TlsConfig::insecure();
        assert_eq!(config.policy, TlsPolicy::Opportunistic);
        assert!(config.accepts_invalid_certs());
    }

    #[test]
    fn test_tls_policy_clone_and_equality() {
        let policy = TlsPolicy::Required;
        let cloned = policy;
        assert_eq!(policy, cloned);
    }

    #[test]
    fn test_tls_config_clone_and_equality() {
        let config = TlsConfig {
            policy: TlsPolicy::Required,
            certificate: TlsCertificatePolicy {
                accept_invalid_certs: true,
            },
        };
        let cloned = config;
        assert_eq!(config.policy, cloned.policy);
        assert_eq!(
            config.certificate.accept_invalid_certs,
            cloned.certificate.accept_invalid_certs
        );
    }
}
