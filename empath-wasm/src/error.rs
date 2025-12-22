//! Error types for WebAssembly module operations

use thiserror::Error;

/// Result type for WASM operations
pub type Result<T> = std::result::Result<T, WasmError>;

/// Errors that can occur during WASM module operations
#[derive(Debug, Error)]
pub enum WasmError {
    /// WASM module file not found
    #[error("WASM module file not found: {0}")]
    ModuleNotFound(String),

    /// Wasmtime engine error
    #[error("Wasmtime error: {0}")]
    Wasmtime(#[from] wasmtime::Error),

    /// Callback not found in module
    #[error("Callback '{0}' not found in module")]
    CallbackNotFound(String),

    /// Module initialization failed
    #[error("Module initialization failed: {0}")]
    InitFailed(String),

    /// Invalid return value from callback
    #[error("Invalid return value from callback '{0}': {1}")]
    InvalidReturn(String, String),

    /// Serialization/deserialization error
    #[error("Protocol error: {0}")]
    Protocol(#[from] postcard::Error),

    /// I/O error
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Resource limit exceeded
    #[error("Resource limit exceeded: {0}")]
    ResourceLimit(String),

    /// Execution timeout
    #[error("Callback '{0}' timed out after {1}s")]
    Timeout(String, u64),

    /// WASI context build error
    #[error("WASI build error: {0}")]
    WasiBuild(String),

    /// Engine creation failed
    #[error("Failed to create Wasmtime engine: {0}")]
    EngineCreation(String),

    /// Internal error (mutex poisoning, Arc unwrap failure, etc.)
    #[error("Internal error: {0}")]
    Internal(String),
}
