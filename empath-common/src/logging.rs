use std::str::FromStr;

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

/// Initialize the global tracing subscriber with JSON structured logging
///
/// This configures:
/// - JSON formatted logs for machine parsing and LogQL queries
/// - Environment-based log level filtering (LOG_LEVEL or RUST_LOG)
/// - File and line number information for debugging
/// - Current span context included in log entries
///
/// **Note**: OpenTelemetry trace context (trace_id, span_id) will be added in tasks 0.35+0.36
/// (Distributed Tracing Pipeline). For now, span names and fields are included but not
/// globally unique trace IDs.
///
/// # Example JSON Output
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
///     "name": "deliver_message"
///   },
///   "spans": [
///     {"name": "process_queue"},
///     {"name": "deliver_message"}
///   ],
///   "file": "empath-delivery/src/lib.rs",
///   "line": 456
/// }
/// ```
///
/// # LogQL Query Examples
///
/// ```logql
/// # Find all logs for a specific message
/// {service="empath"} | json | message_id="01JCXYZ..."
///
/// # Find delivery failures
/// {service="empath"} | json | level="ERROR" | line_format "{{.domain}}: {{.message}}"
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

    tracing_subscriber::Registry::default()
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
