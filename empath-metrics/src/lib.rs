//! OpenTelemetry metrics and tracing for Empath MTA
//!
//! This crate provides comprehensive observability instrumentation using OpenTelemetry.
//! It exports metrics via OTLP to an OpenTelemetry Collector, which can expose them
//! in Prometheus format for scraping.
//!
//! # Features
//!
//! - **SMTP Metrics**: Connection counts, command errors, session durations
//! - **Delivery Metrics**: Attempt counts, success/failure rates, delivery durations
//! - **Queue Metrics**: Queue size by status, processing latency
//! - **DNS Metrics**: Lookup durations, cache hit rates
//! - **OTLP Export**: Push metrics to OpenTelemetry Collector
//!
//! # Architecture
//!
//! ```text
//! Empath MTA → OTLP/HTTP → OpenTelemetry Collector → Prometheus (scrape) → Grafana
//! ```
//!
//! # Usage
//!
//! ```rust,no_run
//! use empath_metrics::{init_metrics, MetricsConfig};
//!
//! # fn example() -> Result<(), Box<dyn std::error::Error>> {
//! // Initialize metrics system
//! let config = MetricsConfig {
//!     enabled: true,
//!     endpoint: "http://localhost:4318".to_string(),
//!     max_domain_cardinality: 1000,
//!     high_priority_domains: vec!["gmail.com".to_string(), "outlook.com".to_string()],
//! };
//!
//! init_metrics(&config)?;
//!
//! // Metrics are now pushed to the OpenTelemetry Collector
//! # Ok(())
//! # }
//! ```

mod config;
mod delivery;
mod dns;
mod error;
mod exporter;
mod smtp;

pub use config::MetricsConfig;
pub use delivery::DeliveryMetrics;
pub use dns::DnsMetrics;
pub use error::MetricsError;
use once_cell::sync::OnceCell;
pub use smtp::SmtpMetrics;

/// Global metrics instance
static METRICS_INSTANCE: OnceCell<Metrics> = OnceCell::new();

/// Root metrics container
#[derive(Debug)]
pub struct Metrics {
    pub smtp: SmtpMetrics,
    pub delivery: DeliveryMetrics,
    pub dns: DnsMetrics,
}

/// Initialize the metrics system
///
/// This must be called once at startup before any metrics are recorded.
/// If metrics are disabled in the config, this is a no-op.
///
/// Metrics will be pushed to the configured OTLP endpoint (typically an
/// OpenTelemetry Collector) which can then expose them for Prometheus to scrape.
///
/// # Errors
///
/// Returns an error if metrics initialization fails or if called multiple times.
///
/// # Example
///
/// ```rust,no_run
/// use empath_metrics::{init_metrics, MetricsConfig};
///
/// # fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let config = MetricsConfig {
///     enabled: true,
///     endpoint: "http://localhost:4318".to_string(),
///     max_domain_cardinality: 1000,
///     high_priority_domains: vec!["gmail.com".to_string(), "outlook.com".to_string()],
/// };
///
/// init_metrics(&config)?;
/// # Ok(())
/// # }
/// ```
pub fn init_metrics(config: &MetricsConfig) -> Result<(), MetricsError> {
    if !config.enabled {
        tracing::info!("Metrics collection is disabled");
        return Ok(());
    }

    tracing::info!(
        endpoint = %config.endpoint,
        "Initializing OpenTelemetry metrics with OTLP exporter"
    );

    // Initialize the OTLP exporter
    let provider = exporter::init_otlp_exporter(&config.endpoint)?;

    // Install the provider as the global meter provider
    opentelemetry::global::set_meter_provider(provider);

    // Create metric instruments
    let smtp = SmtpMetrics::new()?;
    let delivery = DeliveryMetrics::new(
        config.max_domain_cardinality,
        config.high_priority_domains.clone(),
    )?;
    let dns = DnsMetrics::new()?;

    let metrics = Metrics {
        smtp,
        delivery,
        dns,
    };

    // Store the global metrics instance
    METRICS_INSTANCE
        .set(metrics)
        .map_err(|_| MetricsError::AlreadyInitialized)?;

    tracing::info!("Metrics collection initialized successfully");

    Ok(())
}

/// Get a reference to the global metrics instance
///
/// # Panics
///
/// Panics if metrics have not been initialized via `init_metrics()`.
#[must_use]
pub fn metrics() -> &'static Metrics {
    METRICS_INSTANCE
        .get()
        .expect("Metrics not initialized. Call init_metrics() first.")
}

/// Check if metrics are enabled
#[must_use]
pub fn is_enabled() -> bool {
    METRICS_INSTANCE.get().is_some()
}
