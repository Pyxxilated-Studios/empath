//! Domain-specific policy resolution
//!
//! Provides a clean abstraction for resolving domain-specific configuration
//! with fallback to global defaults. Encapsulates the logic for:
//! - MX server overrides (for testing/routing)
//! - TLS requirements (per-domain compliance)
//! - Certificate validation policies (with security-conscious defaults)

use std::sync::{Arc, RwLock};

use crate::dns::MailServer;
use crate::domain_config::DomainConfigRegistry;
use crate::policy::TlsPolicy;

/// Resolver for domain-specific delivery policies
///
/// Encapsulates the logic for looking up domain-specific configuration
/// and falling back to global defaults when no domain-specific config exists.
///
/// # Design Goals
///
/// - **Single Responsibility**: Only handles policy resolution, not enforcement
/// - **Testability**: Pure lookups, easy to test without full processor setup
/// - **Clarity**: Explicit methods for each policy dimension
#[derive(Debug, Clone)]
pub struct DomainPolicyResolver {
    /// Registry of per-domain configurations
    domains: DomainConfigRegistry,

    /// Global defaults for policy resolution.
    defaults: Arc<RwLock<DomainPolicyDefaults>>,
}

#[derive(Debug, Clone, Copy)]
struct DomainPolicyDefaults {
    accept_invalid_certs: bool,
    tls_policy: TlsPolicy,
}

impl DomainPolicyResolver {
    /// Create a new policy resolver
    ///
    /// # Arguments
    ///
    /// * `domains` - Registry of per-domain configurations
    /// * `global_accept_invalid_certs` - Global default for certificate validation
    /// * `global_tls_policy` - Global default for TLS policy
    #[must_use]
    pub fn new(
        domains: DomainConfigRegistry,
        global_accept_invalid_certs: bool,
        global_tls_policy: TlsPolicy,
    ) -> Self {
        Self {
            domains,
            defaults: Arc::new(RwLock::new(DomainPolicyDefaults {
                accept_invalid_certs: global_accept_invalid_certs,
                tls_policy: global_tls_policy,
            })),
        }
    }

    pub fn update_defaults(&self, accept_invalid_certs: bool, tls_policy: TlsPolicy) {
        if let Ok(mut guard) = self.defaults.write() {
            guard.accept_invalid_certs = accept_invalid_certs;
            guard.tls_policy = tls_policy;
        }
    }

    /// Resolve MX server override for a domain
    ///
    /// Returns `Some(MailServer)` if the domain has an MX override configured,
    /// otherwise `None` to indicate DNS lookup should be performed.
    ///
    /// # Arguments
    ///
    /// * `domain` - The recipient domain to check
    ///
    /// # Returns
    ///
    /// `Some(MailServer)` with the override configuration, or `None` for DNS lookup
    #[must_use]
    pub fn resolve_mx_override(&self, domain: &str) -> Option<MailServer> {
        self.domains
            .get(domain)
            .and_then(|config| config.mx_override_address().map(parse_mx_override))
    }

    /// Check if TLS is required for a domain
    ///
    /// Returns `true` if the domain has `require_tls` set, otherwise `false`.
    ///
    /// # Arguments
    ///
    /// * `domain` - The recipient domain to check
    ///
    /// # Returns
    ///
    /// `true` if TLS is required, `false` otherwise
    #[must_use]
    pub fn requires_tls(&self, domain: &str) -> bool {
        self.domains
            .get(domain)
            .is_some_and(|config| config.require_tls)
    }

    /// Determine if invalid certificates should be accepted for a domain
    ///
    /// Resolution order:
    /// 1. Per-domain override (if `Some`)
    /// 2. Global configuration default
    ///
    /// # Arguments
    ///
    /// * `domain` - The recipient domain to check
    ///
    /// # Returns
    ///
    /// `true` if invalid certificates should be accepted, `false` otherwise
    ///
    /// # Security
    ///
    /// **WARNING**: Accepting invalid certificates disables TLS validation
    /// and makes connections vulnerable to man-in-the-middle attacks.
    /// Only use for testing with self-signed certificates.
    #[must_use]
    pub fn accepts_invalid_certs(&self, domain: &str) -> bool {
        self.domains
            .get(domain)
            .and_then(|config| config.accept_invalid_certs)
            .unwrap_or_else(|| {
                self.defaults
                    .read()
                    .map(|defaults| defaults.accept_invalid_certs)
                    .unwrap_or(false)
            })
    }

    /// Resolve the TLS policy for a domain.
    #[must_use]
    pub fn tls_policy(&self, domain: &str) -> TlsPolicy {
        if let Some(config) = self.domains.get(domain) {
            if let Some(policy) = config.tls_policy {
                return policy;
            }
            if config.require_tls {
                return TlsPolicy::Required;
            }
        }

        self.defaults
            .read()
            .map(|defaults| defaults.tls_policy)
            .unwrap_or_default()
    }

    /// Resolve outbound binding override for a domain.
    ///
    /// Returns `Some(binding_name)` if configured, otherwise `None`.
    #[must_use]
    pub fn binding_override(&self, domain: &str) -> Option<String> {
        self.domains
            .get(domain)
            .and_then(|config| config.binding.clone())
    }
}

/// Parse MX override string into `MailServer`
///
/// Accepts formats:
/// - `"hostname:port"` - explicit host and port
/// - `"hostname"` - uses default port 25
///
/// # Arguments
///
/// * `mx_override` - The MX override string from configuration
///
/// # Returns
///
/// `MailServer` with parsed host and port
fn parse_mx_override(mx_override: &str) -> MailServer {
    let (host, port) = if let Some((h, p)) = mx_override.split_once(':') {
        (h.to_string(), p.parse::<u16>().unwrap_or(25))
    } else {
        (mx_override.to_string(), 25)
    };

    MailServer {
        host,
        port,
        priority: 0, // Override servers have highest priority
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain_config::DomainConfig;

    #[test]
    fn test_parse_mx_override_with_port() {
        let server = parse_mx_override("localhost:1025");
        assert_eq!(server.host, "localhost");
        assert_eq!(server.port, 1025);
        assert_eq!(server.priority, 0);
    }

    #[test]
    fn test_parse_mx_override_without_port() {
        let server = parse_mx_override("mail.example.com");
        assert_eq!(server.host, "mail.example.com");
        assert_eq!(server.port, 25);
        assert_eq!(server.priority, 0);
    }

    #[test]
    fn test_parse_mx_override_invalid_port() {
        let server = parse_mx_override("localhost:invalid");
        assert_eq!(server.host, "localhost");
        assert_eq!(server.port, 25); // Falls back to default
    }

    #[test]
    fn test_resolve_mx_override_configured() {
        let registry = DomainConfigRegistry::new();
        registry.insert(
            "test.example.com".to_string(),
            DomainConfig {
                mx_override: Some("localhost:1025".to_string()),
                ..Default::default()
            },
        );

        let resolver = DomainPolicyResolver::new(registry, false, TlsPolicy::default());
        let server = resolver.resolve_mx_override("test.example.com");

        assert!(server.is_some(), "Expected MX override to be configured");
        if let Some(server) = server {
            assert_eq!(server.host, "localhost");
            assert_eq!(server.port, 1025);
        }
    }

    #[test]
    fn test_resolve_mx_override_not_configured() {
        let registry = DomainConfigRegistry::new();
        let resolver = DomainPolicyResolver::new(registry, false, TlsPolicy::default());

        assert!(resolver.resolve_mx_override("example.com").is_none());
    }

    #[test]
    fn test_requires_tls_enabled() {
        let registry = DomainConfigRegistry::new();
        registry.insert(
            "secure.example.com".to_string(),
            DomainConfig {
                require_tls: true,
                ..Default::default()
            },
        );

        let resolver = DomainPolicyResolver::new(registry, false, TlsPolicy::default());
        assert!(resolver.requires_tls("secure.example.com"));
    }

    #[test]
    fn test_requires_tls_disabled() {
        let registry = DomainConfigRegistry::new();
        registry.insert(
            "example.com".to_string(),
            DomainConfig {
                require_tls: false,
                ..Default::default()
            },
        );

        let resolver = DomainPolicyResolver::new(registry, false, TlsPolicy::default());
        assert!(!resolver.requires_tls("example.com"));
    }

    #[test]
    fn test_requires_tls_not_configured() {
        let registry = DomainConfigRegistry::new();
        let resolver = DomainPolicyResolver::new(registry, false, TlsPolicy::default());

        // No domain config = default to false
        assert!(!resolver.requires_tls("example.com"));
    }

    #[test]
    fn test_accepts_invalid_certs_domain_override_true() {
        let registry = DomainConfigRegistry::new();
        registry.insert(
            "test.local".to_string(),
            DomainConfig {
                accept_invalid_certs: Some(true),
                ..Default::default()
            },
        );

        let resolver = DomainPolicyResolver::new(registry, false, TlsPolicy::default());
        assert!(resolver.accepts_invalid_certs("test.local"));
    }

    #[test]
    fn test_accepts_invalid_certs_domain_override_false() {
        let registry = DomainConfigRegistry::new();
        registry.insert(
            "secure.example.com".to_string(),
            DomainConfig {
                accept_invalid_certs: Some(false),
                ..Default::default()
            },
        );

        // Global is true, but domain overrides to false
        let resolver = DomainPolicyResolver::new(registry, true, TlsPolicy::default());
        assert!(!resolver.accepts_invalid_certs("secure.example.com"));
    }

    #[test]
    fn test_accepts_invalid_certs_fallback_to_global() {
        let registry = DomainConfigRegistry::new();
        registry.insert(
            "example.com".to_string(),
            DomainConfig {
                accept_invalid_certs: None, // No override
                ..Default::default()
            },
        );

        // Should use global default
        let resolver_false =
            DomainPolicyResolver::new(registry.clone(), false, TlsPolicy::default());
        assert!(!resolver_false.accepts_invalid_certs("example.com"));

        let resolver_true = DomainPolicyResolver::new(registry, true, TlsPolicy::default());
        assert!(resolver_true.accepts_invalid_certs("example.com"));
    }

    #[test]
    fn test_accepts_invalid_certs_no_domain_config() {
        let registry = DomainConfigRegistry::new();

        // No domain config at all, use global
        let resolver_false =
            DomainPolicyResolver::new(registry.clone(), false, TlsPolicy::default());
        assert!(!resolver_false.accepts_invalid_certs("unconfigured.example.com"));

        let resolver_true = DomainPolicyResolver::new(registry, true, TlsPolicy::default());
        assert!(resolver_true.accepts_invalid_certs("unconfigured.example.com"));
    }
}
