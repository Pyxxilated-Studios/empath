pub mod control_handler;
pub mod controller;

// Import tracing items for macro expansion
use empath_common::tracing::{Instrument, Level, event, span};

// Create a tracing alias so macros can find tracing:: paths
extern crate self as tracing;
