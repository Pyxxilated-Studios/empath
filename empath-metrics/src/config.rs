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

    /// Maximum number of unique domains to track in metrics
    ///
    /// High-cardinality labels (like domain names) can create thousands of metric series
    /// which impacts Prometheus memory and query performance. This limit caps the number
    /// of unique domains that will be tracked individually.
    ///
    /// Once the limit is reached, additional domains are bucketed into an "other" category.
    /// This prevents metric explosion while still tracking the most common domains.
    ///
    /// Recommended values:
    /// - Small deployments (< 100 domains): 100
    /// - Medium deployments (100-1000 domains): 500
    /// - Large deployments (1000+ domains): 1000
    ///
    /// Default: 1000
    #[serde(default = "default_max_domain_cardinality")]
    pub max_domain_cardinality: usize,

    /// Domains that should always be tracked individually
    ///
    /// These domains bypass the cardinality limit and are always tracked with their
    /// full domain name. Useful for prioritizing metrics for your own domains or
    /// major email providers.
    ///
    /// Example:
    /// ```ron
    /// high_priority_domains: [
    ///     "gmail.com",
    ///     "outlook.com",
    ///     "company.com",
    /// ]
    /// ```
    ///
    /// Default: empty list
    #[serde(default)]
    pub high_priority_domains: Vec<String>,

    /// Optional API key for authenticating with the OTLP collector
    ///
    /// When set, this API key will be sent in the `Authorization: Bearer <key>` header
    /// with all OTLP metric exports. The collector must be configured to validate this key.
    ///
    /// **Security Note:** This stores the API key in plaintext in the configuration file.
    /// For better security, consider using environment variable substitution in your
    /// configuration management system, or mounting secrets in Kubernetes.
    ///
    /// Example:
    /// ```ron
    /// metrics: (
    ///     enabled: true,
    ///     endpoint: "http://otel-collector:4318/v1/metrics",
    ///     api_key: "your-secret-api-key-here",
    /// )
    /// ```
    ///
    /// Default: None (no authentication)
    #[serde(default)]
    pub api_key: Option<String>,
}

const fn default_enabled() -> bool {
    true
}

fn default_endpoint() -> String {
    "http://localhost:4318/v1/metrics".to_string()
}

const fn default_max_domain_cardinality() -> usize {
    1000
}

impl Default for MetricsConfig {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            endpoint: default_endpoint(),
            max_domain_cardinality: default_max_domain_cardinality(),
            high_priority_domains: Vec::new(),
            api_key: None,
        }
    }
}
