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
    time::Duration,
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
    /// Execution timeout for callbacks
    pub timeout: Duration,
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
    /// Helper method to lock the context mutex with graceful error handling
    ///
    /// Returns `Ok(guard)` on success, or `Err(WasmError)` if the mutex is poisoned.
    /// Poisoned mutexes are logged but don't panic - the caller should handle
    /// the error gracefully (e.g., return `None`, `false`, or just log and return).
    fn try_lock_context(&self) -> Result<std::sync::MutexGuard<'_, Context>> {
        self.context.lock().map_err(|e| {
            internal!(
                level = ERROR,
                "Context mutex poisoned in WASM host function: {}. \
                 Callback will fail gracefully without crashing.",
                e
            );
            WasmError::Internal(format!("Context mutex poisoned: {e}"))
        })
    }

    /// Create a new store with context reference
    ///
    /// # Arguments
    /// * `context` - Shared mutable context wrapped in Arc<Mutex<>>
    /// * `max_memory_bytes` - Maximum memory limit for WASM module
    /// * `wasi_ctx` - WASI context for filesystem/network access
    ///
    /// # Errors
    /// Returns error if context serialization fails or if the context mutex is poisoned
    pub fn new(
        context: Arc<Mutex<Context>>,
        max_memory_bytes: usize,
        timeout: Duration,
        wasi_ctx: WasiCtx,
    ) -> Result<Self> {
        let context_bytes = {
            let serializable = {
                let ctx_lock = context.lock().map_err(|e| {
                    internal!(
                        level = ERROR,
                        "Context mutex poisoned during WasmStore creation: {}",
                        e
                    );
                    WasmError::Internal(format!("Context mutex poisoned: {e}"))
                })?;
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
            timeout,
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
///
/// All host functions gracefully handle poisoned mutexes by logging an error and
/// returning a safe default value instead of panicking:
///
/// - **Functions returning `Option<T>`**: Return `None` on poisoned mutex
/// - **Functions returning `bool`**: Return `false` on poisoned mutex
/// - **Functions returning nothing**: Log error and return early
///
/// This ensures that a poisoned mutex (caused by a panic in another thread) doesn't
/// crash the entire server. The WASM module execution will fail, but the server
/// continues processing other requests.
impl crate::bindings::empath::validator::host::Host for WasmStore {
    fn log(&mut self, message: String) {
        internal!(level = INFO, "[WASM] {}", message);
        self.log_buffer.push(message);
    }

    fn session_set(&mut self, key: String, value: String) {
        if let Ok(mut ctx) = self.try_lock_context() {
            ctx.session.insert(Cow::Owned(key), value);
        }
        // If Err: Mutex poisoned - error already logged, just return
    }

    fn session_get(&mut self, key: String) -> Option<String> {
        self.try_lock_context()
            .ok()?
            .session
            .get(key.as_str())
            .cloned()
    }

    fn session_exists(&mut self, key: String) -> bool {
        self.try_lock_context()
            .ok()
            .is_some_and(|ctx| ctx.session.contains_key(key.as_str()))
    }

    fn transaction_set(&mut self, key: String, value: String) {
        if let Ok(mut ctx) = self.try_lock_context() {
            ctx.transaction.insert(Cow::Owned(key), value);
        }
        // If Err: Mutex poisoned - error already logged, just return
    }

    fn transaction_get(&mut self, key: String) -> Option<String> {
        self.try_lock_context()
            .ok()?
            .transaction
            .get(key.as_str())
            .cloned()
    }

    fn transaction_exists(&mut self, key: String) -> bool {
        self.try_lock_context()
            .ok()
            .is_some_and(|ctx| ctx.transaction.contains_key(key.as_str()))
    }
}
