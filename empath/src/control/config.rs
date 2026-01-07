//! Configuration management utilities

use std::path::{Path, PathBuf};
use std::sync::Arc;

use empath_delivery::{DeliveryQueryService, DomainConfigRegistry, TlsPolicy};

use crate::config;
use crate::controller::Empath;

pub(super) const RELOADABLE_FIELDS: &[&str] = &[
    "delivery.domains",
    "delivery.rate_limit",
    "delivery.retry_policy",
    "delivery.smtp_timeouts",
    "delivery.circuit_breaker",
];

pub(super) const RESTART_REQUIRED_FIELDS: &[&str] = &[
    "smtp.listeners/timeouts",
    "controller socket/auth/rate limits",
    "delivery.dns",
    "delivery.smtp_pool",
    "delivery.dsn",
    "delivery.outbound_bindings",
    "metrics",
    "health",
    "audit",
];

pub(super) fn reload_config_from_disk(
    delivery: &Arc<dyn DeliveryQueryService>,
) -> Result<String, String> {
    let config_path = find_config_file().map_err(|e| e.to_string())?;
    let empath: Empath = config::parse_config_file(&config_path).map_err(|e| e.to_string())?;

    let (domains, rate_limit, retry_policy, smtp_timeouts, circuit_breaker) =
        empath.delivery_reload_settings();

    delivery.replace_domains(domains);
    delivery.replace_rate_limit_config(rate_limit);
    delivery.replace_retry_policy(retry_policy);
    delivery.replace_smtp_timeouts(smtp_timeouts);
    delivery.replace_circuit_breaker_config(circuit_breaker);

    let report = ReloadReport::new(RELOADABLE_FIELDS.to_vec(), RESTART_REQUIRED_FIELDS.to_vec());

    Ok(report.render(&config_path))
}

pub(super) fn find_config_file() -> anyhow::Result<PathBuf> {
    if let Ok(env_path) = std::env::var("EMPATH_CONFIG") {
        let path = PathBuf::from(env_path);
        if path.exists() {
            return Ok(path);
        }
        anyhow::bail!(
            "EMPATH_CONFIG points to non-existent file: {}",
            path.display()
        );
    }

    let default_paths = vec![
        PathBuf::from("./empath.config.ron"),
        PathBuf::from("/etc/empath/empath.config.ron"),
    ];

    for path in &default_paths {
        if path.exists() {
            return Ok(path.clone());
        }
    }

    let paths_tried = default_paths
        .iter()
        .map(|p| format!("  - {}", p.display()))
        .collect::<Vec<_>>()
        .join("\n");

    anyhow::bail!(
        "No configuration file found. Tried:\n  - EMPATH_CONFIG environment variable\n{paths_tried}"
    )
}

pub(super) fn resolve_tls_policy_preview(
    domains: &DomainConfigRegistry,
    domain: &str,
    global_policy: TlsPolicy,
) -> TlsPolicy {
    if let Some(config) = domains.get(domain) {
        if let Some(policy) = config.tls_policy {
            return policy;
        }
        if config.require_tls {
            return TlsPolicy::Required;
        }
    }

    global_policy
}

struct ReloadReport {
    applied: Vec<&'static str>,
    restart_required: Vec<&'static str>,
}

impl ReloadReport {
    const fn new(applied: Vec<&'static str>, restart_required: Vec<&'static str>) -> Self {
        Self {
            applied,
            restart_required,
        }
    }

    fn render(&self, path: &Path) -> String {
        let applied = format_reload_list(&self.applied);
        let restart_required = format_reload_list(&self.restart_required);
        format!(
            "Configuration reloaded from {}. Applied: {applied}. Restart required for: {restart_required}.",
            path.display()
        )
    }
}

fn format_reload_list(items: &[&'static str]) -> String {
    if items.is_empty() {
        "none".to_string()
    } else {
        items.join(", ")
    }
}
