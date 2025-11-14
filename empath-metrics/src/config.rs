//! Metrics configuration

use serde::Deserialize;

/// Configuration for metrics collection and export
#[derive(Debug, Clone, Deserialize)]
pub struct MetricsConfig {
    /// Enable or disable metrics collection
    ///
    /// When disabled, all metrics operations become no-ops with minimal overhead.
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    /// OTLP endpoint URL for metrics export
    ///
    /// Metrics will be pushed to this OpenTelemetry Collector endpoint using OTLP over HTTP.
    /// The Collector can then expose metrics for Prometheus to scrape.
    ///
    /// Common values:
    /// - `http://localhost:4318` (OTLP HTTP default for local development)
    /// - `http://otel-collector:4318` (Docker Compose service name)
    /// - `http://otel-collector.monitoring.svc.cluster.local:4318` (Kubernetes)
    #[serde(default = "default_endpoint")]
    pub endpoint: String,
}

const fn default_enabled() -> bool {
    true
}

fn default_endpoint() -> String {
    "http://localhost:4318/v1/metrics".to_string()
}

impl Default for MetricsConfig {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            endpoint: default_endpoint(),
        }
    }
}
