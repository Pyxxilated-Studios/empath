//! SMTP client connection pool for outbound delivery.

use std::{
    collections::VecDeque,
    hash::{Hash, Hasher},
    net::IpAddr,
    sync::Mutex,
    time::{Duration, Instant},
};

use dashmap::DashMap;
use empath_smtp::client::{ConnectConfig, SmtpClient};

use crate::types::SmtpPoolConfig;

#[derive(Debug, Clone, Eq)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "Pool keys need the full TLS/binding policy to prevent unsafe reuse."
)]
pub struct SmtpPoolKey {
    pub domain: String,
    pub server_address: String,
    pub local_ip: Option<IpAddr>,
    pub local_interface: Option<String>,
    pub attempt_tls: bool,
    pub require_tls: bool,
    pub accept_invalid_certs: bool,
    pub tls_active: bool,
}

impl PartialEq for SmtpPoolKey {
    fn eq(&self, other: &Self) -> bool {
        self.domain == other.domain
            && self.server_address == other.server_address
            && self.local_ip == other.local_ip
            && self.local_interface == other.local_interface
            && self.attempt_tls == other.attempt_tls
            && self.require_tls == other.require_tls
            && self.accept_invalid_certs == other.accept_invalid_certs
            && self.tls_active == other.tls_active
    }
}

impl Hash for SmtpPoolKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.domain.hash(state);
        self.server_address.hash(state);
        self.local_ip.hash(state);
        self.local_interface.hash(state);
        self.attempt_tls.hash(state);
        self.require_tls.hash(state);
        self.accept_invalid_certs.hash(state);
        self.tls_active.hash(state);
    }
}

impl SmtpPoolKey {
    #[must_use]
    #[allow(
        clippy::fn_params_excessive_bools,
        reason = "Pool keys need the full TLS/binding policy to prevent unsafe reuse."
    )]
    pub fn new(
        domain: String,
        server_address: String,
        connect_config: Option<&ConnectConfig>,
        attempt_tls: bool,
        require_tls: bool,
        accept_invalid_certs: bool,
        tls_active: bool,
    ) -> Self {
        let (local_ip, local_interface) = connect_config.map_or((None, None), |config| {
            (config.local_ip, config.local_interface.clone())
        });

        Self {
            domain,
            server_address,
            local_ip,
            local_interface,
            attempt_tls,
            require_tls,
            accept_invalid_certs,
            tls_active,
        }
    }
}

pub struct SmtpPoolRequest<'a> {
    pub domain: &'a str,
    pub server_address: &'a str,
    pub connect_config: Option<&'a ConnectConfig>,
    pub attempt_tls: bool,
    pub require_tls: bool,
    pub accept_invalid_certs: bool,
}

impl<'a> SmtpPoolRequest<'a> {
    #[must_use]
    pub const fn new(
        domain: &'a str,
        server_address: &'a str,
        connect_config: Option<&'a ConnectConfig>,
        attempt_tls: bool,
        require_tls: bool,
        accept_invalid_certs: bool,
    ) -> Self {
        Self {
            domain,
            server_address,
            connect_config,
            attempt_tls,
            require_tls,
            accept_invalid_certs,
        }
    }
}

struct PoolEntry {
    client: SmtpClient,
    last_used: Instant,
}

pub struct SmtpClientPool {
    config: SmtpPoolConfig,
    entries: DashMap<SmtpPoolKey, Mutex<VecDeque<PoolEntry>>>,
}

impl std::fmt::Debug for SmtpClientPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SmtpClientPool")
            .field("enabled", &self.config.enabled)
            .field("max_per_domain", &self.config.max_per_domain)
            .field("idle_ttl_secs", &self.config.idle_ttl_secs)
            .field("entries", &self.entries.len())
            .finish()
    }
}

impl Default for SmtpClientPool {
    fn default() -> Self {
        Self::new(SmtpPoolConfig::default())
    }
}

impl SmtpClientPool {
    #[must_use]
    pub fn new(config: SmtpPoolConfig) -> Self {
        Self {
            config,
            entries: DashMap::new(),
        }
    }

    #[must_use]
    pub const fn enabled(&self) -> bool {
        self.config.enabled
    }

    pub async fn acquire(
        &self,
        request: SmtpPoolRequest<'_>,
        metrics: Option<&empath_metrics::DeliveryMetrics>,
    ) -> Option<SmtpClient> {
        if !self.config.enabled {
            return None;
        }

        let key_tls = SmtpPoolKey::new(
            request.domain.to_string(),
            request.server_address.to_string(),
            request.connect_config,
            request.attempt_tls,
            request.require_tls,
            request.accept_invalid_certs,
            true,
        );
        let key_plain = SmtpPoolKey::new(
            request.domain.to_string(),
            request.server_address.to_string(),
            request.connect_config,
            request.attempt_tls,
            request.require_tls,
            request.accept_invalid_certs,
            false,
        );

        let preferred_keys = if request.attempt_tls {
            if request.require_tls {
                vec![key_tls]
            } else {
                vec![key_tls, key_plain]
            }
        } else {
            vec![key_plain]
        };

        for key in preferred_keys {
            if let Some(client) = self.try_take(&key, metrics).await {
                Self::record_hit(request.domain, metrics);
                return Some(client);
            }
        }

        Self::record_miss(request.domain, metrics);
        None
    }

    pub fn release(
        &self,
        key: SmtpPoolKey,
        client: SmtpClient,
        metrics: Option<&empath_metrics::DeliveryMetrics>,
    ) {
        if !self.config.enabled {
            return;
        }
        if self.config.max_per_domain == 0 {
            self.record_pool_size(&key.domain, metrics);
            return;
        }

        let domain = key.domain.clone();
        let entry = PoolEntry {
            client,
            last_used: Instant::now(),
        };

        let queue = self
            .entries
            .entry(key)
            .or_insert_with(|| Mutex::new(VecDeque::new()));

        let mut guard = queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if guard.len() >= self.config.max_per_domain {
            drop(entry);
            drop(guard);
            self.record_pool_size(&domain, metrics);
            return;
        }
        guard.push_back(entry);
        drop(guard);

        self.record_pool_size(&domain, metrics);
    }

    pub fn cleanup_idle(&self, metrics: Option<&empath_metrics::DeliveryMetrics>) -> usize {
        if !self.config.enabled {
            return 0;
        }

        let ttl = Duration::from_secs(self.config.idle_ttl_secs);
        let now = Instant::now();
        let mut removed = 0;

        let keys: Vec<SmtpPoolKey> = self
            .entries
            .iter()
            .map(|entry| entry.key().clone())
            .collect();
        let mut affected_domains = Vec::new();
        for key in keys {
            if let Some(queue) = self.entries.get(&key) {
                let mut guard = queue
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let before = guard.len();
                guard.retain(|entry| now.duration_since(entry.last_used) <= ttl);
                let after = guard.len();
                removed += before.saturating_sub(after);
                drop(guard);
                affected_domains.push(key.domain.clone());
            }

            if let Some(queue) = self.entries.get(&key) {
                let guard = queue
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if guard.is_empty() {
                    drop(guard);
                    self.entries.remove(&key);
                }
            }
        }

        affected_domains.sort();
        affected_domains.dedup();
        for domain in affected_domains {
            self.record_pool_size(&domain, metrics);
        }

        removed
    }

    async fn try_take(
        &self,
        key: &SmtpPoolKey,
        metrics: Option<&empath_metrics::DeliveryMetrics>,
    ) -> Option<SmtpClient> {
        let ttl = Duration::from_secs(self.config.idle_ttl_secs);
        loop {
            let entry = {
                let queue = self.entries.get(key)?;
                let mut guard = queue
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let entry = guard.pop_front();
                drop(guard);
                entry
            };

            let mut entry = entry?;

            if entry.last_used.elapsed() > ttl {
                self.record_pool_size(&key.domain, metrics);
                continue;
            }

            if self.config.health_check_on_acquire {
                match entry.client.noop().await {
                    Ok(response) if response.is_success() => {}
                    _ => {
                        self.record_pool_size(&key.domain, metrics);
                        continue;
                    }
                }
            }

            self.record_pool_size(&key.domain, metrics);
            return Some(entry.client);
        }
    }

    fn record_hit(domain: &str, metrics: Option<&empath_metrics::DeliveryMetrics>) {
        if let Some(metrics) = metrics {
            metrics.record_smtp_pool_hit(domain);
        }
    }

    fn record_miss(domain: &str, metrics: Option<&empath_metrics::DeliveryMetrics>) {
        if let Some(metrics) = metrics {
            metrics.record_smtp_pool_miss(domain);
        }
    }

    fn record_pool_size(&self, domain: &str, metrics: Option<&empath_metrics::DeliveryMetrics>) {
        if let Some(metrics) = metrics {
            metrics.update_smtp_pool_size(domain, self.pool_size_for_domain(domain));
        }
    }

    fn pool_size_for_domain(&self, domain: &str) -> u64 {
        self.entries
            .iter()
            .filter(|entry| entry.key().domain == domain)
            .map(|entry| {
                let guard = entry
                    .value()
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                u64::try_from(guard.len()).unwrap_or(0)
            })
            .sum()
    }
}
