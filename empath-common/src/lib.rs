#![feature(associated_type_defaults, slice_pattern)]

pub mod address;
pub mod address_parser;
pub mod audit;
pub mod config;
pub mod context;
pub mod controller;
pub mod domain;
pub mod email;
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
