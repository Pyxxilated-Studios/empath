//! Generated bindings from WIT interface definitions
//!
//! This module contains auto-generated Rust types and traits from the
//! WebAssembly Interface Types (WIT) definitions in `wit/empath.wit`.

use wasmtime::component::bindgen;

// Generate Rust bindings from WIT files
bindgen!({
    world: "validator",
    path: "wit",
});
