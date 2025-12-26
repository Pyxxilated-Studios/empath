//! Outbound binding configuration and selection logic.

use std::{collections::HashMap, net::IpAddr};

use empath_common::{
    Domain,
    context::{Context, OUTBOUND_BINDING_CONTEXT_KEY},
};
use serde::{Deserialize, Serialize};

use crate::{
    binding_health::BindingHealthConfig,
    error::{DeliveryError, SystemError},
    policy::{DomainPolicyResolver, RetryPolicy},
    types::SmtpTimeouts,
};

/// Outbound binding definition.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OutboundBinding {
    /// Source IP address to bind to (optional).
    #[serde(default)]
    pub source_ip: Option<IpAddr>,
    /// Source interface name to bind to (optional).
    #[serde(default)]
    pub source_interface: Option<String>,
    /// HELO/EHLO domain override (optional).
    #[serde(default)]
    pub helo_domain: Option<String>,
    /// TLS policy for this binding.
    #[serde(default)]
    pub tls: BindingTlsConfig,

    /// Whether to fetch MTA-STS policy when using this binding (override).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub mta_sts_enabled: Option<bool>,

    /// Optional SMTP timeout overrides for this binding.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub smtp_timeouts: Option<SmtpTimeouts>,

    /// Optional retry policy overrides for this binding.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub retry_policy: Option<RetryPolicy>,

    /// Allow retry attempts to fall back to another binding when this binding's
    /// retry policy is exhausted.
    #[serde(default)]
    pub allow_failover: bool,

    /// Optional health configuration override for this binding.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub health: Option<BindingHealthConfig>,
}

/// TLS policy for a binding.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BindingTlsConfig {
    /// Whether to attempt STARTTLS when advertised.
    #[serde(default = "default_attempt_starttls")]
    pub attempt_starttls: bool,
}

impl Default for BindingTlsConfig {
    fn default() -> Self {
        Self {
            attempt_starttls: default_attempt_starttls(),
        }
    }
}

const fn default_attempt_starttls() -> bool {
    true
}

/// Registry of configured outbound bindings.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OutboundBindingRegistry {
    /// Default binding name used when no overrides are present.
    #[serde(default)]
    pub default: Option<String>,
    /// Named binding definitions.
    #[serde(default)]
    pub bindings: HashMap<String, OutboundBinding>,
    /// Health configuration defaults for bindings.
    #[serde(default)]
    pub health: BindingHealthConfig,
}

impl OutboundBindingRegistry {
    /// Get a binding definition by name.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&OutboundBinding> {
        self.bindings.get(name)
    }

    /// List configured binding names in a stable order.
    #[must_use]
    pub fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.bindings.keys().cloned().collect();
        names.sort();
        names
    }

    #[must_use]
    pub fn health_config_for(&self, name: &str) -> BindingHealthConfig {
        self.bindings
            .get(name)
            .and_then(|binding| binding.health.clone())
            .unwrap_or_else(|| self.health.clone())
    }

    /// Resolve the binding name for a message.
    ///
    /// Precedence: module override → per-domain override → global default.
    ///
    /// # Errors
    ///
    /// Returns an error if an override refers to an unknown binding.
    pub fn resolve_binding_name(
        &self,
        context: &Context,
        domain: &Domain,
        domain_resolver: &DomainPolicyResolver,
    ) -> Result<Option<String>, DeliveryError> {
        if let Some(name) = context
            .transaction
            .get(OUTBOUND_BINDING_CONTEXT_KEY)
            .and_then(|value| normalize_binding_name(value.as_str()))
        {
            return self.validate_binding_name(name).map(Some);
        }

        if let Some(name) = domain_resolver.binding_override(domain.as_str()) {
            return self.validate_binding_name(&name).map(Some);
        }

        if let Some(name) = self
            .default
            .as_ref()
            .and_then(|value| normalize_binding_name(value.as_str()))
        {
            return self.validate_binding_name(name).map(Some);
        }

        Ok(None)
    }

    fn validate_binding_name(&self, name: &str) -> Result<String, DeliveryError> {
        if self.bindings.contains_key(name) {
            Ok(name.to_string())
        } else {
            Err(SystemError::Configuration(format!("Unknown outbound binding '{name}'")).into())
        }
    }
}

fn normalize_binding_name(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

#[cfg(test)]
#[allow(clippy::manual_let_else, clippy::panic)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::domain_config::{DomainConfig, DomainConfigRegistry};

    fn registry_with_default() -> OutboundBindingRegistry {
        let mut registry = OutboundBindingRegistry {
            default: Some("primary".to_string()),
            ..OutboundBindingRegistry::default()
        };
        registry.bindings.insert(
            "primary".to_string(),
            OutboundBinding {
                helo_domain: Some("mx.example.com".to_string()),
                ..OutboundBinding::default()
            },
        );
        registry.bindings.insert(
            "backup".to_string(),
            OutboundBinding {
                helo_domain: Some("mx-backup.example.com".to_string()),
                ..OutboundBinding::default()
            },
        );
        registry
    }

    #[test]
    fn resolves_default_binding() {
        let registry = registry_with_default();
        let context = Context::default();
        let domains = DomainConfigRegistry::default();
        let resolver = DomainPolicyResolver::new(domains, false);

        let name = registry
            .resolve_binding_name(&context, &Domain::new("example.com"), &resolver)
            .unwrap_or_else(|err| panic!("binding resolution should succeed: {err}"));

        assert_eq!(name.as_deref(), Some("primary"));
    }

    #[test]
    fn resolves_domain_override_before_default() {
        let registry = registry_with_default();
        let context = Context::default();
        let domains = DomainConfigRegistry::from_map(HashMap::from([(
            "example.com".to_string(),
            DomainConfig {
                binding: Some("backup".to_string()),
                ..DomainConfig::default()
            },
        )]));
        let resolver = DomainPolicyResolver::new(domains, false);

        let name = registry
            .resolve_binding_name(&context, &Domain::new("example.com"), &resolver)
            .unwrap_or_else(|err| panic!("binding resolution should succeed: {err}"));

        assert_eq!(name.as_deref(), Some("backup"));
    }

    #[test]
    fn resolves_module_override_before_domain() {
        let registry = registry_with_default();
        let mut context = Context::default();
        context
            .transaction
            .insert(OUTBOUND_BINDING_CONTEXT_KEY.into(), "backup".to_string());
        let domains = DomainConfigRegistry::from_map(HashMap::from([(
            "example.com".to_string(),
            DomainConfig {
                binding: Some("primary".to_string()),
                ..DomainConfig::default()
            },
        )]));
        let resolver = DomainPolicyResolver::new(domains, false);

        let name = registry
            .resolve_binding_name(&context, &Domain::new("example.com"), &resolver)
            .unwrap_or_else(|err| panic!("binding resolution should succeed: {err}"));

        assert_eq!(name.as_deref(), Some("backup"));
    }

    #[test]
    fn rejects_unknown_override() {
        let registry = registry_with_default();
        let mut context = Context::default();
        context
            .transaction
            .insert(OUTBOUND_BINDING_CONTEXT_KEY.into(), "unknown".to_string());
        let domains = DomainConfigRegistry::default();
        let resolver = DomainPolicyResolver::new(domains, false);

        let err =
            match registry.resolve_binding_name(&context, &Domain::new("example.com"), &resolver) {
                Ok(_) => panic!("expected unknown binding error"),
                Err(err) => err,
            };

        assert!(matches!(
            err,
            DeliveryError::System(SystemError::Configuration(_))
        ));
    }
}
