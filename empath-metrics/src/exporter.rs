//! OTLP metrics exporter

use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::metrics::SdkMeterProvider;

use crate::MetricsError;

/// Initialize the OTLP metrics exporter
///
/// This configures the OpenTelemetry SDK to push metrics to an OTLP endpoint
/// (typically an OpenTelemetry Collector) which can then expose them for Prometheus to scrape.
///
/// # Errors
///
/// Returns an error if the OTLP exporter cannot be initialized.
pub fn init_otlp_exporter(endpoint: &str) -> Result<SdkMeterProvider, MetricsError> {
    tracing::info!(endpoint = %endpoint, "Configuring OTLP metrics exporter");

    let exporter = opentelemetry_otlp::MetricExporter::builder()
        .with_http()
        .with_endpoint(endpoint)
        .build()
        .map_err(|e| {
            tracing::error!(endpoint = %endpoint, error = %e, "Failed to build OTLP exporter");
            MetricsError::OpenTelemetry(e.to_string())
        })?;

    let reader = opentelemetry_sdk::metrics::PeriodicReader::builder(exporter).build();

    let provider = SdkMeterProvider::builder().with_reader(reader).build();

    tracing::info!("OTLP metrics exporter initialized successfully");
    Ok(provider)
}
