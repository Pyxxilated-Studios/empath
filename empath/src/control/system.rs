//! System command handling

use std::str::FromStr;
use std::time::SystemTime;

use empath_common::Signal;
use empath_common::context::Capability;
use empath_control::protocol::{self as control_protocol, ResponseData};
use empath_control::{ControlError, Response, Result as ControlResult, SystemCommand};
use empath_delivery::BindingHealthState;
use tracing_subscriber::filter::LevelFilter;

use super::audit::audit;
use super::config::{reload_config_from_disk, resolve_tls_policy_preview};
use super::handler::EmpathControlHandler;

impl EmpathControlHandler {
    /// Handle system management commands
    pub(super) fn handle_system_command(&self, command: &SystemCommand) -> ControlResult<Response> {
        // Audit log: Record who executed this command
        audit!(INFO, "Control command: System", command = ?command);

        let result = match command {
            SystemCommand::Ping => Ok(Response::ok()),
            SystemCommand::Status => Ok(self.handle_system_status()),
            SystemCommand::SetLogLevel { level } => self.handle_system_set_log_level(level),
            SystemCommand::ReloadConfig => self.handle_system_reload_config(),
            SystemCommand::ReloadTls => self.handle_system_reload_tls(),
            SystemCommand::ListTls => self.handle_system_list_tls(),
            SystemCommand::SetTlsProfile { set_id, profile } => {
                self.handle_system_set_tls_profile(set_id, profile)
            }
            SystemCommand::DrainListener { socket } => Self::handle_system_drain_listener(socket),
            SystemCommand::ConnectionStats => Ok(self.handle_system_connection_stats()),
            SystemCommand::ListBindings => Ok(self.list_bindings_response()),
            SystemCommand::PreviewBinding { domain } => {
                Ok(self.handle_system_binding_preview(domain))
            }
            SystemCommand::SetRateLimit {
                domain,
                messages_per_second,
                burst,
            } => self.handle_system_set_rate_limit(domain.as_ref(), *messages_per_second, *burst),
            SystemCommand::SetBindingState { name, enabled } => {
                self.handle_system_set_binding_state(name, *enabled)
            }
            SystemCommand::ListCapabilities => self.handle_system_list_capabilities(),
            SystemCommand::SetCapabilityState {
                capability,
                enabled,
            } => self.handle_system_set_capability_state(capability, *enabled),
        };

        // Audit log: Record command result
        match &result {
            Ok(_) => {
                audit!(INFO, "System command completed successfully");
            }
            Err(e) => {
                audit!(WARN, "System command failed", error = %e);
            }
        }

        result
    }

    fn handle_system_status(&self) -> Response {
        let uptime_secs = self.start_time.elapsed().as_secs();

        let dns_cache_entries = self
            .delivery
            .dns_resolver()
            .as_ref()
            .map_or(0, |r| r.cache_stats().total_entries);

        let queue_size = self.delivery.queue_len();

        let status = control_protocol::SystemStatus {
            version: env!("CARGO_PKG_VERSION").to_string(),
            uptime_secs,
            queue_size,
            dns_cache_entries,
        };

        Response::data(ResponseData::SystemStatus(status))
    }

    fn handle_system_binding_preview(&self, domain: &str) -> Response {
        let domains = self.delivery.domains();
        let outbound_bindings = self.delivery.outbound_bindings();

        let binding_name = domains
            .get(domain)
            .and_then(|config| config.binding.clone())
            .or_else(|| outbound_bindings.default.clone());
        let binding = binding_name
            .as_deref()
            .and_then(|name| outbound_bindings.get(name));
        let binding_enabled = binding_name
            .as_deref()
            .map(|name| self.delivery.binding_enabled(name));
        let attempt_starttls = binding.map(|binding| binding.tls.attempt_starttls);

        let tls_policy = resolve_tls_policy_preview(domains, domain, self.delivery.tls_policy());

        let preview = control_protocol::BindingPreview {
            domain: domain.to_string(),
            binding: binding_name,
            binding_enabled,
            attempt_starttls,
            tls_policy: tls_policy.to_string(),
        };

        Response::data(ResponseData::BindingPreview(preview))
    }

    fn handle_system_set_log_level(&self, level: &str) -> ControlResult<Response> {
        let parsed = LevelFilter::from_str(level)
            .map_err(|e| ControlError::ServerError(format!("Invalid log level '{level}': {e}")))?;
        self.log_handle
            .set_log_level(parsed)
            .map_err(|e| ControlError::ServerError(format!("Failed to set log level: {e}")))?;
        Ok(Response::data(ResponseData::Message(format!(
            "Log level set to {parsed}"
        ))))
    }

    fn handle_system_reload_config(&self) -> ControlResult<Response> {
        match reload_config_from_disk(&self.delivery) {
            Ok(message) => Ok(Response::data(ResponseData::Message(message))),
            Err(e) => Err(ControlError::ServerError(e)),
        }
    }

    fn handle_system_reload_tls(&self) -> ControlResult<Response> {
        let registry = self.tls_registry.as_ref().ok_or_else(|| {
            ControlError::ServerError("TLS is not configured for this instance".to_string())
        })?;

        match registry.reload_all() {
            Ok(0) => Ok(Response::data(ResponseData::Message(
                "No TLS contexts configured".to_string(),
            ))),
            Ok(count) => Ok(Response::data(ResponseData::Message(format!(
                "Reloaded TLS credentials for {count} context(s)",
            )))),
            Err(err) => Err(ControlError::ServerError(format!(
                "Failed to reload TLS credentials: {err}"
            ))),
        }
    }

    fn handle_system_list_tls(&self) -> ControlResult<Response> {
        let registry = self.tls_registry.as_ref().ok_or_else(|| {
            ControlError::ServerError("TLS is not configured for this instance".to_string())
        })?;

        let contexts = registry
            .list_profiles()
            .into_iter()
            .map(|profile| control_protocol::TlsContextInfo {
                set_id: profile.set_id,
                profile: Some(profile.profile),
                active: profile.active,
                certificate_path: profile.context.certificate_path.display().to_string(),
                key_path: profile.context.key_path.display().to_string(),
                fingerprint_sha256: profile.context.fingerprint_sha256,
                subject: profile.context.subject,
                issuer: profile.context.issuer,
                not_before: profile.context.not_before,
                not_after: profile.context.not_after,
                subject_alt_names: profile.context.subject_alt_names,
                parse_error: profile.context.parse_error,
            })
            .collect();

        Ok(Response::data(ResponseData::TlsContexts(contexts)))
    }

    fn handle_system_set_tls_profile(
        &self,
        set_id: &str,
        profile: &str,
    ) -> ControlResult<Response> {
        let registry = self.tls_registry.as_ref().ok_or_else(|| {
            ControlError::ServerError("TLS is not configured for this instance".to_string())
        })?;

        registry
            .set_active_profile(set_id, profile)
            .map_err(|err| {
                ControlError::ServerError(format!("Failed to switch TLS profile: {err}"))
            })?;

        Ok(Response::data(ResponseData::Message(format!(
            "TLS profile '{profile}' is now active for '{set_id}'"
        ))))
    }

    fn handle_system_drain_listener(socket: &str) -> ControlResult<Response> {
        let target = socket.parse().map_err(|err| {
            ControlError::ServerError(format!("Invalid socket '{socket}': {err}"))
        })?;

        if crate::controller::SHUTDOWN_BROADCAST
            .send(Signal::Drain(target))
            .is_err()
        {
            return Err(ControlError::ServerError(
                "Failed to send drain signal".to_string(),
            ));
        }

        Ok(Response::data(ResponseData::Message(format!(
            "Listener {socket} entering drain mode"
        ))))
    }

    fn handle_system_connection_stats(&self) -> Response {
        let stats = self.delivery.connection_stats();
        Response::data(ResponseData::ConnectionStats(
            control_protocol::ConnectionStats {
                queue_size: stats.queue_size,
                rate_limit_domains: stats.rate_limit_domains,
            },
        ))
    }

    pub(super) fn list_bindings_response(&self) -> Response {
        let registry = self.delivery.outbound_bindings();
        let mut bindings: Vec<control_protocol::OutboundBindingInfo> = registry
            .bindings
            .into_iter()
            .map(|(name, binding)| {
                let enabled = self.delivery.binding_enabled(&name);
                let health = self.delivery.binding_health_status(&name);
                let (health_state, consecutive_failures, open_until) =
                    health.map_or((None, None, None), |status| {
                        let state_label = match status.state {
                            BindingHealthState::Closed => "closed",
                            BindingHealthState::HalfOpen => "half_open",
                            BindingHealthState::Open => "open",
                        };
                        let open_until = status
                            .open_until
                            .and_then(|time| time.duration_since(SystemTime::UNIX_EPOCH).ok())
                            .map(|duration| duration.as_secs());
                        (
                            Some(state_label.to_string()),
                            Some(status.consecutive_failures),
                            open_until,
                        )
                    });

                control_protocol::OutboundBindingInfo {
                    name,
                    source_ip: binding.source_ip.map(|ip| ip.to_string()),
                    source_interface: binding.source_interface,
                    helo_domain: binding.helo_domain,
                    attempt_starttls: binding.tls.attempt_starttls,
                    enabled,
                    health_state,
                    consecutive_failures,
                    open_until,
                }
            })
            .collect();
        bindings.sort_by(|a, b| a.name.cmp(&b.name));

        Response::data(ResponseData::OutboundBindings(bindings))
    }

    fn handle_system_set_rate_limit(
        &self,
        domain: Option<&String>,
        messages_per_second: f64,
        burst: u32,
    ) -> ControlResult<Response> {
        let domain = domain.map(ToString::to_string);
        self.delivery
            .update_rate_limit(domain.clone(), messages_per_second, burst)
            .map_err(ControlError::ServerError)?;
        Ok(Response::data(ResponseData::Message(format!(
            "Rate limit updated (domain={domain:?}, mps={messages_per_second}, burst={burst})"
        ))))
    }

    fn handle_system_set_binding_state(
        &self,
        name: &str,
        enabled: bool,
    ) -> ControlResult<Response> {
        self.delivery
            .set_binding_enabled(name, enabled)
            .map_err(ControlError::ServerError)?;
        let state = if enabled { "enabled" } else { "disabled" };
        Ok(Response::data(ResponseData::Message(format!(
            "Binding {name} {state}"
        ))))
    }

    fn handle_system_list_capabilities(&self) -> ControlResult<Response> {
        let state = self.smtp_capabilities.as_ref().ok_or_else(|| {
            ControlError::ServerError(
                "SMTP capabilities are not configured for this instance".to_string(),
            )
        })?;
        let mut capabilities: Vec<String> = state
            .read()
            .map_err(|_| {
                ControlError::ServerError("SMTP capabilities state is unavailable".to_string())
            })?
            .iter()
            .map(std::string::ToString::to_string)
            .collect();
        capabilities.sort();
        Ok(Response::data(ResponseData::SmtpCapabilities(capabilities)))
    }

    fn handle_system_set_capability_state(
        &self,
        capability: &str,
        enabled: bool,
    ) -> ControlResult<Response> {
        let state = self.smtp_capabilities.as_ref().ok_or_else(|| {
            ControlError::ServerError(
                "SMTP capabilities are not configured for this instance".to_string(),
            )
        })?;
        let parsed = Capability::parse(capability).ok_or_else(|| {
            ControlError::ServerError(format!("Unknown SMTP capability '{capability}'"))
        })?;
        let mut guard = state.write().map_err(|_| {
            ControlError::ServerError("SMTP capabilities state is unavailable".to_string())
        })?;
        if enabled {
            if !guard.contains(&parsed) {
                guard.push(parsed.clone());
            }
        } else {
            guard.retain(|cap| cap != &parsed);
        }
        drop(guard);
        let action = if enabled { "enabled" } else { "disabled" };
        Ok(Response::data(ResponseData::Message(format!(
            "SMTP capability {} {action}",
            parsed.as_str()
        ))))
    }
}
