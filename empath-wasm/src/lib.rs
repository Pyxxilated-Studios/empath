//! WebAssembly module support for Empath
//!
//! This crate enables writing Empath validation modules in any language that compiles
//! to WebAssembly (Rust, C, Go, `AssemblyScript`, etc.). It provides:
//! - Fast, near-native performance (~1-2ms overhead)
//! - True sandboxing (no host memory access)
//! - Strict resource limits (memory, CPU)
//! - Multi-language support via WASM compilation
//! - Integration with the existing module dispatch system

mod bindings;
mod error;
mod host;
mod loader;

// Re-export generated bindings
pub use bindings::*;
pub use error::{Result, WasmError};
pub use host::{SerializableContext, WasmStore};
pub use loader::WasmModule;
