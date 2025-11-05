pub mod controller;

// Import tracing items for macro expansion
use empath_common::tracing::{event, span, Instrument, Level};

// Create a tracing alias so macros can find tracing:: paths
extern crate self as tracing;
