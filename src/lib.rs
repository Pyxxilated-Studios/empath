#![feature(
    vec_into_raw_parts,
    lazy_cell,
    lint_reasons,
    result_option_inspect,
    slice_pattern
)]

pub mod controller;
mod ffi;
mod listener;
mod logging;
mod server;
mod smtp;
mod traits;

pub use tracing;
