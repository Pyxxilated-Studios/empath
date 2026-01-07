//! Tests for control handler

use std::io;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use empath_common::DeliveryStatus;
use empath_delivery::{
    BindingHealthStatus, CircuitBreakerConfig, ConnectionStats, DeliveryInfo, DeliveryQueryService,
    DnsResolver, DomainConfig, DomainConfigRegistry, OutboundBindingRegistry, QueueSnapshot,
    RateLimitConfig, RetryPolicy, SmtpTimeouts, TlsPolicy,
};
use empath_spool::{BackingStore, SpooledMessageId};

use super::config::{reload_config_from_disk, resolve_tls_policy_preview};
use super::message::parse_message_id;

static ENV_LOCK: Mutex<()> = Mutex::new(());
type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

struct EnvVarGuard {
    key: &'static str,
    old: Option<String>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: &std::path::Path) -> Self {
        let old = std::env::var(key).ok();
        unsafe {
            std::env::set_var(key, value);
        }
        Self { key, old }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        if let Some(old) = self.old.take() {
            unsafe {
                std::env::set_var(self.key, old);
            }
        } else {
            unsafe {
                std::env::remove_var(self.key);
            }
        }
    }
}

#[derive(Default)]
struct MockDelivery {
    domains_view: DomainConfigRegistry,
    dns_resolver: Option<Arc<dyn DnsResolver>>,
    spool: Option<Arc<dyn BackingStore>>,
    replaced_domains: Mutex<Option<DomainConfigRegistry>>,
    replaced_rate_limit: Mutex<Option<RateLimitConfig>>,
    replaced_retry_policy: Mutex<Option<RetryPolicy>>,
    replaced_smtp_timeouts: Mutex<Option<SmtpTimeouts>>,
    replaced_circuit_breaker: Mutex<Option<CircuitBreakerConfig>>,
    tls_policy: TlsPolicy,
}

impl MockDelivery {
    fn replaced_domains_len(&self) -> Option<usize> {
        let guard = match self.replaced_domains.lock() {
            Ok(guard) => guard,
            Err(err) => err.into_inner(),
        };
        guard.as_ref().map(DomainConfigRegistry::len)
    }

    fn replaced_rate_limit_mps(&self) -> Option<f64> {
        let guard = match self.replaced_rate_limit.lock() {
            Ok(guard) => guard,
            Err(err) => err.into_inner(),
        };
        guard.as_ref().map(|cfg| cfg.messages_per_second)
    }

    fn replaced_retry_policy_max_attempts(&self) -> Option<u32> {
        let guard = match self.replaced_retry_policy.lock() {
            Ok(guard) => guard,
            Err(err) => err.into_inner(),
        };
        guard.as_ref().map(|cfg| cfg.max_attempts)
    }

    fn replaced_smtp_timeouts_ehlo_secs(&self) -> Option<u64> {
        let guard = match self.replaced_smtp_timeouts.lock() {
            Ok(guard) => guard,
            Err(err) => err.into_inner(),
        };
        guard.as_ref().map(|cfg| cfg.ehlo_secs)
    }

    fn replaced_circuit_breaker_threshold(&self) -> Option<u32> {
        let guard = match self.replaced_circuit_breaker.lock() {
            Ok(guard) => guard,
            Err(err) => err.into_inner(),
        };
        guard.as_ref().map(|cfg| cfg.failure_threshold)
    }
}

impl DeliveryQueryService for MockDelivery {
    fn queue_len(&self) -> usize {
        0
    }

    fn get_message(&self, _id: &SpooledMessageId) -> Option<DeliveryInfo> {
        None
    }

    fn list_messages(&self, _status: Option<DeliveryStatus>) -> Vec<DeliveryInfo> {
        Vec::new()
    }

    fn list_messages_iter(
        &self,
        _status: Option<DeliveryStatus>,
    ) -> Box<dyn Iterator<Item = DeliveryInfo> + '_> {
        Box::new(std::iter::empty())
    }

    fn list_message_ids(&self, _status_filter: Option<&str>) -> Vec<SpooledMessageId> {
        Vec::new()
    }

    fn queue_stats_snapshot(&self) -> QueueSnapshot {
        QueueSnapshot::default()
    }

    fn update_status(&self, _message_id: &SpooledMessageId, _status: DeliveryStatus) {}

    fn set_next_retry_at(&self, _message_id: &SpooledMessageId, _next_retry_at: SystemTime) {}

    fn reset_server_index(&self, _message_id: &SpooledMessageId) {}

    fn remove(&self, _message_id: &SpooledMessageId) -> Option<DeliveryInfo> {
        None
    }

    fn dns_resolver(&self) -> &Option<Arc<dyn DnsResolver>> {
        &self.dns_resolver
    }

    fn spool(&self) -> &Option<Arc<dyn BackingStore>> {
        &self.spool
    }

    fn domains(&self) -> &DomainConfigRegistry {
        &self.domains_view
    }

    fn outbound_bindings(&self) -> OutboundBindingRegistry {
        OutboundBindingRegistry::default()
    }

    fn binding_health_status(&self, _name: &str) -> Option<BindingHealthStatus> {
        None
    }

    fn binding_enabled(&self, _name: &str) -> bool {
        true
    }

    fn set_binding_enabled(&self, _name: &str, _enabled: bool) -> Result<(), String> {
        Ok(())
    }

    fn request_process_now(&self) {}

    fn connection_stats(&self) -> ConnectionStats {
        ConnectionStats {
            queue_size: 0,
            rate_limit_domains: 0,
        }
    }

    fn update_rate_limit(
        &self,
        _domain: Option<String>,
        _messages_per_second: f64,
        _burst: u32,
    ) -> Result<(), String> {
        Ok(())
    }

    fn replace_rate_limit_config(&self, config: RateLimitConfig) {
        let mut guard = match self.replaced_rate_limit.lock() {
            Ok(guard) => guard,
            Err(err) => err.into_inner(),
        };
        *guard = Some(config);
    }

    fn replace_retry_policy(&self, policy: RetryPolicy) {
        let mut guard = match self.replaced_retry_policy.lock() {
            Ok(guard) => guard,
            Err(err) => err.into_inner(),
        };
        *guard = Some(policy);
    }

    fn replace_smtp_timeouts(&self, timeouts: SmtpTimeouts) {
        let mut guard = match self.replaced_smtp_timeouts.lock() {
            Ok(guard) => guard,
            Err(err) => err.into_inner(),
        };
        *guard = Some(timeouts);
    }

    fn replace_circuit_breaker_config(&self, config: CircuitBreakerConfig) {
        let mut guard = match self.replaced_circuit_breaker.lock() {
            Ok(guard) => guard,
            Err(err) => err.into_inner(),
        };
        *guard = Some(config);
    }

    fn replace_domains(&self, registry: DomainConfigRegistry) {
        let mut guard = match self.replaced_domains.lock() {
            Ok(guard) => guard,
            Err(err) => err.into_inner(),
        };
        *guard = Some(registry);
    }

    fn tls_policy(&self) -> TlsPolicy {
        self.tls_policy
    }
}

#[test]
#[cfg_attr(miri, ignore)]
fn reload_config_reports_restart_required_fields() -> TestResult {
    let _guard = ENV_LOCK.lock().map_err(|err| format!("lock env: {err}"))?;
    let temp_dir = tempfile::TempDir::new()?;
    let config_path = temp_dir.path().join("empath.config.ron");
    let config = r#"
Empath (
  smtp: (
    listeners: [],
  ),
  delivery: {
    "max_attempts": 7,
    "smtp_timeouts": (
      ehlo_secs: 5,
    ),
    "domains": {
      "example.com": (
        mx_override: "localhost:2525",
      ),
    },
    "rate_limit": (
      messages_per_second: 5.0,
      burst_size: 10,
    ),
    "circuit_breaker": (
      failure_threshold: 7,
    ),
  },
)
"#;
    std::fs::write(&config_path, config)?;
    let _env = EnvVarGuard::set("EMPATH_CONFIG", &config_path);

    let delivery = Arc::new(MockDelivery::default());
    let delivery_service: Arc<dyn DeliveryQueryService> = delivery.clone();
    let message = reload_config_from_disk(&delivery_service)?;

    assert!(message.contains("Applied:"));
    assert!(message.contains("Restart required"));
    assert_eq!(delivery.replaced_domains_len(), Some(1));
    assert_eq!(delivery.replaced_rate_limit_mps(), Some(5.0));
    assert_eq!(delivery.replaced_retry_policy_max_attempts(), Some(7));
    assert_eq!(delivery.replaced_smtp_timeouts_ehlo_secs(), Some(5));
    assert_eq!(delivery.replaced_circuit_breaker_threshold(), Some(7));
    Ok(())
}

#[test]
fn parse_message_id_accepts_ulid() -> TestResult {
    let message_id = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
    let parsed =
        parse_message_id(message_id).map_err(|err| format!("expected valid message id: {err}"))?;
    assert_eq!(parsed.to_string(), message_id);
    Ok(())
}

#[test]
fn parse_message_id_rejects_invalid_ulid() -> TestResult {
    let err = parse_message_id("not_a_valid_ulid")
        .err()
        .ok_or_else(|| io::Error::other("expected invalid message id to error"))?;
    assert!(err.to_string().contains("Invalid message ID"));
    Ok(())
}

#[test]
fn resolve_tls_policy_preview_prefers_explicit_policy() {
    let registry = DomainConfigRegistry::new();
    let config = DomainConfig {
        tls_policy: Some(TlsPolicy::Disabled),
        require_tls: true,
        ..DomainConfig::default()
    };
    registry.insert("example.com".to_string(), config);
    let resolved = resolve_tls_policy_preview(&registry, "example.com", TlsPolicy::Required);
    assert_eq!(resolved, TlsPolicy::Disabled);
}

#[test]
fn resolve_tls_policy_preview_requires_tls_when_flag_set() {
    let registry = DomainConfigRegistry::new();
    let config = DomainConfig {
        require_tls: true,
        ..DomainConfig::default()
    };
    registry.insert("example.com".to_string(), config);
    let resolved = resolve_tls_policy_preview(&registry, "example.com", TlsPolicy::Opportunistic);
    assert_eq!(resolved, TlsPolicy::Required);
}

#[test]
fn resolve_tls_policy_preview_falls_back_to_global() {
    let registry = DomainConfigRegistry::new();
    let resolved = resolve_tls_policy_preview(&registry, "example.com", TlsPolicy::Opportunistic);
    assert_eq!(resolved, TlsPolicy::Opportunistic);
}
