#![feature(associated_type_defaults, slice_pattern, vec_into_raw_parts)]

pub mod context;
pub mod controller;
pub mod envelope;
pub mod listener;
pub mod logging;
pub mod status;
pub mod traits;

pub use tracing;

#[derive(Debug, Clone, Copy)]
pub enum Signal {
    Shutdown,
    Finalised,
}
