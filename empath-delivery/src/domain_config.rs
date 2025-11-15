//! Per-domain delivery configuration
//!
//! Allows customizing delivery behavior for specific recipient domains:
//! - MX server override for testing
//! - TLS requirements for compliance
//! - Connection limits for performance tuning
//! - Rate limiting to avoid blacklisting

use std::{collections::HashMap, sync::Arc};

use dashmap::DashMap;
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

    /// Accept invalid TLS certificates for this domain
    ///
    /// **SECURITY WARNING**: Setting this overrides the global configuration
    /// and disables certificate validation for this specific domain.
    /// Only use for testing with self-signed certificates.
    ///
    /// - `Some(true)`: Accept invalid certs (override global config)
    /// - `Some(false)`: Require valid certs (override global config)
    /// - `None`: Use global configuration (default)
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub accept_invalid_certs: Option<bool>,

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
///
/// Uses `Arc<DashMap>` for lock-free concurrent access and runtime updates.
/// This allows the control socket to dynamically add/remove domain configurations
/// without requiring a restart.
#[derive(Debug, Clone)]
pub struct DomainConfigRegistry {
    domains: Arc<DashMap<String, DomainConfig>>,
}

impl DomainConfigRegistry {
    /// Create a new empty registry
    #[must_use]
    pub fn new() -> Self {
        Self {
            domains: Arc::new(DashMap::new()),
        }
    }

    /// Create a registry from a HashMap (used during deserialization)
    #[must_use]
    pub fn from_map(map: HashMap<String, DomainConfig>) -> Self {
        let registry = Self::new();
        for (domain, config) in map {
            registry.domains.insert(domain, config);
        }
        registry
    }

    /// Get configuration for a specific domain
    ///
    /// Returns `None` if no configuration exists, in which case default behavior applies.
    #[must_use]
    pub fn get(&self, domain: &str) -> Option<dashmap::mapref::one::Ref<'_, String, DomainConfig>> {
        self.domains.get(domain)
    }

    /// Add or update configuration for a domain
    ///
    /// This method provides interior mutability, allowing runtime updates
    /// through the control socket without requiring `&mut self`.
    pub fn insert(&self, domain: String, config: DomainConfig) {
        self.domains.insert(domain, config);
    }

    /// Remove configuration for a domain
    ///
    /// Returns the removed configuration if it existed.
    pub fn remove(&self, domain: &str) -> Option<(String, DomainConfig)> {
        self.domains.remove(domain)
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

    /// Iterate over all domain configurations (for control interface)
    ///
    /// Note: This returns owned values by cloning the internal map.
    /// For read-only access with zero-copy, use `iter_ref()`.
    pub fn iter(&self) -> impl Iterator<Item = (String, DomainConfig)> {
        self.domains
            .iter()
            .map(|entry| (entry.key().clone(), entry.value().clone()))
            .collect::<Vec<_>>()
            .into_iter()
    }

    /// Convert to HashMap (used during serialization)
    #[must_use]
    pub fn to_map(&self) -> HashMap<String, DomainConfig> {
        self.domains
            .iter()
            .map(|entry| (entry.key().clone(), entry.value().clone()))
            .collect()
    }
}

impl Default for DomainConfigRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl Serialize for DomainConfigRegistry {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.to_map().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for DomainConfigRegistry {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let map = HashMap::<String, DomainConfig>::deserialize(deserializer)?;
        Ok(Self::from_map(map))
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
        assert!(config.accept_invalid_certs.is_none());
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
        let registry = DomainConfigRegistry::new();
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
        let registry = DomainConfigRegistry::new();
        registry.insert(
            "gmail.com".to_string(),
            DomainConfig {
                max_connections: Some(10),
                rate_limit: Some(100),
                require_tls: true,
                accept_invalid_certs: None,
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

    #[test]
    fn test_accept_invalid_certs_configuration() {
        // Test per-domain override of certificate validation
        let registry = DomainConfigRegistry::new();

        // Domain that explicitly accepts invalid certs
        registry.insert(
            "test.local".to_string(),
            DomainConfig {
                accept_invalid_certs: Some(true),
                ..Default::default()
            },
        );

        // Domain that explicitly requires valid certs
        registry.insert(
            "secure.example.com".to_string(),
            DomainConfig {
                accept_invalid_certs: Some(false),
                require_tls: true,
                ..Default::default()
            },
        );

        // Domain with no override (uses global config)
        registry.insert(
            "default.example.com".to_string(),
            DomainConfig {
                accept_invalid_certs: None,
                ..Default::default()
            },
        );

        // Verify configurations
        let test_config = registry.get("test.local").unwrap();
        assert_eq!(test_config.accept_invalid_certs, Some(true));

        let secure_config = registry.get("secure.example.com").unwrap();
        assert_eq!(secure_config.accept_invalid_certs, Some(false));
        assert!(secure_config.require_tls);

        let default_config = registry.get("default.example.com").unwrap();
        assert_eq!(default_config.accept_invalid_certs, None);
    }

    #[test]
    fn test_runtime_updates() {
        // Test runtime MX override updates without requiring &mut self
        let registry = DomainConfigRegistry::new();

        // Add initial configuration
        registry.insert(
            "test.example.com".to_string(),
            DomainConfig {
                mx_override: Some("localhost:1025".to_string()),
                ..Default::default()
            },
        );

        assert_eq!(registry.len(), 1);
        assert_eq!(
            registry
                .get("test.example.com")
                .unwrap()
                .mx_override_address(),
            Some("localhost:1025")
        );

        // Update configuration at runtime
        registry.insert(
            "test.example.com".to_string(),
            DomainConfig {
                mx_override: Some("localhost:2525".to_string()),
                ..Default::default()
            },
        );

        assert_eq!(registry.len(), 1); // Same entry, updated
        assert_eq!(
            registry
                .get("test.example.com")
                .unwrap()
                .mx_override_address(),
            Some("localhost:2525")
        );

        // Remove configuration
        let removed = registry.remove("test.example.com");
        assert!(removed.is_some());
        assert_eq!(registry.len(), 0);
        assert!(registry.get("test.example.com").is_none());
    }
}
