use chrono::Utc;
use tracing::metadata::LevelFilter;
use tracing_subscriber::{
    filter::FilterFn, fmt::time::FormatTime, prelude::__tracing_subscriber_SubscriberExt,
    util::SubscriberInitExt, Layer,
};

struct Time;

impl FormatTime for Time {
    fn format_time(&self, w: &mut tracing_subscriber::fmt::format::Writer<'_>) -> std::fmt::Result {
        let time = Utc::now();
        w.write_fmt(format_args!("{:?}", time.timestamp_micros()))
    }
}

#[macro_export]
macro_rules! log {
    ($level:expr, $span:expr, $($msg:expr),*) => {{
        let span = $crate::tracing::span!(target: "empath", $level, $span);
        let _enter = span.enter();

        $crate::tracing::event!(target: "empath", $level, $($msg),*)
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

pub fn init() {
    let level = if let Ok(level) = std::env::var("LOG_LEVEL") {
        match level.to_ascii_lowercase().as_str() {
            "warn" => LevelFilter::WARN,
            "info" => LevelFilter::INFO,
            "trace" => LevelFilter::TRACE,
            _ => LevelFilter::ERROR,
        }
    } else if cfg!(debug_assertions) {
        LevelFilter::TRACE
    } else {
        LevelFilter::INFO
    };

    tracing_subscriber::Registry::default()
        .with(
            (if cfg!(debug_assertions) {
                tracing_subscriber::fmt::layer()
            } else {
                tracing_subscriber::fmt::layer()
                    .with_file(false)
                    .with_line_number(false)
            })
            .compact()
            .with_ansi(true)
            .with_timer(Time)
            .with_target(false)
            .with_level(false)
            .with_filter(level)
            .with_filter(FilterFn::new(|metadata| {
                metadata.target().starts_with("empath")
            })),
        )
        .init();
}
