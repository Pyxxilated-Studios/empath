//! Error types for metrics operations

use thiserror::Error;

/// Errors that can occur during metrics operations
#[derive(Debug, Error)]
pub enum MetricsError {
    /// Metrics system has already been initialized
    #[error("Metrics system already initialized")]
    AlreadyInitialized,

    /// OpenTelemetry SDK error
    #[error("OpenTelemetry error: {0}")]
    OpenTelemetry(String),

    /// HTTP server error
    #[error("HTTP server error: {0}")]
    HttpServer(#[from] std::io::Error),

    /// Prometheus export error
    #[error("Prometheus export error: {0}")]
    PrometheusExport(String),
}
