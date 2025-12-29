use std::str::FromStr;

use opentelemetry::trace::TracerProvider as _;
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::trace::{RandomIdGenerator, SdkTracerProvider};
use tracing::metadata::LevelFilter;
use tracing_subscriber::filter::{EnvFilter, LevelFilter as SubscriberLevelFilter};
use tracing_subscriber::prelude::__tracing_subscriber_SubscriberExt;
use tracing_subscriber::reload;
use tracing_subscriber::util::SubscriberInitExt;

pub struct LoggingHandle {
    filter_handle: reload::Handle<EnvFilter, tracing_subscriber::Registry>,
}

impl LoggingHandle {
    /// Update the runtime log level for all Empath targets.
    ///
    /// # Errors
    ///
    /// Returns an error if the reload handle cannot be updated.
    pub fn set_log_level(&self, level: LevelFilter) -> Result<(), String> {
        let subscriber_level = level.into_level().unwrap_or(tracing::Level::TRACE);

        self.filter_handle
            .modify(|filter| {
                *filter = EnvFilter::builder()
                    .with_default_directive(
                        SubscriberLevelFilter::from_level(subscriber_level).into(),
                    )
                    .parse_lossy(format!(
                        "empath={level},empath_common={level},empath_delivery={level},empath_smtp={level}"
                    ));
            })
            .map_err(|e| format!("failed to update log level: {e}"))
    }
}

pub struct LoggingConfigBuilder {
    otlp_endpoint: Option<String>,
    default_level: Option<LevelFilter>,
}

impl Default for LoggingConfigBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl LoggingConfigBuilder {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            otlp_endpoint: None,
            default_level: None,
        }
    }

    #[must_use]
    pub fn with_otlp_endpoint(mut self, otlp_endpoint: impl Into<String>) -> Self {
        self.otlp_endpoint = Some(otlp_endpoint.into());
        self
    }

    #[must_use]
    pub const fn with_default_level(mut self, level: LevelFilter) -> Self {
        self.default_level = Some(level);
        self
    }

    /// Initialize the global tracing subscriber with JSON structured logging and OpenTelemetry
    ///
    /// This configures:
    /// - JSON formatted logs for machine parsing and `LogQL` queries
    /// - OpenTelemetry trace context (`trace_id`, `span_id`) injected into all log entries
    /// - OTLP trace export to Jaeger via OpenTelemetry Collector
    /// - Environment-based log level filtering (`LOG_LEVEL` or `RUST_LOG`)
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
    /// # `LogQL` Query Examples with Trace Correlation
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
    ///
    /// # Errors
    ///
    /// Returns an error if the OTLP exporter cannot be configured.
    pub fn init(self) -> anyhow::Result<LoggingHandle> {
        let default_level = if cfg!(debug_assertions) {
            LevelFilter::TRACE
        } else {
            LevelFilter::INFO
        };
        let default = self.default_level.unwrap_or(default_level);

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
        // Priority: 1) Provided parameter, 2) Environment variable, 3) Default
        let otlp_base_endpoint = self
            .otlp_endpoint
            .or_else(|| std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").ok())
            .unwrap_or_else(|| "http://localhost:4318".to_string());

        // OTLP HTTP exporter requires full path for traces endpoint
        let traces_endpoint = if otlp_base_endpoint.ends_with("/v1/traces") {
            otlp_base_endpoint
        } else if otlp_base_endpoint.ends_with("/v1/metrics") {
            // Replace /v1/metrics with /v1/traces (reuse same OTLP collector)
            otlp_base_endpoint.replace("/v1/metrics", "/v1/traces")
        } else {
            format!("{}/v1/traces", otlp_base_endpoint.trim_end_matches('/'))
        };

        let exporter = opentelemetry_otlp::SpanExporter::builder()
            .with_http()
            .with_endpoint(traces_endpoint)
            .build()?;

        let resource = Resource::builder_empty()
            .with_service_name("empath-mta")
            .with_attributes([opentelemetry::KeyValue::new(
                "service.version",
                env!("CARGO_PKG_VERSION"),
            )])
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

        let subscriber_level = level.into_level().unwrap_or(tracing::Level::TRACE);

        let filter = EnvFilter::builder()
            .with_default_directive(SubscriberLevelFilter::from_level(subscriber_level).into())
            .parse_lossy(format!(
                "empath={level},empath_common={level},empath_delivery={level},empath_smtp={level}"
            ));

        let (filter_layer, handle) = reload::Layer::new(filter);

        let fmt_layer = tracing_subscriber::fmt::layer()
            .with_file(true) // Include file for debugging
            .with_line_number(true) // Include line number for debugging
            .json() // JSON format for structured logging and LogQL queries
            .with_current_span(true) // Include current span fields
            .with_span_list(true) // Include span list for context
            .with_timer(tracing_subscriber::fmt::time::ChronoUtc::rfc_3339());

        tracing_subscriber::Registry::default()
            .with(filter_layer)
            .with(tracing_opentelemetry::layer().with_tracer(tracer))
            .with(fmt_layer)
            .init();

        Ok(LoggingHandle {
            filter_handle: handle,
        })
    }
}

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
