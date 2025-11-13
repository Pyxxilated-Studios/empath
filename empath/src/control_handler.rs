//! Control handler implementation for Empath MTA
//!
//! This module implements the `CommandHandler` trait to process control requests
//! for managing the running MTA instance.

use std::{collections::HashMap, future::Future, pin::Pin, sync::Arc, time::Instant};

use empath_control::{
    ControlError, DnsCommand, Request, Response, SystemCommand, protocol::ResponseData,
    server::CommandHandler,
};
use empath_delivery::DeliveryProcessor;

/// Handler for control commands
pub struct EmpathControlHandler {
    /// Reference to the delivery processor for DNS operations
    delivery: Arc<DeliveryProcessor>,
    /// Server start time for uptime calculation
    start_time: Instant,
}

impl EmpathControlHandler {
    /// Create a new control handler
    #[must_use]
    pub fn new(delivery: Arc<DeliveryProcessor>) -> Self {
        Self {
            delivery,
            start_time: Instant::now(),
        }
    }
}

impl CommandHandler for EmpathControlHandler {
    fn handle_request(
        &self,
        request: Request,
    ) -> Pin<Box<dyn Future<Output = empath_control::Result<Response>> + Send + '_>> {
        Box::pin(async move {
            match request {
                Request::Dns(dns_cmd) => self.handle_dns_command(dns_cmd).await,
                Request::System(sys_cmd) => self.handle_system_command(sys_cmd).await,
            }
        })
    }
}

impl EmpathControlHandler {
    /// Handle DNS cache management commands
    async fn handle_dns_command(&self, command: DnsCommand) -> empath_control::Result<Response> {
        let Some(resolver) = self.delivery.dns_resolver() else {
            return Err(ControlError::ServerError(
                "DNS resolver not initialized".to_string(),
            ));
        };

        match command {
            DnsCommand::ListCache => {
                let cache = resolver.list_cache();

                // Convert to control protocol types
                let cache_data: HashMap<String, Vec<empath_control::protocol::CachedMailServer>> =
                    cache
                        .into_iter()
                        .map(|(domain, servers)| {
                            let servers = servers
                                .into_iter()
                                .map(|(server, ttl)| empath_control::protocol::CachedMailServer {
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
                self.update_mx_override(&domain, Some(mx_server.clone()))?;

                let message = format!("Set MX override for {domain} -> {mx_server}");
                Ok(Response::data(ResponseData::Message(message)))
            }

            DnsCommand::RemoveOverride(domain) => {
                self.update_mx_override(&domain, None)?;

                let message = format!("Removed MX override for {domain}");
                Ok(Response::data(ResponseData::Message(message)))
            }

            DnsCommand::ListOverrides => {
                let overrides = self.list_mx_overrides();
                Ok(Response::data(ResponseData::MxOverrides(overrides)))
            }
        }
    }

    /// Handle system management commands
    async fn handle_system_command(
        &self,
        command: SystemCommand,
    ) -> empath_control::Result<Response> {
        match command {
            SystemCommand::Ping => Ok(Response::ok()),

            SystemCommand::Status => {
                let uptime_secs = self.start_time.elapsed().as_secs();

                let dns_cache_entries = self
                    .delivery
                    .dns_resolver()
                    .as_ref()
                    .map_or(0, |r| r.cache_stats().total_entries);

                let queue_size = self.delivery.queue().len().await;

                let status = empath_control::protocol::SystemStatus {
                    version: env!("CARGO_PKG_VERSION").to_string(),
                    uptime_secs,
                    queue_size,
                    dns_cache_entries,
                };

                Ok(Response::data(ResponseData::SystemStatus(status)))
            }
        }
    }

    /// Update MX override in domain configuration
    ///
    /// Note: This is a runtime-only change and does not persist across restarts.
    /// To make overrides permanent, update the configuration file.
    fn update_mx_override(
        &self,
        domain: &str,
        mx_override: Option<String>,
    ) -> empath_control::Result<()> {
        // Access the domain registry
        let registry = self.delivery.domains();

        // Get or create domain config
        let mut config = registry
            .get(domain)
            .map(|c| (*c).clone())
            .unwrap_or_default();

        // Update MX override
        config.mx_override = mx_override;

        // Note: DomainConfigRegistry uses interior mutability via HashMap,
        // but it's not behind a Mutex/RwLock. Since we have an Arc<DeliveryProcessor>,
        // we can't mutate it. For now, return an error indicating this is not yet supported.
        // TODO: Make DomainConfigRegistry use Arc<DashMap> for runtime updates
        Err(ControlError::ServerError(
            "Runtime MX override updates not yet supported. Please update the configuration file and restart.".to_string()
        ))
    }

    /// List all configured MX overrides
    fn list_mx_overrides(&self) -> HashMap<String, String> {
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
