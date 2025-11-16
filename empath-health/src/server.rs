//! Health check HTTP server

use crate::{HealthChecker, HealthConfig, HealthError};
use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use empath_common::Signal;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tower_http::timeout::TimeoutLayer;

/// Health check HTTP server
///
/// Provides `/health/live` and `/health/ready` endpoints for Kubernetes probes.
pub struct HealthServer {
    listener: TcpListener,
    router: Router,
}

impl HealthServer {
    /// Create a new health server
    ///
    /// # Errors
    ///
    /// Returns an error if binding to the specified address fails.
    pub async fn new(
        config: HealthConfig,
        health_checker: Arc<HealthChecker>,
    ) -> Result<Self, HealthError> {
        let listener = TcpListener::bind(&config.listen_address)
            .await
            .map_err(|e| HealthError::BindError {
                address: config.listen_address.clone(),
                source: e,
            })?;

        tracing::info!(
            address = %config.listen_address,
            "Health check server bound successfully"
        );

        // Create router with health endpoints
        let router = Router::new()
            .route("/health/live", get(liveness_handler))
            .route("/health/ready", get(readiness_handler))
            .with_state(health_checker)
            // Add timeout layer to ensure probes respond within 1 second
            .layer(TimeoutLayer::new(Duration::from_secs(1)));

        Ok(Self { listener, router })
    }

    /// Run the health server until shutdown signal is received
    ///
    /// # Errors
    ///
    /// Returns an error if the server encounters a runtime error.
    pub async fn serve(
        self,
        mut shutdown: tokio::sync::broadcast::Receiver<Signal>,
    ) -> Result<(), HealthError> {
        tracing::info!("Health check server starting");

        axum::serve(self.listener, self.router)
            .with_graceful_shutdown(async move {
                let _ = shutdown.recv().await;
                tracing::info!("Health check server received shutdown signal");
            })
            .await
            .map_err(|e| HealthError::ServerError(e.to_string()))?;

        tracing::info!("Health check server stopped");
        Ok(())
    }
}

/// Liveness probe handler
///
/// Returns 200 OK if the application is alive (can respond to requests).
/// Kubernetes will restart the container if this probe fails.
async fn liveness_handler(
    State(health_checker): State<Arc<HealthChecker>>,
) -> Response {
    if health_checker.is_alive() {
        (StatusCode::OK, "OK").into_response()
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, "Service Unavailable").into_response()
    }
}

/// Readiness probe handler
///
/// Returns 200 OK if the application is ready to accept traffic.
/// Kubernetes will remove the pod from service endpoints if this probe fails.
async fn readiness_handler(
    State(health_checker): State<Arc<HealthChecker>>,
) -> Response {
    if health_checker.is_ready() {
        (StatusCode::OK, "OK").into_response()
    } else {
        let status = health_checker.get_status();
        tracing::warn!(
            smtp_ready = status.smtp_ready,
            spool_ready = status.spool_ready,
            delivery_ready = status.delivery_ready,
            dns_ready = status.dns_ready,
            queue_size = status.queue_size,
            max_queue_size = status.max_queue_size,
            "Readiness probe failed"
        );
        (StatusCode::SERVICE_UNAVAILABLE, Json(status)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_liveness_probe_always_passes() {
        let checker = Arc::new(HealthChecker::new(10000));
        let response = liveness_handler(State(checker)).await;
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_readiness_probe_fails_when_not_ready() {
        let checker = Arc::new(HealthChecker::new(10000));
        // Don't set any components as ready
        let response = readiness_handler(State(checker)).await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn test_readiness_probe_passes_when_all_ready() {
        let checker = Arc::new(HealthChecker::new(10000));
        checker.set_smtp_ready(true);
        checker.set_spool_ready(true);
        checker.set_delivery_ready(true);
        checker.set_dns_ready(true);
        checker.set_queue_size(100);

        let response = readiness_handler(State(checker)).await;
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_readiness_probe_fails_when_queue_too_large() {
        let checker = Arc::new(HealthChecker::new(1000));
        checker.set_smtp_ready(true);
        checker.set_spool_ready(true);
        checker.set_delivery_ready(true);
        checker.set_dns_ready(true);
        checker.set_queue_size(2000); // Exceeds max

        let response = readiness_handler(State(checker)).await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }
}
