#![feature(
    ascii_char,
    associated_type_defaults,
    iter_advance_by,
    result_option_map_or_default,
    slice_pattern,
    vec_into_raw_parts
)]

pub mod address;
pub mod context;
pub mod controller;
pub mod domain;
pub mod envelope;
pub mod error;
pub mod listener;
pub mod logging;
pub mod message;
pub mod mime;
pub mod status;
pub mod traits;

pub use context::{DeliveryAttempt, DeliveryContext, DeliveryStatus};
pub use domain::Domain;
pub use tracing;

#[derive(Debug, Clone, Copy)]
pub enum Signal {
    Shutdown,
    Finalised,
}
