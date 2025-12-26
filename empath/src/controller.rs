use std::{
    path::PathBuf,
    sync::{Arc, LazyLock, RwLock},
    time::{Duration, SystemTime},
};

use empath_common::{
    Signal, context::Capability, controller::Controller, internal, logging, tracing,
};
use empath_control::{
    ControlAuthConfig, ControlRateLimitConfig, ControlServer, DEFAULT_CONTROL_SOCKET,
};
use empath_ffi::modules::{self, Module};
use empath_health::{HealthChecker, HealthConfig, HealthServer};
use empath_smtp::{Smtp, TlsContextRegistry};
use empath_tracing::traced;
use serde::Deserialize;
use tokio::{sync::broadcast, task::JoinSet, time};

use crate::control_handler::EmpathControlHandler;

#[allow(
    clippy::unsafe_derive_deserialize,
    reason = "The unsafe aspects have nothing to do with the struct"
)]
#[derive(Default, Deserialize)]
pub struct Empath {
    #[serde(alias = "smtp")]
    smtp_controller: Controller<Smtp>,
    #[serde(alias = "module", default)]
    modules: Vec<Module>,
    #[serde(alias = "spool", default)]
    spool: empath_spool::SpoolConfig,
    #[serde(alias = "delivery", default)]
    delivery: empath_delivery::DeliveryProcessor,
    /// Path to the control socket (optional, defaults to /tmp/empath.sock)
    #[serde(alias = "control_socket", default = "default_control_socket")]
    control_socket_path: String,
    /// Control socket authentication configuration (optional)
    #[serde(alias = "control_auth", default)]
    control_auth: Option<ControlAuthConfig>,
    /// Control socket command rate limiting configuration (optional)
    #[serde(alias = "control_rate_limit", default)]
    control_rate_limit: Option<ControlRateLimitConfig>,
    /// Metrics configuration
    #[serde(alias = "metrics", default)]
    metrics: empath_metrics::MetricsConfig,
    /// Health check configuration
    #[serde(alias = "health", default)]
    health: HealthConfig,
    /// Audit logging configuration
    #[serde(alias = "audit", default)]
    audit: empath_common::audit::AuditConfig,
    /// Shutdown drain timeout in seconds.
    #[serde(
        alias = "shutdown_timeout_secs",
        default = "default_shutdown_timeout_secs"
    )]
    shutdown_timeout_secs: u64,
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
    /// Initialize the audit logging system with the configured settings
    pub fn init_audit(&self) {
        empath_common::audit::init(self.audit.clone());
        tracing::info!(
            enabled = self.audit.enabled,
            redact_sender = self.audit.redact_sender,
            redact_recipients = self.audit.redact_recipients,
            "Audit logging initialized"
        );
    }

    /// Run this controller, and everything it controls
    ///
    /// # Errors
    ///
    /// This function will return an error if any of the configured modules fail
    /// to initialise.
    #[allow(clippy::too_many_lines)]
    #[traced(instrument(level = tracing::Level::TRACE, skip_all, err), timing(precision = "s"))]
    pub async fn run(mut self) -> anyhow::Result<()> {
        // Initialize logging with configured OTLP endpoint for traces
        // This uses the same endpoint as metrics (just different path: /v1/traces vs /v1/metrics)
        logging::init(Some(&self.metrics.endpoint))?;

        internal!("Controller running");

        // Initialize metrics
        if let Err(e) = empath_metrics::init_metrics(&self.metrics) {
            tracing::warn!(error = %e, "Failed to initialize metrics");
        }

        // Initialize health checker
        let health_checker = Arc::new(HealthChecker::new(self.health.max_queue_size));

        let spool_path: Option<PathBuf> = self.spool.path().map(std::path::Path::to_path_buf);

        // Initialize the spool from configuration
        let spool = self.spool.into_spool()?;

        // Mark spool as ready after successful initialization
        health_checker.set_spool_ready(true);

        // Extract the backing store for SMTP and delivery
        let backing_store = spool.backing_store();

        // Initialize delivery controller first to get the DNS resolver
        // Use default DNS resolver (None = HickoryDnsResolver)
        self.delivery.init(backing_store.clone(), None)?;

        modules::set_outbound_bindings(self.delivery.outbound_bindings.names());
        modules::init(self.modules)?;

        // Mark delivery and DNS as ready after successful initialization
        health_checker.set_delivery_ready(true);
        health_checker.set_dns_ready(true);

        // Create SPF DNS adapter from the delivery DNS resolver if available
        let spf_dns_resolver = self.delivery.dns_resolver().as_ref().map(|resolver| {
            Arc::new(empath_delivery::SpfDnsAdapter::new(Arc::clone(resolver)))
                as Arc<dyn empath_smtp::spf::SpfDnsResolver>
        });

        // Configure SMTP with spool and SPF DNS resolver
        let tls_registry = Arc::new(TlsContextRegistry::new());
        let smtp_capabilities: Arc<RwLock<Vec<Capability>>> = Arc::new(RwLock::new(Vec::new()));
        let spf_resolver_clone = spf_dns_resolver.clone();
        let registry_for_args = Arc::clone(&tls_registry);
        let capabilities_for_args = Arc::clone(&smtp_capabilities);
        self.smtp_controller.map_args(move |args| {
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
        });

        self.smtp_controller.init()?;

        // Mark SMTP as ready after successful initialization
        health_checker.set_smtp_ready(true);

        // Create control server
        let delivery_arc = Arc::new(self.delivery);
        let delivery_service = delivery_arc.clone();
        let control_handler = Arc::new(EmpathControlHandler::new(
            delivery_service,
            Some(Arc::clone(&tls_registry)),
            Some(Arc::clone(&smtp_capabilities)),
        ));
        let control_server = ControlServer::with_auth_and_rate_limit(
            &self.control_socket_path,
            control_handler,
            self.control_auth.clone(),
            self.control_rate_limit.clone(),
        )
        .map_err(|e| anyhow::anyhow!("Failed to create control server: {e}"))?;

        internal!(
            "Control server will listen on: {}",
            self.control_socket_path
        );

        // Create health server (if enabled)
        let health_server = if self.health.enabled {
            match HealthServer::new(self.health, health_checker).await {
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
        };

        let mut join_set: JoinSet<(&'static str, anyhow::Result<()>)> = JoinSet::new();

        if let Some(metrics) = empath_metrics::try_metrics() {
            let mut shutdown_rx = SHUTDOWN_BROADCAST.subscribe();
            let spool_path = spool_path.clone();
            let tls_registry = Arc::clone(&tls_registry);
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

        let smtp_controller = self.smtp_controller;
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

        let delivery_handle = Arc::clone(&delivery_arc);
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

        let mut first_error: Option<anyhow::Error> = None;
        if let Some(result) = join_set.join_next().await {
            match result {
                Ok((name, res)) => {
                    internal!(level = INFO, "Service {name} exited");
                    if let Err(err) = res {
                        first_error = Some(err);
                    }
                }
                Err(join_err) => {
                    first_error = Some(anyhow::anyhow!("Join error: {join_err}"));
                }
            }
        }

        let _ = SHUTDOWN_BROADCAST.send(Signal::Shutdown);

        let deadline =
            std::time::Instant::now() + std::time::Duration::from_secs(self.shutdown_timeout_secs);
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

        internal!("Shutting down...");

        first_error.map_or(Ok(()), Err)
    }

    /// Snapshot delivery-related config needed for in-process reloads.
    #[must_use]
    pub fn delivery_reload_settings(
        &self,
    ) -> (
        empath_delivery::DomainConfigRegistry,
        empath_delivery::RateLimitConfig,
    ) {
        (
            self.delivery.domains.clone(),
            self.delivery.rate_limit.clone(),
        )
    }
}

fn record_tls_watchdog_metrics(metrics: &empath_metrics::Metrics, registry: &TlsContextRegistry) {
    let now_secs = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let contexts = registry.list_contexts();

    let mut expired = 0;
    let mut parse_errors = 0;
    let mut min_seconds_until_expiry: Option<u64> = None;

    for ctx in &contexts {
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
        u64::try_from(contexts.len()).unwrap_or(u64::MAX),
        min_seconds_until_expiry,
        expired,
        parse_errors,
    );
}
