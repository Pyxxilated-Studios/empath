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

pub fn init() {
    let default = if cfg!(debug_assertions) {
        LevelFilter::TRACE
    } else {
        LevelFilter::INFO
    };

    let level = std::env::var("LOG_LEVEL").map_or(default, |level| {
        LevelFilter::from_str(level.as_str()).unwrap_or_else(|_| {
            eprintln!("Invalid log level specified {level}, defaulting to {default}");
            default
        })
    });

    tracing_subscriber::Registry::default()
        .with(
            tracing_subscriber::fmt::layer()
                .with_file(false)
                .with_line_number(false)
                .compact()
                .with_ansi(true)
                .with_timer(tracing_subscriber::fmt::time::ChronoUtc::rfc_3339())
                .with_filter(level)
                .with_filter(FilterFn::new(|metadata| {
                    metadata.target().starts_with("empath")
                })),
        )
        .init();
}
