//! Helper APIs for building Empath WASM validation modules.
//!
//! The `bindings!` proc macro expands to the canonical WIT bindings for the
//! Empath validator world and layers ergonomic helpers on top (context access,
//! accept/reject shorthands, and safe wrappers around host/message functions).

extern crate alloc;

pub use empath_wasm_guest_macros::bindings;
pub use wit_bindgen;

pub mod context;
pub mod message_helpers;
pub mod prelude;
pub mod result;

/// Canonical WIT definition used by the guest bindings.
pub const EMPATH_WIT: &str = include_str!("../wit/empath.wit");
