//! OTLP metrics exporter

use std::collections::HashMap;

use opentelemetry_otlp::{WithExportConfig, WithHttpConfig};
use opentelemetry_sdk::metrics::SdkMeterProvider;

use crate::{MetricsConfig, MetricsError};

/// Initialize the OTLP metrics exporter
///
/// This configures the OpenTelemetry SDK to push metrics to an OTLP endpoint
/// (typically an OpenTelemetry Collector) which can then expose them for Prometheus to scrape.
///
/// If an API key is configured, it will be sent in the `Authorization: Bearer <key>` header
/// with all OTLP requests.
///
/// # Errors
///
/// Returns an error if the OTLP exporter cannot be initialized.
pub fn init_otlp_exporter(config: &MetricsConfig) -> Result<SdkMeterProvider, MetricsError> {
    tracing::info!(endpoint = %config.endpoint, "Configuring OTLP metrics exporter");

    let mut builder = opentelemetry_otlp::MetricExporter::builder()
        .with_http()
        .with_endpoint(&config.endpoint);

    // Add Authorization header if API key is configured
    if let Some(api_key) = &config.api_key {
        tracing::info!("Metrics API key authentication enabled");
        let mut headers = HashMap::new();
        headers.insert(
            "authorization".to_string(),
            format!("Bearer {api_key}"),
        );
        builder = builder.with_headers(headers);
    } else {
        tracing::info!("Metrics API key authentication disabled");
    }

    let exporter = builder.build().map_err(|e| {
        tracing::error!(endpoint = %config.endpoint, error = %e, "Failed to build OTLP exporter");
        MetricsError::OpenTelemetry(e.to_string())
    })?;

    let reader = opentelemetry_sdk::metrics::PeriodicReader::builder(exporter).build();

    let provider = SdkMeterProvider::builder().with_reader(reader).build();

    tracing::info!("OTLP metrics exporter initialized successfully");
    Ok(provider)
}
