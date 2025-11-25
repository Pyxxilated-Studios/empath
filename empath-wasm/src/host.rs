//! Host import functions for WASM modules
//!
//! This module provides host functions that WASM modules can call to interact with
//! the Empath runtime. These functions enable WASM modules to:
//! - Access Context data (sender, recipients, message content)
//! - Allocate and free memory
//! - Log messages
//! - Set response codes

use empath_common::{context::Context, internal};
use serde::{Deserialize, Serialize};
use wasmtime::{Caller, ResourceLimiter};

use crate::error::{Result, WasmError};

/// Serializable Context representation for passing to WASM
///
/// This is a simplified view of Context that only includes fields
/// needed by WASM modules. The session field is intentionally excluded
/// as it's ephemeral and not persisted.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SerializableContext {
    pub extended: bool,
    pub id: String,
    pub sender: Option<String>,
    pub recipients: Vec<String>,
    pub data: Option<Vec<u8>>,
    pub max_message_size: usize,
    pub transaction: Vec<(String, String)>,
    pub tracking_id: Option<String>,
}

impl From<&Context> for SerializableContext {
    fn from(ctx: &Context) -> Self {
        Self {
            extended: ctx.extended,
            id: ctx.id.clone(),
            sender: ctx.envelope.sender().map(ToString::to_string),
            recipients: ctx.envelope.recipients().map_or_else(Vec::new, |addrs| {
                addrs.iter().map(ToString::to_string).collect()
            }),
            data: ctx.data.as_ref().map(|d| d.to_vec()),
            max_message_size: ctx.max_message_size,
            transaction: ctx
                .transaction
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect(),
            tracking_id: ctx.tracking_id.clone(),
        }
    }
}

/// Store data for WASM instance
///
/// This holds the serialized Context and other runtime state that
/// WASM modules can access via host functions.
pub struct WasmStore {
    /// Serialized context bytes (allocated in host memory)
    pub context_bytes: Vec<u8>,
    /// Return code from WASM callback (0 = accept, non-zero = reject)
    pub return_code: i32,
    /// Log buffer for WASM module output
    pub log_buffer: Vec<String>,
    /// Maximum memory in bytes
    max_memory_bytes: usize,
}

impl WasmStore {
    /// Create a new store with serialized context
    ///
    /// # Errors
    /// Returns error if context serialization fails
    pub fn new(context: &Context, max_memory_bytes: usize) -> Result<Self> {
        let serializable = SerializableContext::from(context);
        // Use bincode with serde feature for serialization
        let context_bytes =
            bincode::serde::encode_to_vec(&serializable, bincode::config::standard()).map_err(
                |e| {
                    internal!(level = INFO, "Error creating WasmStore: {e}");
                    match e {
                        bincode::error::EncodeError::Io { inner, .. } => WasmError::Io(inner),
                        e => WasmError::Serialization(e),
                    }
                },
            )?;

        Ok(Self {
            context_bytes,
            return_code: 0,
            log_buffer: Vec::new(),
            max_memory_bytes,
        })
    }
}

/// Implement `ResourceLimiter` to enforce memory caps
impl ResourceLimiter for WasmStore {
    fn memory_growing(
        &mut self,
        _current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> anyhow::Result<bool> {
        // Allow growth if it doesn't exceed our limit
        Ok(desired <= self.max_memory_bytes)
    }

    fn table_growing(
        &mut self,
        _current: usize,
        _desired: usize,
        _maximum: Option<usize>,
    ) -> anyhow::Result<bool> {
        // Allow table growth (tables are much smaller than memory)
        Ok(true)
    }
}

/// Add host import functions to the linker
///
/// These functions are available to WASM modules for accessing Context
/// data and interacting with the host runtime.
///
/// # Errors
/// Returns error if any host function fails to link
pub fn add_host_imports(linker: &mut wasmtime::Linker<WasmStore>) -> Result<()> {
    internal!(level = INFO, "Adding Host Imports ...");

    // Memory management functions
    linker.func_wrap("env", "em_alloc", _em_alloc)?;
    linker.func_wrap("env", "em_free", _em_free)?;

    // Context access functions
    linker.func_wrap("env", "em_get_context", _em_get_context)?;
    linker.func_wrap("env", "em_get_context_size", _em_get_context_size)?;

    // Logging functions
    linker.func_wrap("env", "em_log", _em_log)?;

    internal!(level = INFO, "Host Imports Added");
    Ok(())
}

/// Allocate memory in WASM linear memory
///
/// # Arguments
/// * `size` - Number of bytes to allocate
///
/// # Returns
/// Pointer to allocated memory (offset in WASM linear memory)
fn _em_alloc(mut caller: Caller<'_, WasmStore>, size: u32) -> u32 {
    let Some(wasmtime::Extern::Memory(memory)) = caller.get_export("memory") else {
        tracing::error!("WASM module does not export 'memory'");
        return 0;
    };

    // Get current memory size in bytes
    let current_size = memory.data_size(&caller);

    // Simple bump allocator: return current end as allocation pointer
    // WASM module is responsible for tracking allocations
    let ptr = u32::try_from(current_size).unwrap_or_default();

    // Grow memory if needed
    let pages_needed = u64::from(size.div_ceil(65536));
    if pages_needed > 0 && memory.grow(&mut caller, pages_needed).is_err() {
        tracing::error!("Failed to grow WASM memory by {pages_needed} pages");
        return 0;
    }

    ptr
}

/// Free memory in WASM linear memory
///
/// # Arguments
/// * `ptr` - Pointer to memory to free (currently a no-op)
///
/// Note: This is a placeholder. Real implementation would need a proper allocator.
#[allow(clippy::needless_pass_by_value)]
const fn _em_free(_caller: Caller<'_, WasmStore>, _ptr: u32) {
    // No-op for now - WASM modules use simple bump allocation
    // Real implementation would need a proper allocator (e.g., wee_alloc)
}

/// Get serialized Context data
///
/// # Arguments
/// * `ptr` - Pointer in WASM memory where Context bytes should be written
/// * `max_len` - Maximum bytes to write
///
/// # Returns
/// Number of bytes written (0 on error)
fn _em_get_context(mut caller: Caller<'_, WasmStore>, ptr: u32, max_len: u32) -> u32 {
    let Some(wasmtime::Extern::Memory(memory)) = caller.get_export("memory") else {
        tracing::error!("WASM module does not export 'memory'");
        return 0;
    };

    // Clone the bytes to avoid holding immutable borrow during mutable write
    let context_bytes = caller.data().context_bytes.clone();
    let len = context_bytes.len().min(max_len as usize);

    // Write context bytes to WASM memory
    if let Err(e) = memory.write(&mut caller, ptr as usize, &context_bytes[..len]) {
        tracing::error!("Failed to write context to WASM memory: {e}");
        return 0;
    }

    u32::try_from(len).unwrap_or_default()
}

/// Get size of serialized Context in bytes
///
/// # Returns
/// Size of context data in bytes
#[allow(clippy::needless_pass_by_value)]
fn _em_get_context_size(caller: Caller<'_, WasmStore>) -> u32 {
    u32::try_from(caller.data().context_bytes.len()).unwrap_or_default()
}

/// Log a message from WASM module
///
/// # Arguments
/// * `ptr` - Pointer to null-terminated string in WASM memory
/// * `len` - Length of string (excluding null terminator)
fn _em_log(mut caller: Caller<'_, WasmStore>, ptr: u32, len: u32) {
    let Some(wasmtime::Extern::Memory(memory)) = caller.get_export("memory") else {
        tracing::error!("WASM module does not export 'memory'");
        return;
    };

    // Read string from WASM memory
    let mut buf = vec![0u8; len as usize];
    if let Err(e) = memory.read(&caller, ptr as usize, &mut buf) {
        tracing::error!("Failed to read log message from WASM memory: {e}");
        return;
    }

    match String::from_utf8(buf) {
        Ok(msg) => {
            tracing::info!("[WASM] {msg}");
            caller.data_mut().log_buffer.push(msg);
        }
        Err(e) => {
            tracing::error!("Invalid UTF-8 in WASM log message: {e}");
        }
    }
}
