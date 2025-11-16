use std::sync::{Arc, LazyLock};

use empath_common::{Signal, controller::Controller, internal, logging, tracing};
use empath_control::{ControlAuthConfig, ControlServer, DEFAULT_CONTROL_SOCKET};
use empath_ffi::modules::{self, Module};
use empath_health::{HealthChecker, HealthConfig, HealthServer};
use empath_smtp::Smtp;
use empath_tracing::traced;
use serde::Deserialize;
use tokio::sync::broadcast;

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
    /// Metrics configuration
    #[serde(alias = "metrics", default)]
    metrics: empath_metrics::MetricsConfig,
    /// Health check configuration
    #[serde(alias = "health", default)]
    health: HealthConfig,
    /// Audit logging configuration
    #[serde(alias = "audit", default)]
    audit: empath_common::audit::AuditConfig,
}

fn default_control_socket() -> String {
    DEFAULT_CONTROL_SOCKET.to_string()
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
    #[traced(instrument(level = tracing::Level::TRACE, skip_all, err), timing(precision = "s"))]
    pub async fn run(mut self) -> anyhow::Result<()> {
        logging::init()?;

        internal!("Controller running");

        // Initialize metrics
        if let Err(e) = empath_metrics::init_metrics(&self.metrics) {
            tracing::warn!(error = %e, "Failed to initialize metrics");
        }

        modules::init(self.modules)?;

        // Initialize health checker
        let health_checker = Arc::new(HealthChecker::new(self.health.max_queue_size));

        // Initialize the spool from configuration
        let spool = self.spool.into_spool()?;

        // Mark spool as ready after successful initialization
        health_checker.set_spool_ready(true);

        // Extract the backing store for SMTP and delivery
        let backing_store = spool.backing_store();

        self.smtp_controller
            .map_args(|args| args.with_spool(backing_store.clone()));

        self.smtp_controller.init()?;

        // Mark SMTP as ready after successful initialization
        health_checker.set_smtp_ready(true);

        // Initialize delivery controller with the same backing store and spool path
        self.delivery.init(backing_store)?;

        // Mark delivery and DNS as ready after successful initialization
        health_checker.set_delivery_ready(true);
        health_checker.set_dns_ready(true);

        // Create control server
        let delivery_arc = Arc::new(self.delivery);
        let delivery_service: Arc<dyn empath_delivery::DeliveryQueryService> = delivery_arc.clone();
        let control_handler = Arc::new(crate::control_handler::EmpathControlHandler::new(
            delivery_service,
        ));
        let control_server = ControlServer::with_auth(
            &self.control_socket_path,
            control_handler,
            self.control_auth.clone(),
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

        let ret = if let Some(health_server) = health_server {
            tokio::select! {
                r = self.smtp_controller.control(vec![SHUTDOWN_BROADCAST.subscribe()]) => {
                    r.map_err(|e| anyhow::anyhow!(e))
                }
                r = spool.serve(SHUTDOWN_BROADCAST.subscribe()) => {
                    r.map_err(|e| anyhow::anyhow!(e))
                }
                r = delivery_arc.serve(SHUTDOWN_BROADCAST.subscribe()) => {
                    r.map_err(|e| anyhow::anyhow!(e))
                }
                r = control_server.serve(SHUTDOWN_BROADCAST.subscribe()) => {
                    r.map_err(|e| anyhow::anyhow!("Control server error: {e}"))
                }
                r = health_server.serve(SHUTDOWN_BROADCAST.subscribe()) => {
                    r.map_err(|e| anyhow::anyhow!("Health server error: {e}"))
                }
                r = shutdown() => {
                    r
                }
            }
        } else {
            tokio::select! {
                r = self.smtp_controller.control(vec![SHUTDOWN_BROADCAST.subscribe()]) => {
                    r.map_err(|e| anyhow::anyhow!(e))
                }
                r = spool.serve(SHUTDOWN_BROADCAST.subscribe()) => {
                    r.map_err(|e| anyhow::anyhow!(e))
                }
                r = delivery_arc.serve(SHUTDOWN_BROADCAST.subscribe()) => {
                    r.map_err(|e| anyhow::anyhow!(e))
                }
                r = control_server.serve(SHUTDOWN_BROADCAST.subscribe()) => {
                    r.map_err(|e| anyhow::anyhow!("Control server error: {e}"))
                }
                r = shutdown() => {
                    r
                }
            }
        };

        internal!("Shutting down...");

        ret
    }
}
