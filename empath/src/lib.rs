#![deny(clippy::pedantic, clippy::all, clippy::nursery)]
#![allow(clippy::must_use_candidate)]

pub mod controller;

// Import tracing items for macro expansion
use empath_common::tracing::{Instrument, Level, event, span};

// Create a tracing alias so macros can find tracing:: paths
extern crate self as tracing;
