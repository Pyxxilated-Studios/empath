use std::path::PathBuf;
use std::sync::{Arc, LazyLock, RwLock};
use std::time::{Duration, SystemTime};

use empath_common::audit::{AuditConfig, AuditLogger, AuditLoggerBuilder};
use empath_common::context::Capability;
use empath_common::controller::Controller;
use empath_common::{Signal, internal, logging, tracing};
use empath_control::{
    ControlAuthConfig, ControlRateLimitConfig, ControlServer, DEFAULT_CONTROL_SOCKET,
};
use empath_delivery::{
    CircuitBreakerConfig, DeliveryProcessor, DomainConfigRegistry, RateLimitConfig, RetryPolicy,
    SmtpTimeouts, SpfDnsAdapter,
};
use empath_ffi::modules::traits::Module;
use empath_ffi::modules::{ModuleDispatchConfig, ModuleRegistryBuilder};
use empath_health::{HealthChecker, HealthConfig, HealthServer};
use empath_metrics::{Metrics, MetricsConfig, init_metrics, metrics};
use empath_smtp::spf::SpfDnsResolver;
use empath_smtp::{Smtp, TlsContextRegistry};
use empath_spool::config::SpoolType;
use empath_spool::{BackingStore, SpoolConfig};
use empath_tracing::traced;
use serde::Deserialize;
use tokio::sync::broadcast;
use tokio::task::JoinSet;
use tokio::time;

use crate::control_handler::EmpathControlHandler;

#[derive(Default)]
pub struct Empath {
    smtp_controller: Controller<Smtp>,
    modules: Vec<Box<dyn Module>>,
    module_dispatch: ModuleDispatchConfig,
    spool: SpoolConfig,
    delivery: DeliveryProcessor,
    /// Path to the control socket (optional, defaults to /tmp/empath.sock)
    control_socket_path: String,
    /// Control socket authentication configuration (optional)
    control_auth: Option<ControlAuthConfig>,
    /// Control socket command rate limiting configuration (optional)
    control_rate_limit: Option<ControlRateLimitConfig>,
    /// Metrics configuration
    metrics: MetricsConfig,
    /// Health check configuration
    health: HealthConfig,
    /// Audit logging configuration
    audit: AuditConfig,
    /// Shutdown drain timeout in seconds.
    shutdown_timeout_secs: u64,
}

#[derive(Deserialize)]
#[serde(rename = "Empath")]
struct EmpathConfig {
    #[serde(alias = "smtp")]
    smtp_controller: Controller<Smtp>,
    #[serde(alias = "module", default)]
    modules: Vec<Box<dyn Module>>,
    #[serde(default)]
    module_dispatch: ModuleDispatchConfig,
    #[serde(alias = "spool", default)]
    spool: SpoolConfig,
    #[serde(alias = "delivery", default)]
    delivery: DeliveryProcessor,
    #[serde(alias = "control_socket", default = "default_control_socket")]
    control_socket_path: String,
    #[serde(alias = "control_auth", default)]
    control_auth: Option<ControlAuthConfig>,
    #[serde(alias = "control_rate_limit", default)]
    control_rate_limit: Option<ControlRateLimitConfig>,
    #[serde(alias = "metrics", default)]
    metrics: MetricsConfig,
    #[serde(alias = "health", default)]
    health: HealthConfig,
    #[serde(alias = "audit", default)]
    audit: AuditConfig,
    #[serde(
        alias = "shutdown_timeout_secs",
        default = "default_shutdown_timeout_secs"
    )]
    shutdown_timeout_secs: u64,
}

impl<'de> Deserialize<'de> for Empath {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let config = EmpathConfig::deserialize(deserializer)?;
        Ok(Self::from(config))
    }
}

impl From<EmpathConfig> for Empath {
    fn from(config: EmpathConfig) -> Self {
        Self {
            smtp_controller: config.smtp_controller,
            modules: config.modules,
            module_dispatch: config.module_dispatch,
            spool: config.spool,
            delivery: config.delivery,
            control_socket_path: config.control_socket_path,
            control_auth: config.control_auth,
            control_rate_limit: config.control_rate_limit,
            metrics: config.metrics,
            health: config.health,
            audit: config.audit,
            shutdown_timeout_secs: config.shutdown_timeout_secs,
        }
    }
}

fn default_control_socket() -> String {
    DEFAULT_CONTROL_SOCKET.to_string()
}

const fn default_shutdown_timeout_secs() -> u64 {
    10
}

pub static SHUTDOWN_BROADCAST: LazyLock<broadcast::Sender<Signal>> = LazyLock::new(|| {
    let (sender, _receiver) = broadcast::channel(64);
    sender
});

#[traced(instrument(level = tracing::Level::TRACE))]
async fn shutdown() -> anyhow::Result<()> {
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;

    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            internal!("CTRL+C entered -- Enter it again to force shutdown");
        }
        _ = terminate.recv() => {
            internal!("Terminate Signal received, shutting down");
        }
    };

    let mut receiver = SHUTDOWN_BROADCAST.subscribe();

    SHUTDOWN_BROADCAST
        .send(Signal::Shutdown)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Interrupted, e.to_string()))?;

    loop {
        tokio::select! {
            sig = receiver.recv() => {
                match sig {
                    Ok(s) => tracing::debug!("Received {s:?}"),
                    Err(broadcast::error::RecvError::Closed) => break,
                    Err(e) => tracing::debug!("Received: {e:?}"),
                }
            }

            _ = tokio::signal::ctrl_c() => {
                break;
            }
        }
    }

    Ok(())
}

impl Empath {
    /// Run this controller, and everything it controls
    ///
    /// # Errors
    ///
    /// This function will return an error if any of the configured modules fail
    /// to initialise.
    #[traced(instrument(level = tracing::Level::TRACE, skip_all, err), timing(precision = "s"))]
    pub async fn run(mut self) -> anyhow::Result<()> {
        let logging_handle = Arc::new(self.init_logging_and_metrics()?);

        internal!("Controller running");

        let audit_logger = AuditLoggerBuilder::new(self.audit.clone()).build();
        tracing::info!(
            enabled = self.audit.enabled,
            redact_sender = self.audit.redaction.redact_sender,
            redact_recipients = self.audit.redaction.redact_recipients,
            "Audit logging initialized"
        );

        // Initialize health checker
        let health_checker = Arc::new(HealthChecker::new(self.health.max_queue_size));

        let spool_config = std::mem::take(&mut self.spool);
        let (spool_path, spool) = Self::init_spool(spool_config)?;
        health_checker.set_spool_ready(true);

        // Extract the backing store for SMTP and delivery
        let backing_store = spool.backing_store();

        self.delivery.set_audit_logger(Arc::clone(&audit_logger));
        self.init_delivery(backing_store.clone())?;
        let modules = std::mem::take(&mut self.modules);
        let module_dispatch = std::mem::take(&mut self.module_dispatch);
        ModuleRegistryBuilder::new()
            .with_modules(modules)
            .with_outbound_bindings(self.delivery.outbound_bindings.names())
            .with_dispatch_config(module_dispatch)
            .build()?
            .install()?;
        health_checker.set_delivery_ready(true);
        health_checker.set_dns_ready(true);

        let spf_dns_resolver = Self::spf_dns_resolver(&self.delivery);
        let tls_registry = Arc::new(TlsContextRegistry::new());
        let smtp_capabilities: Arc<RwLock<Vec<Capability>>> = Arc::new(RwLock::new(Vec::new()));
        Self::init_smtp_controller(
            &mut self.smtp_controller,
            backing_store,
            spf_dns_resolver.as_ref(),
            &tls_registry,
            &smtp_capabilities,
            Arc::clone(&audit_logger),
        )?;

        // Mark SMTP as ready after successful initialization
        health_checker.set_smtp_ready(true);

        // Create control server
        let delivery = std::mem::take(&mut self.delivery);
        let delivery_arc = Arc::new(delivery);
        let delivery_service = delivery_arc.clone();
        let control_handler = Arc::new(EmpathControlHandler::new(
            delivery_service,
            Some(Arc::clone(&tls_registry)),
            Some(Arc::clone(&smtp_capabilities)),
            Arc::clone(&logging_handle),
        ));
        let control_server = self.build_control_server(control_handler)?;

        internal!(
            "Control server will listen on: {}",
            self.control_socket_path
        );

        // Create health server (if enabled)
        let health_server = self.build_health_server(health_checker).await;

        let mut join_set: JoinSet<(&'static str, anyhow::Result<()>)> = JoinSet::new();

        Self::spawn_metrics_task(&mut join_set, Arc::clone(&tls_registry), spool_path);
        Self::spawn_service_tasks(
            &mut join_set,
            self.smtp_controller,
            spool,
            &delivery_arc,
            control_server,
            health_server,
        );
        let mut first_error = Self::await_first_exit(&mut join_set).await;

        let _ = SHUTDOWN_BROADCAST.send(Signal::Shutdown);

        first_error =
            Self::drain_shutdown(&mut join_set, self.shutdown_timeout_secs, first_error).await;

        internal!("Shutting down...");

        first_error.map_or(Ok(()), Err)
    }

    /// Snapshot delivery-related config needed for in-process reloads.
    #[must_use]
    pub fn delivery_reload_settings(
        &self,
    ) -> (
        DomainConfigRegistry,
        RateLimitConfig,
        RetryPolicy,
        SmtpTimeouts,
        CircuitBreakerConfig,
    ) {
        (
            self.delivery.domains.clone(),
            self.delivery.rate_limit.clone(),
            self.delivery.retry_policy.read().clone(),
            self.delivery.smtp_timeouts.read().clone(),
            self.delivery.circuit_breaker.read().clone(),
        )
    }

    fn init_logging_and_metrics(&self) -> anyhow::Result<logging::LoggingHandle> {
        // Initialize logging with configured OTLP endpoint for traces
        // This uses the same endpoint as metrics (just different path: /v1/traces vs /v1/metrics)
        let logging_handle = logging::LoggingConfigBuilder::new()
            .with_otlp_endpoint(self.metrics.endpoint.clone())
            .init()?;

        // Initialize metrics
        if let Err(e) = init_metrics(&self.metrics) {
            tracing::warn!(error = %e, "Failed to initialize metrics");
        }

        Ok(logging_handle)
    }

    fn init_spool(spool_config: SpoolConfig) -> anyhow::Result<(Option<PathBuf>, SpoolType)> {
        let spool_path: Option<PathBuf> = spool_config.path().map(std::path::Path::to_path_buf);
        let spool = spool_config.into_spool()?;
        Ok((spool_path, spool))
    }

    fn init_delivery(&mut self, backing_store: Arc<dyn BackingStore>) -> anyhow::Result<()> {
        // Initialize delivery controller first to get the DNS resolver
        // Use default DNS resolver (None = HickoryDnsResolver)
        self.delivery.init(backing_store, None)?;
        Ok(())
    }

    fn spf_dns_resolver(delivery: &DeliveryProcessor) -> Option<Arc<dyn SpfDnsResolver>> {
        delivery.dns_resolver().as_ref().map(|resolver| {
            Arc::new(SpfDnsAdapter::new(Arc::clone(resolver))) as Arc<dyn SpfDnsResolver>
        })
    }

    fn init_smtp_controller(
        smtp_controller: &mut Controller<Smtp>,
        backing_store: Arc<dyn BackingStore>,
        spf_dns_resolver: Option<&Arc<dyn SpfDnsResolver>>,
        tls_registry: &Arc<TlsContextRegistry>,
        smtp_capabilities: &Arc<RwLock<Vec<Capability>>>,
        audit_logger: Arc<dyn AuditLogger>,
    ) -> anyhow::Result<()> {
        // Configure SMTP with spool and SPF DNS resolver
        let spf_resolver_clone = spf_dns_resolver.cloned();
        let registry_for_args = Arc::clone(tls_registry);
        let capabilities_for_args = Arc::clone(smtp_capabilities);
        smtp_controller.map_args(move |args| {
            let capabilities = args.capabilities().to_vec();
            let mut args = args.with_spool(backing_store.clone());
            if let Some(resolver) = spf_resolver_clone.clone() {
                args = args.with_spf_dns_resolver(resolver);
            }
            if let Ok(mut guard) = capabilities_for_args.write()
                && guard.is_empty()
            {
                *guard = capabilities;
            }
            args = args.with_capabilities_state(Arc::clone(&capabilities_for_args));
            args.with_tls_context_registry(&registry_for_args)
                .with_audit_logger(Arc::clone(&audit_logger))
        });

        smtp_controller.init()?;
        Ok(())
    }

    fn build_control_server(
        &self,
        control_handler: Arc<EmpathControlHandler>,
    ) -> anyhow::Result<ControlServer> {
        ControlServer::with_auth_and_rate_limit(
            &self.control_socket_path,
            control_handler,
            self.control_auth.clone(),
            self.control_rate_limit.clone(),
        )
        .map_err(|e| anyhow::anyhow!("Failed to create control server: {e}"))
    }

    async fn build_health_server(
        &self,
        health_checker: Arc<HealthChecker>,
    ) -> Option<HealthServer> {
        if self.health.enabled {
            match HealthServer::new(self.health.clone(), health_checker).await {
                Ok(server) => {
                    internal!("Health check server initialized");
                    Some(server)
                }
                Err(e) => {
                    tracing::warn!(error = %e, "Failed to create health server");
                    None
                }
            }
        } else {
            tracing::info!("Health check server is disabled");
            None
        }
    }

    fn spawn_metrics_task(
        join_set: &mut JoinSet<(&'static str, anyhow::Result<()>)>,
        tls_registry: Arc<TlsContextRegistry>,
        spool_path: Option<PathBuf>,
    ) {
        if let Some(metrics) = metrics() {
            let mut shutdown_rx = SHUTDOWN_BROADCAST.subscribe();
            join_set.spawn(async move {
                let mut interval = time::interval(Duration::from_secs(15));
                loop {
                    tokio::select! {
                        _ = interval.tick() => {
                            metrics.system.update_process_metrics().await;
                            metrics.system.update_network_metrics().await;
                            if let Some(path) = spool_path.as_deref() {
                                metrics.system.update_spool_metrics(path);
                            }
                            let connections = metrics.smtp.active_connections()
                                + metrics.delivery.active_connections_count();
                            metrics.system.record_network_connections(connections);
                            record_tls_watchdog_metrics(metrics, &tls_registry);
                        }
                        _ = shutdown_rx.recv() => break,
                    }
                }
                ("metrics", Ok(()))
            });
        }
    }

    fn spawn_service_tasks(
        join_set: &mut JoinSet<(&'static str, anyhow::Result<()>)>,
        smtp_controller: Controller<Smtp>,
        spool: SpoolType,
        delivery_arc: &Arc<DeliveryProcessor>,
        control_server: ControlServer,
        health_server: Option<HealthServer>,
    ) {
        join_set.spawn(async move {
            (
                "smtp",
                smtp_controller
                    .control(vec![SHUTDOWN_BROADCAST.subscribe()])
                    .await
                    .map_err(anyhow::Error::from),
            )
        });

        join_set.spawn(async move {
            (
                "spool",
                spool
                    .serve(SHUTDOWN_BROADCAST.subscribe())
                    .await
                    .map_err(anyhow::Error::from),
            )
        });

        let delivery_handle = Arc::clone(delivery_arc);
        join_set.spawn(async move {
            (
                "delivery",
                delivery_handle
                    .serve(SHUTDOWN_BROADCAST.subscribe())
                    .await
                    .map_err(anyhow::Error::from),
            )
        });

        join_set.spawn(async move {
            (
                "control",
                control_server
                    .serve(SHUTDOWN_BROADCAST.subscribe())
                    .await
                    .map_err(|e| anyhow::anyhow!("Control server error: {e}")),
            )
        });

        if let Some(server) = health_server {
            join_set.spawn(async move {
                (
                    "health",
                    server
                        .serve(SHUTDOWN_BROADCAST.subscribe())
                        .await
                        .map_err(|e| anyhow::anyhow!("Health server error: {e}")),
                )
            });
        }

        join_set.spawn(async move { ("shutdown", shutdown().await) });
    }

    async fn await_first_exit(
        join_set: &mut JoinSet<(&'static str, anyhow::Result<()>)>,
    ) -> Option<anyhow::Error> {
        join_set.join_next().await.map_or_else(
            || None,
            |result| match result {
                Ok((name, res)) => {
                    internal!(level = INFO, "Service {name} exited");
                    res.err()
                }
                Err(join_err) => Some(anyhow::anyhow!("Join error: {join_err}")),
            },
        )
    }

    async fn drain_shutdown(
        join_set: &mut JoinSet<(&'static str, anyhow::Result<()>)>,
        shutdown_timeout_secs: u64,
        mut first_error: Option<anyhow::Error>,
    ) -> Option<anyhow::Error> {
        let deadline =
            std::time::Instant::now() + std::time::Duration::from_secs(shutdown_timeout_secs);
        while !join_set.is_empty() {
            let now = std::time::Instant::now();
            if now >= deadline {
                internal!(
                    level = WARN,
                    "Shutdown drain timeout reached, aborting tasks"
                );
                join_set.abort_all();
                break;
            }

            let timeout = deadline.saturating_duration_since(now);
            match tokio::time::timeout(timeout, join_set.join_next()).await {
                Ok(Some(result)) => match result {
                    Ok((name, res)) => {
                        internal!(level = DEBUG, "Service {name} exited");
                        if first_error.is_none()
                            && let Err(err) = res
                        {
                            first_error = Some(err);
                        }
                    }
                    Err(join_err) => {
                        if first_error.is_none() {
                            first_error =
                                Some(anyhow::anyhow!("Join error after shutdown: {join_err}"));
                        }
                    }
                },
                Ok(None) => break,
                Err(_) => {
                    internal!(level = WARN, "Shutdown drain timed out, aborting tasks");
                    join_set.abort_all();
                    break;
                }
            }
        }

        first_error
    }
}

fn record_tls_watchdog_metrics(metrics: &Metrics, registry: &TlsContextRegistry) {
    let now_secs = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let profiles = registry.list_profiles();

    let mut expired = 0;
    let mut parse_errors = 0;
    let mut min_seconds_until_expiry: Option<u64> = None;

    for profile in &profiles {
        let ctx = &profile.context;
        if ctx.parse_error.is_some() {
            parse_errors += 1;
            continue;
        }

        let Some(not_after) = ctx.not_after else {
            parse_errors += 1;
            continue;
        };

        if not_after <= now_secs {
            expired += 1;
            min_seconds_until_expiry = Some(0);
            continue;
        }

        let seconds_until = not_after.saturating_sub(now_secs);
        min_seconds_until_expiry = Some(
            min_seconds_until_expiry.map_or(seconds_until, |current| current.min(seconds_until)),
        );
    }

    metrics.smtp.record_tls_cert_status(
        u64::try_from(profiles.len()).unwrap_or(u64::MAX),
        min_seconds_until_expiry,
        expired,
        parse_errors,
    );
}

#[cfg(test)]
mod tests {
    use anyhow::{Result, ensure};
    use empath_control::DEFAULT_CONTROL_SOCKET;
    use empath_delivery::{
        CircuitBreakerConfig, DomainConfig, DomainRateLimit, RateLimitConfig, RetryPolicy,
        SmtpTimeouts, TlsPolicy,
    };

    use super::{Empath, default_control_socket, default_shutdown_timeout_secs};

    #[test]
    fn default_control_socket_matches_const() -> Result<()> {
        ensure!(
            default_control_socket() == DEFAULT_CONTROL_SOCKET,
            "expected default control socket path to match constant"
        );
        Ok(())
    }

    #[test]
    fn default_shutdown_timeout_is_expected() -> Result<()> {
        ensure!(
            default_shutdown_timeout_secs() == 10,
            "expected default shutdown timeout to be 10 seconds"
        );
        Ok(())
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn delivery_reload_settings_snapshots_delivery_config() -> Result<()> {
        let mut empath = Empath::default();
        let domain = DomainConfig {
            mx_override: Some("smtp.example.com:2525".to_string()),
            require_tls: true,
            accept_invalid_certs: Some(true),
            tls_policy: Some(TlsPolicy::Required),
            max_connections: Some(5),
            ..DomainConfig::default()
        };
        empath
            .delivery
            .domains
            .insert("example.com".to_string(), domain);

        let mut rate_limit = RateLimitConfig {
            messages_per_second: 3.5,
            burst_size: 12,
            ..RateLimitConfig::default()
        };
        rate_limit.domain_limits.insert(
            "example.com".to_string(),
            DomainRateLimit {
                messages_per_second: 1.5,
                burst_size: 3,
            },
        );
        empath.delivery.rate_limit = rate_limit;

        let retry_policy = RetryPolicy {
            max_attempts: 5,
            base_retry_delay_secs: 42,
            max_retry_delay_secs: 600,
            retry_jitter_factor: 0.25,
        };
        *empath.delivery.retry_policy.write() = retry_policy;

        let timeouts = SmtpTimeouts {
            connect_secs: 11,
            ehlo_secs: 12,
            starttls_secs: 13,
            mail_from_secs: 14,
            rcpt_to_secs: 15,
            data_secs: 16,
            quit_secs: 17,
        };
        *empath.delivery.smtp_timeouts.write() = timeouts;

        let circuit = CircuitBreakerConfig {
            failure_threshold: 7,
            failure_window_secs: 33,
            timeout_secs: 77,
            success_threshold: 2,
            ..CircuitBreakerConfig::default()
        };
        *empath.delivery.circuit_breaker.write() = circuit;

        let (domains, rate_limit, retry_policy, timeouts, circuit) =
            empath.delivery_reload_settings();

        let (require_tls, accept_invalid_certs, tls_policy, mx_override) = {
            let entry = domains
                .get("example.com")
                .ok_or_else(|| anyhow::anyhow!("missing domain config"))?;
            let require_tls = entry.require_tls;
            let accept_invalid_certs = entry.accept_invalid_certs;
            let tls_policy = entry.tls_policy;
            let mx_override = entry.mx_override.clone();
            drop(entry);
            (require_tls, accept_invalid_certs, tls_policy, mx_override)
        };
        ensure!(
            require_tls
                && accept_invalid_certs == Some(true)
                && tls_policy == Some(TlsPolicy::Required),
            "domain configuration not preserved"
        );
        ensure!(
            mx_override.as_deref() == Some("smtp.example.com:2525"),
            "domain MX override not preserved"
        );

        ensure!(
            (rate_limit.messages_per_second - 3.5).abs() < f64::EPSILON
                && rate_limit.burst_size == 12,
            "rate limit defaults not preserved"
        );
        let rate_override = rate_limit
            .domain_limits
            .get("example.com")
            .ok_or_else(|| anyhow::anyhow!("missing per-domain rate limit"))?;
        ensure!(
            (rate_override.messages_per_second - 1.5).abs() < f64::EPSILON
                && rate_override.burst_size == 3,
            "per-domain rate limit not preserved"
        );

        ensure!(
            retry_policy.max_attempts == 5
                && retry_policy.base_retry_delay_secs == 42
                && retry_policy.max_retry_delay_secs == 600
                && (retry_policy.retry_jitter_factor - 0.25).abs() < f64::EPSILON,
            "retry policy not preserved"
        );
        ensure!(
            timeouts.connect_secs == 11
                && timeouts.ehlo_secs == 12
                && timeouts.starttls_secs == 13
                && timeouts.mail_from_secs == 14
                && timeouts.rcpt_to_secs == 15
                && timeouts.data_secs == 16
                && timeouts.quit_secs == 17,
            "SMTP timeouts not preserved"
        );
        ensure!(
            circuit.failure_threshold == 7
                && circuit.failure_window_secs == 33
                && circuit.timeout_secs == 77
                && circuit.success_threshold == 2,
            "circuit breaker config not preserved"
        );

        Ok(())
    }
}
