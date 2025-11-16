use std::str::FromStr;

use opentelemetry::trace::TracerProvider as _;
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::{
    trace::{RandomIdGenerator, SdkTracerProvider},
    Resource,
};
use tracing::metadata::LevelFilter;
use tracing_subscriber::{
    Layer, filter::FilterFn, prelude::__tracing_subscriber_SubscriberExt, util::SubscriberInitExt,
};

#[macro_export]
macro_rules! log {
    ($level:expr, $span:expr, $($msg:expr),*) => {{
        let span = $crate::tracing::span!($level, $span);
        let _enter = span.enter();

        $crate::tracing::event!($level, $($msg),*)
    }};
}

#[macro_export]
macro_rules! outgoing {
    (level = $level:ident, $($msg:expr),*) => {
        $crate::log!($crate::tracing::Level::$level, "outgoing", $($msg),*)
    };

    ($($msg:expr),*) => {
        $crate::outgoing!(level = TRACE, $($msg),*)
    };
}

#[macro_export]
macro_rules! incoming {
    (level = $level:ident, $($msg:expr),*) => {
        $crate::log!($crate::tracing::Level::$level, "incoming", $($msg),*)
    };

    ($($msg:expr),*) => {
        $crate::incoming!(level = TRACE, $($msg),*)
    };
}

#[macro_export]
macro_rules! internal {
    (level = $level:ident, $($msg:expr),*) => {
        $crate::log!($crate::tracing::Level::$level, "internal", $($msg),*)
    };

    ($($msg:expr),*) => {
        $crate::internal!(level = TRACE, $($msg),*)
    };
}

/// Initialize the global tracing subscriber with JSON structured logging and OpenTelemetry
///
/// This configures:
/// - JSON formatted logs for machine parsing and LogQL queries
/// - OpenTelemetry trace context (trace_id, span_id) injected into all log entries
/// - OTLP trace export to Jaeger via OpenTelemetry Collector
/// - Environment-based log level filtering (LOG_LEVEL or RUST_LOG)
/// - File and line number information for debugging
/// - Current span context included in log entries
///
/// # Example JSON Output with Trace Context
///
/// ```json
/// {
///   "timestamp": "2025-11-16T10:30:45.123456789Z",
///   "level": "INFO",
///   "target": "empath_delivery",
///   "fields": {
///     "message": "Delivery successful",
///     "message_id": "01JCXYZ...",
///     "domain": "example.com",
///     "delivery_attempt": 1
///   },
///   "span": {
///     "name": "deliver_message",
///     "trace_id": "a1b2c3d4e5f6g7h8...",
///     "span_id": "9i0j1k2l..."
///   },
///   "spans": [
///     {"name": "process_queue", "trace_id": "a1b2c3d4...", "span_id": "3m4n5o6p..."},
///     {"name": "deliver_message", "trace_id": "a1b2c3d4...", "span_id": "9i0j1k2l..."}
///   ],
///   "file": "empath-delivery/src/lib.rs",
///   "line": 456
/// }
/// ```
///
/// # LogQL Query Examples with Trace Correlation
///
/// ```logql
/// # Find all logs for a specific trace
/// {service="empath"} | json | span.trace_id="a1b2c3d4e5f6g7h8..."
///
/// # Find all logs for a specific message
/// {service="empath"} | json | message_id="01JCXYZ..."
///
/// # Find delivery failures and their trace IDs
/// {service="empath"} | json | level="ERROR" | line_format "{{.span.trace_id}}: {{.domain}}: {{.message}}"
///
/// # Track delivery attempts by domain
/// sum by (domain) (count_over_time({service="empath"} | json | delivery_attempt > 0 [1h]))
/// ```
pub fn init() {
    let default = if cfg!(debug_assertions) {
        LevelFilter::TRACE
    } else {
        LevelFilter::INFO
    };

    // Support both LOG_LEVEL (legacy) and RUST_LOG (standard)
    let level = std::env::var("RUST_LOG")
        .or_else(|_| std::env::var("LOG_LEVEL"))
        .map_or(default, |level| {
            LevelFilter::from_str(level.as_str()).unwrap_or_else(|_| {
                eprintln!("Invalid log level specified '{level}', defaulting to {default}");
                default
            })
        });

    // Set up OpenTelemetry tracer with OTLP exporter to send traces to Jaeger
    let otlp_endpoint = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT")
        .unwrap_or_else(|_| "http://localhost:4318".to_string());

    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .with_endpoint(otlp_endpoint)
        .build()
        .expect("Failed to build OTLP exporter");

    let resource = Resource::builder_empty()
        .with_service_name("empath-mta")
        .with_attributes([
            opentelemetry::KeyValue::new("service.version", env!("CARGO_PKG_VERSION")),
        ])
        .build();

    // Create batch processor for the exporter
    let batch_processor = opentelemetry_sdk::trace::BatchSpanProcessor::builder(exporter)
        .with_batch_config(
            opentelemetry_sdk::trace::BatchConfigBuilder::default()
                .with_max_queue_size(2048)
                .build(),
        )
        .build();

    let tracer_provider = SdkTracerProvider::builder()
        .with_span_processor(batch_processor)
        .with_resource(resource)
        .with_id_generator(RandomIdGenerator::default())
        .build();

    // Set as global provider and get tracer
    opentelemetry::global::set_tracer_provider(tracer_provider.clone());
    let tracer = tracer_provider.tracer("empath");

    tracing_subscriber::Registry::default()
        .with(
            // OpenTelemetry layer for trace context (trace_id, span_id)
            tracing_opentelemetry::layer().with_tracer(tracer),
        )
        .with(
            tracing_subscriber::fmt::layer()
                .with_file(true) // Include file for debugging
                .with_line_number(true) // Include line number for debugging
                .json() // JSON format for structured logging and LogQL queries
                .with_current_span(true) // Include current span fields
                .with_span_list(true) // Include span list for context
                .with_timer(tracing_subscriber::fmt::time::ChronoUtc::rfc_3339())
                .with_filter(level)
                .with_filter(FilterFn::new(|metadata| {
                    metadata.target().starts_with("empath")
                })),
        )
        .init();
}
