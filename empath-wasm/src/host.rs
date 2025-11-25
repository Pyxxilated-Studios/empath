//! Host import functions for WASM modules
//!
//! This module provides host functions that WASM modules can call to interact with
//! the Empath runtime. These functions enable WASM modules to:
//! - Access Context data (sender, recipients, message content)
//! - Allocate and free memory
//! - Log messages
//! - Set response codes

use std::{
    borrow::Cow,
    sync::{Arc, Mutex},
};

use bincode::{Decode, Encode};
use empath_common::{context::Context, internal};
use serde::{Deserialize, Serialize};
use wasmtime::ResourceLimiter;
use wasmtime_wasi::{ResourceTable, WasiCtx, WasiCtxView, WasiView};

use crate::error::{Result, WasmError};

/// Serializable Context representation for passing to WASM
///
/// This is a simplified view of Context that only includes fields
/// needed by WASM modules. The session field is intentionally excluded
/// as it's ephemeral and not persisted.
#[derive(Debug, Clone, Serialize, Deserialize, Encode, Decode)]
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
/// This holds the Context and runtime state that WASM modules can access
/// via host functions (Component Model) or serialized access (core modules).
pub struct WasmStore {
    /// Shared mutable context for component model host functions
    pub context: Arc<Mutex<Context>>,
    /// Serialized context bytes (for backward compatibility with core modules)
    pub context_bytes: Vec<u8>,
    /// Return code from WASM callback (0 = accept, non-zero = reject)
    pub return_code: i32,
    /// Log buffer for WASM module output
    pub log_buffer: Vec<String>,
    /// Maximum memory in bytes
    max_memory_bytes: usize,
    /// WASI context (if WASI is enabled)
    pub wasi_ctx: WasiCtx,
    /// Resource table for component model
    pub resource_table: ResourceTable,
}

impl WasiView for WasmStore {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi_ctx,
            table: &mut self.resource_table,
        }
    }
}

impl WasmStore {
    /// Create a new store with context reference
    ///
    /// # Arguments
    /// * `context` - Shared mutable context wrapped in Arc<Mutex<>>
    /// * `max_memory_bytes` - Maximum memory limit for WASM module
    /// * `wasi_ctx` - WASI context for filesystem/network access
    ///
    /// # Errors
    /// Returns error if context serialization fails
    ///
    /// # Panics
    /// Panics if the context mutex is poisoned (i.e., a thread panicked while holding the lock)
    #[allow(
        clippy::expect_used,
        reason = "Host functions allowed to panic on poisoned mutex"
    )]
    pub fn new(
        context: Arc<Mutex<Context>>,
        max_memory_bytes: usize,
        wasi_ctx: WasiCtx,
    ) -> Result<Self> {
        let context_bytes = {
            let serializable = {
                let ctx_lock = context.lock().expect("Failed to lock context");
                SerializableContext::from(&*ctx_lock)
            };
            // Use bincode for serialization (for backward compat with core modules)
            bincode::encode_to_vec(serializable, bincode::config::standard())
                .map_err(WasmError::Serialization)?
        };

        Ok(Self {
            context,
            context_bytes,
            return_code: 0,
            log_buffer: Vec::new(),
            max_memory_bytes,
            wasi_ctx,
            resource_table: ResourceTable::new(),
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

/// Implement the Host trait from generated WIT bindings
///
/// This provides the host functions that WASM component modules can import.
#[allow(
    clippy::expect_used,
    reason = "Host functions allowed to panic on poisoned mutex"
)]
impl crate::bindings::empath::validator::host::Host for WasmStore {
    fn log(&mut self, message: String) {
        internal!(level = INFO, "[WASM] {}", message);
        self.log_buffer.push(message);
    }

    fn session_set(&mut self, key: String, value: String) {
        let mut ctx = self.context.lock().expect("Failed to lock context");
        ctx.session.insert(Cow::Owned(key), value);
    }

    fn session_get(&mut self, key: String) -> Option<String> {
        let ctx = self.context.lock().expect("Failed to lock context");
        ctx.session.get(key.as_str()).cloned()
    }

    fn session_exists(&mut self, key: String) -> bool {
        let ctx = self.context.lock().expect("Failed to lock context");
        ctx.session.contains_key(key.as_str())
    }

    fn transaction_set(&mut self, key: String, value: String) {
        let mut ctx = self.context.lock().expect("Failed to lock context");
        ctx.transaction.insert(Cow::Owned(key), value);
    }

    fn transaction_get(&mut self, key: String) -> Option<String> {
        let ctx = self.context.lock().expect("Failed to lock context");
        ctx.transaction.get(key.as_str()).cloned()
    }

    fn transaction_exists(&mut self, key: String) -> bool {
        let ctx = self.context.lock().expect("Failed to lock context");
        ctx.transaction.contains_key(key.as_str())
    }
}
