//! Per-domain delivery configuration
//!
//! Allows customizing delivery behavior for specific recipient domains:
//! - MX server override for testing
//! - TLS requirements for compliance
//! - Connection limits for performance tuning
//! - Rate limiting to avoid blacklisting

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Configuration for a specific domain
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DomainConfig {
    /// Override MX server lookup with a specific host:port
    ///
    /// Use for testing to route messages to a local SMTP server:
    /// ```ron
    /// domains: {
    ///     "test.example.com": (
    ///         mx_override: "localhost:1025",
    ///     ),
    /// }
    /// ```
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub mx_override: Option<String>,

    /// Require TLS for delivery to this domain
    ///
    /// Delivery will fail if TLS cannot be negotiated.
    #[serde(default)]
    pub require_tls: bool,

    /// Maximum concurrent connections to this domain
    ///
    /// Prevents overwhelming recipient servers.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub max_connections: Option<usize>,

    /// Rate limit (messages per minute) for this domain
    ///
    /// Prevents being flagged as spam or hitting recipient quotas.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub rate_limit: Option<u32>,
}

impl DomainConfig {
    /// Check if this domain has an MX override configured
    #[must_use]
    pub const fn has_mx_override(&self) -> bool {
        self.mx_override.is_some()
    }

    /// Get the MX override server address if configured
    #[must_use]
    pub fn mx_override_address(&self) -> Option<&str> {
        self.mx_override.as_deref()
    }
}

/// Registry of per-domain configurations
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(transparent)]
pub struct DomainConfigRegistry {
    domains: HashMap<String, DomainConfig>,
}

impl DomainConfigRegistry {
    /// Create a new empty registry
    #[must_use]
    pub fn new() -> Self {
        Self {
            domains: HashMap::new(),
        }
    }

    /// Get configuration for a specific domain
    ///
    /// Returns `None` if no configuration exists, in which case default behavior applies.
    #[must_use]
    pub fn get(&self, domain: &str) -> Option<&DomainConfig> {
        self.domains.get(domain)
    }

    /// Add or update configuration for a domain
    pub fn insert(&mut self, domain: String, config: DomainConfig) {
        self.domains.insert(domain, config);
    }

    /// Check if a domain has any custom configuration
    #[must_use]
    pub fn has_config(&self, domain: &str) -> bool {
        self.domains.contains_key(domain)
    }

    /// Get the number of configured domains
    #[must_use]
    pub fn len(&self) -> usize {
        self.domains.len()
    }

    /// Check if the registry is empty
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.domains.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_domain_config_defaults() {
        let config = DomainConfig::default();
        assert!(!config.has_mx_override());
        assert!(!config.require_tls);
        assert!(config.max_connections.is_none());
        assert!(config.rate_limit.is_none());
    }

    #[test]
    fn test_domain_config_with_mx_override() {
        let config = DomainConfig {
            mx_override: Some("localhost:1025".to_string()),
            ..Default::default()
        };
        assert!(config.has_mx_override());
        assert_eq!(config.mx_override_address(), Some("localhost:1025"));
    }

    #[test]
    fn test_registry_operations() {
        let mut registry = DomainConfigRegistry::new();
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);

        registry.insert(
            "test.example.com".to_string(),
            DomainConfig {
                mx_override: Some("localhost:1025".to_string()),
                ..Default::default()
            },
        );

        assert!(!registry.is_empty());
        assert_eq!(registry.len(), 1);
        assert!(registry.has_config("test.example.com"));
        assert!(!registry.has_config("other.example.com"));

        let config = registry.get("test.example.com").unwrap();
        assert!(config.has_mx_override());
    }

    #[test]
    fn test_serde_roundtrip() {
        let mut registry = DomainConfigRegistry::new();
        registry.insert(
            "gmail.com".to_string(),
            DomainConfig {
                max_connections: Some(10),
                rate_limit: Some(100),
                require_tls: true,
                mx_override: None,
            },
        );
        registry.insert(
            "test.local".to_string(),
            DomainConfig {
                mx_override: Some("localhost:1025".to_string()),
                ..Default::default()
            },
        );

        let serialized = ron::to_string(&registry).unwrap();
        let deserialized: DomainConfigRegistry = ron::from_str(&serialized).unwrap();

        assert_eq!(deserialized.len(), 2);
        assert!(deserialized.get("gmail.com").unwrap().require_tls);
        assert_eq!(
            deserialized
                .get("test.local")
                .unwrap()
                .mx_override_address(),
            Some("localhost:1025")
        );
    }

    #[test]
    fn test_deserialize_from_config_format() {
        // Test the exact format used in empath.config.ron (with implicit_some extension)
        let config_str = r#"{
            "test.example.com": (
                mx_override: "localhost:1025",
            ),
            "secure.example.com": (
                require_tls: true,
                max_connections: 5,
            ),
            "gmail.com": (
                max_connections: 10,
                rate_limit: 100,
            ),
        }"#;

        let registry: DomainConfigRegistry = ron::Options::default()
            .with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME)
            .from_str(config_str)
            .unwrap();

        assert_eq!(registry.len(), 3);

        // Verify test.example.com has mx_override
        let test_config = registry.get("test.example.com").unwrap();
        assert_eq!(test_config.mx_override_address(), Some("localhost:1025"));
        assert!(!test_config.require_tls);

        // Verify secure.example.com has TLS and connection limit
        let secure_config = registry.get("secure.example.com").unwrap();
        assert!(secure_config.require_tls);
        assert_eq!(secure_config.max_connections, Some(5));
        assert!(secure_config.mx_override.is_none());

        // Verify gmail.com has connection and rate limits
        let gmail_config = registry.get("gmail.com").unwrap();
        assert_eq!(gmail_config.max_connections, Some(10));
        assert_eq!(gmail_config.rate_limit, Some(100));
        assert!(gmail_config.mx_override.is_none());
    }
}
