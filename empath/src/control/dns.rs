//! DNS command handling

use std::collections::HashMap;

use empath_control::protocol::{self as control_protocol, ResponseData};
use empath_control::{ControlError, DnsCommand, Response, Result as ControlResult};

use super::audit::audit;
use super::handler::EmpathControlHandler;

impl EmpathControlHandler {
    /// Handle DNS cache management commands
    pub(super) async fn handle_dns_command(&self, command: DnsCommand) -> ControlResult<Response> {
        // Audit log: Record who executed this command
        audit!(INFO, "Control command: DNS", command = ?command);

        let Some(resolver) = self.delivery.dns_resolver() else {
            audit!(
                WARN,
                "DNS command failed: resolver not initialized",
                command = ?command
            );
            return Err(ControlError::ServerError(
                "DNS resolver not initialized".to_string(),
            ));
        };

        let result = match command {
            DnsCommand::ListCache => {
                let cache = resolver.list_cache();

                // Convert to control protocol types
                let cache_data: HashMap<String, Vec<control_protocol::CachedMailServer>> = cache
                    .into_iter()
                    .map(|(domain, servers)| {
                        let servers = servers
                            .into_iter()
                            .map(|(server, ttl)| control_protocol::CachedMailServer {
                                host: server.host,
                                priority: server.priority,
                                port: server.port,
                                ttl_remaining_secs: ttl.as_secs(),
                            })
                            .collect();
                        (domain, servers)
                    })
                    .collect();

                Ok(Response::data(ResponseData::DnsCache(cache_data)))
            }

            DnsCommand::ClearCache => {
                resolver.clear_cache();
                Ok(Response::ok())
            }

            DnsCommand::RefreshDomain(domain) => match resolver.refresh_domain(&domain).await {
                Ok(servers) => {
                    let message = format!(
                        "Refreshed DNS for {domain}: {} mail server(s)",
                        servers.len()
                    );
                    Ok(Response::data(ResponseData::Message(message)))
                }
                Err(e) => Err(ControlError::ServerError(format!(
                    "Failed to refresh domain {domain}: {e}"
                ))),
            },

            DnsCommand::SetOverride { domain, mx_server } => {
                // Update domain config registry
                self.update_mx_override(&domain, Some(&mx_server));

                let message = format!("Set MX override for {domain} -> {mx_server}");
                Ok(Response::data(ResponseData::Message(message)))
            }

            DnsCommand::RemoveOverride(domain) => {
                self.update_mx_override(&domain, None);

                let message = format!("Removed MX override for {domain}");
                Ok(Response::data(ResponseData::Message(message)))
            }

            DnsCommand::ListOverrides => {
                let overrides = self.list_mx_overrides();
                Ok(Response::data(ResponseData::MxOverrides(overrides)))
            }
        };

        // Audit log: Record command result
        match &result {
            Ok(_) => {
                audit!(INFO, "DNS command completed successfully");
            }
            Err(e) => {
                audit!(WARN, "DNS command failed", error = %e);
            }
        }

        result
    }

    /// Update MX override in domain configuration
    ///
    /// Note: This is a runtime-only change and does not persist across restarts.
    /// To make overrides permanent, update the configuration file.
    pub(super) fn update_mx_override(&self, domain: &str, mx_override: Option<&String>) {
        // Access the domain registry
        let registry = self.delivery.domains();

        // Get existing configuration or create a default
        let config = registry
            .get(domain)
            .map(|entry| entry.value().clone())
            .unwrap_or_default();

        // Update MX override
        let mut updated_config = config;
        updated_config.mx_override = mx_override.cloned();

        // Insert updated configuration (DomainConfigRegistry now has interior mutability)
        registry.insert(domain.to_string(), updated_config);

        tracing::event!(
            tracing::Level::INFO,
            domain = %domain,
            mx_override = ?mx_override,
            "Updated MX override for domain at runtime"
        );
    }

    /// List all configured MX overrides
    pub(super) fn list_mx_overrides(&self) -> HashMap<String, String> {
        let registry = self.delivery.domains();
        let mut overrides = HashMap::new();

        for (domain, config) in registry.iter() {
            if let Some(ref mx_override) = config.mx_override {
                overrides.insert(domain.clone(), mx_override.clone());
            }
        }

        overrides
    }
}
