//! Error types for WebAssembly module operations

use empath_common::ModuleError;

/// Result type for WASM operations
pub type Result<T> = std::result::Result<T, ModuleError>;

/// Convert Wasmtime errors to `ModuleError`
pub fn from_wasmtime(err: &wasmtime::Error) -> ModuleError {
    ModuleError::Runtime(err.to_string())
}

/// Convert postcard (serialization) errors to `ModuleError`
pub fn from_postcard(err: &postcard::Error) -> ModuleError {
    ModuleError::Protocol(err.to_string())
}
