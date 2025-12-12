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
use empath_common::{
    context::Context,
    internal,
    message_api::{
        BodyError, BodyLimits, BodyMut, BodyOpts, Charset, HeaderError, HeaderLimits, HeaderName,
        HeaderValue, HeadersMut, NewlineStyle,
    },
};
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
    // Body is not inlined; modules should fetch via host::message APIs when needed.
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
            // Do not inline message data; modules can request body bytes via host functions.
            data: None,
            max_message_size: ctx.max_message_size,
            transaction: ctx
                .transaction
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect(),
            tracking_id: ctx.tracking_id.as_ref().map(ToString::to_string),
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

/// Implement the Host trait from generated WIT bindings (core host + message helpers).
///
/// All host functions gracefully handle poisoned mutexes by logging an error and
/// returning a safe default value instead of panicking.
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

impl crate::bindings::empath::validator::message::Host for WasmStore {
    fn raw_bytes(&mut self) -> Vec<u8> {
        self.try_lock_context()
            .ok()
            .and_then(|ctx| ctx.data.as_deref().map(<[u8]>::to_vec))
            .unwrap_or_default()
    }

    fn headers_get(&mut self, name: String) -> Vec<String> {
        let Ok(hname) = HeaderName::new(name) else {
            return Vec::new();
        };
        self.try_lock_context()
            .ok()
            .and_then(|mut ctx| {
                ctx.with_message_parts_mut(|parts| {
                    let view = parts.headers_view();
                    view.get_all(&hname)
                        .map(|v| String::from_utf8_lossy(v.as_bytes()).to_string())
                        .collect::<Vec<_>>()
                })
                .ok()
                .flatten()
            })
            .unwrap_or_default()
    }

    fn headers_contains(&mut self, name: String) -> bool {
        let Ok(hname) = HeaderName::new(name) else {
            return false;
        };
        self.try_lock_context()
            .ok()
            .and_then(|mut ctx| {
                ctx.with_message_parts_mut(|parts| {
                    let view = parts.headers_view();
                    view.contains(&hname)
                })
                .ok()
                .flatten()
            })
            .unwrap_or(false)
    }

    fn headers_set(&mut self, name: String, value: String) -> i32 {
        self.with_headers_mut(&name, &value, |mut headers, n, v| headers.set(&n, &v))
    }

    fn headers_append(&mut self, name: String, value: String) -> i32 {
        self.with_headers_mut(&name, &value, |mut headers, n, v| headers.append(&n, &v))
    }

    fn headers_set_if_absent(
        &mut self,
        name: String,
        value: String,
    ) -> std::result::Result<bool, i32> {
        let Ok(mut ctx) = self.try_lock_context() else {
            return Err(-1);
        };
        match ctx.with_message_parts_mut(|parts| {
            let mut h = parts.headers_mut(HeaderLimits::default(), None);
            let hname = HeaderName::new(name.clone()).map_err(|_| -1)?;
            let hval = HeaderValue::new(value.as_bytes()).map_err(|_| -1)?;
            h.set_if_absent(&hname, &hval).map_err(|_| -1)
        }) {
            Ok(Some(Ok(res))) => Ok(res),
            _ => Err(-1),
        }
    }

    fn headers_remove(&mut self, name: String) -> u32 {
        let Ok(hname) = HeaderName::new(name) else {
            return 0;
        };
        self.try_lock_context()
            .ok()
            .and_then(|mut ctx| {
                ctx.with_message_parts_mut(|parts| {
                    let mut h = parts.headers_mut(HeaderLimits::default(), None);
                    u32::try_from(h.remove(&hname)).unwrap_or(u32::MAX)
                })
                .ok()
                .flatten()
            })
            .unwrap_or(0)
    }

    fn headers_ensure_message_id(&mut self) -> i32 {
        self.try_lock_context().map_or(-1, |mut ctx| {
            match ctx.with_message_parts_mut(|parts| {
                let mut h = parts.headers_mut(HeaderLimits::default(), None);
                h.ensure_message_id().map(|()| 0).map_err(|_| -1)
            }) {
                Ok(Some(Ok(res))) => res,
                _ => -1,
            }
        })
    }

    fn headers_set_content_type(&mut self, value: String) -> i32 {
        self.with_headers_mut("Content-Type", &value, |mut headers, n, v| {
            headers.set(&n, &v)
        })
    }

    fn headers_set_transfer_encoding(&mut self, value: String) -> i32 {
        self.with_headers_mut("Content-Transfer-Encoding", &value, |mut headers, n, v| {
            headers.set(&n, &v)
        })
    }

    fn body_bytes(&mut self) -> Vec<u8> {
        self.try_lock_context()
            .ok()
            .and_then(|mut ctx| {
                ctx.with_message_parts_mut(|parts| parts.body_view().as_bytes().to_vec())
                    .ok()
                    .flatten()
            })
            .unwrap_or_default()
    }

    fn body_newline_style(&mut self) -> String {
        let style = self
            .try_lock_context()
            .ok()
            .and_then(|mut ctx| {
                ctx.with_message_parts_mut(|parts| parts.body_view().newline_style())
                    .ok()
                    .flatten()
            })
            .unwrap_or(NewlineStyle::Lf);
        match style {
            NewlineStyle::Crlf => "crlf",
            NewlineStyle::Lf => "lf",
            NewlineStyle::Mixed => "mixed",
        }
        .to_string()
    }

    fn body_set_bytes(&mut self, data: Vec<u8>) -> i32 {
        self.with_body_mut(data.as_slice(), |mut body, buf| {
            body.set_bytes(buf, BodyOpts::default())
        })
    }

    fn body_append_bytes(&mut self, data: Vec<u8>) -> i32 {
        self.with_body_mut(data.as_slice(), |mut body, buf| {
            body.append_bytes(buf, BodyOpts::default())
        })
    }

    fn body_set_text(&mut self, text: String, charset: String) -> i32 {
        let bytes = text.into_bytes();
        self.with_body_mut(bytes.as_slice(), |mut body, buf| {
            let charset = charset_from_string(&charset);
            body.set_text(&String::from_utf8_lossy(buf), &charset, BodyOpts::default())
        })
    }

    fn body_append_text(&mut self, text: String, charset: String) -> i32 {
        let bytes = text.into_bytes();
        self.with_body_mut(bytes.as_slice(), |mut body, buf| {
            let charset = charset_from_string(&charset);
            body.append_text(&String::from_utf8_lossy(buf), &charset, BodyOpts::default())
        })
    }

    fn body_set_html(&mut self, html: String, charset: String) -> i32 {
        let bytes = html.into_bytes();
        self.with_body_mut(bytes.as_slice(), |mut body, buf| {
            let charset = charset_from_string(&charset);
            body.set_html(&String::from_utf8_lossy(buf), &charset, BodyOpts::default())
        })
    }

    fn body_append_html(&mut self, html: String, charset: String) -> i32 {
        let bytes = html.into_bytes();
        self.with_body_mut(bytes.as_slice(), |mut body, buf| {
            let charset = charset_from_string(&charset);
            body.append_html(&String::from_utf8_lossy(buf), &charset, BodyOpts::default())
        })
    }

    fn body_set_alternative(&mut self, text: String, html: String, charset: String) -> i32 {
        self.try_lock_context().map_or(-1, |mut ctx| {
            match ctx.with_message_parts_mut(|parts| {
                let mut body = parts.body_mut(BodyLimits::default(), None);
                let charset = charset_from_string(&charset);
                body.set_alternative(&text, &html, &charset, BodyOpts::default())
                    .map(|()| 0)
                    .map_err(|_| -1)
            }) {
                Ok(Some(Ok(res))) => res,
                _ => -1,
            }
        })
    }
}

impl WasmStore {
    fn with_headers_mut<F>(&self, name: &str, value: &str, mut f: F) -> i32
    where
        F: FnMut(HeadersMut<'_>, HeaderName, HeaderValue) -> std::result::Result<(), HeaderError>,
    {
        self.try_lock_context().map_or(-1, |mut ctx| {
            match ctx.with_message_parts_mut(|parts| {
                let h = parts.headers_mut(HeaderLimits::default(), None);
                let hname = HeaderName::new(name).map_err(|_| -1)?;
                let hval = HeaderValue::new(value.as_bytes()).map_err(|_| -1)?;
                f(h, hname, hval).map(|()| 0).map_err(|_| -1)
            }) {
                Ok(Some(Ok(res))) => res,
                _ => -1,
            }
        })
    }

    fn with_body_mut<F>(&self, data: &[u8], mut f: F) -> i32
    where
        F: FnMut(BodyMut<'_>, &[u8]) -> std::result::Result<(), BodyError>,
    {
        self.try_lock_context().map_or(-1, |mut ctx| {
            match ctx.with_message_parts_mut(|parts| {
                let body = parts.body_mut(BodyLimits::default(), None);
                f(body, data).map(|()| 0).map_err(|_| -1)
            }) {
                Ok(Some(Ok(res))) => res,
                _ => -1,
            }
        })
    }
}

fn charset_from_string(s: &str) -> Charset {
    match s.to_ascii_lowercase().as_str() {
        "us-ascii" | "ascii" => Charset::Ascii,
        "utf-8" | "utf8" => Charset::Utf8,
        other => Charset::Other(Cow::Owned(other.to_string())),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

    use empath_common::message_api::MessageParts;
    use wasmtime_wasi::WasiCtxBuilder;

    use super::*;
    use crate::bindings::empath::validator::{host::Host, message::Host as MessageHost};

    fn poison_context(context: &Arc<Mutex<Context>>) {
        let ctx_clone = Arc::clone(context);
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _guard = ctx_clone.lock().unwrap();
            panic!("intentionally poison mutex");
        }));
    }

    fn make_store(context: Arc<Mutex<Context>>) -> WasmStore {
        let wasi_ctx = WasiCtxBuilder::new().build();
        WasmStore::new(context, 1024, Duration::from_secs(1), wasi_ctx)
            .expect("store creation should succeed")
    }

    #[test]
    fn poisoned_mutex_returns_safe_defaults() {
        let context = Arc::new(Mutex::new(Context::default()));
        let mut store = make_store(Arc::clone(&context));

        poison_context(&context);

        // Poisoned mutex should surface as an error but not panic
        assert!(store.try_lock_context().is_err());

        store.session_set("session_key".into(), "value".into());
        assert!(!store.session_exists("session_key".into()));
        assert_eq!(store.session_get("session_key".into()), None);

        store.transaction_set("txn_key".into(), "value".into());
        assert!(!store.transaction_exists("txn_key".into()));
        assert_eq!(store.transaction_get("txn_key".into()), None);

        let guard = context
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert!(guard.session.is_empty());
        assert!(guard.transaction.is_empty());
        drop(guard);
    }

    #[test]
    fn memory_growth_respects_limit() {
        let context = Arc::new(Mutex::new(Context::default()));
        let mut store = make_store(context);

        // Allow growth within limit
        assert!(store.memory_growing(0, 512, None).unwrap());
        // Reject growth above limit
        assert!(!store.memory_growing(0, 2048, None).unwrap());
    }

    #[test]
    fn raw_bytes_exposes_message_data() {
        let context = Context {
            data: Some(Arc::from(b"hello world".to_vec())),
            ..Default::default()
        };

        let mut store = make_store(Arc::new(Mutex::new(context)));

        assert_eq!(
            <WasmStore as MessageHost>::raw_bytes(&mut store),
            b"hello world".to_vec(),
            "raw_bytes should surface original message data"
        );
    }

    #[test]
    fn message_helpers_persist_changes() {
        let msg = b"Subject: Old\r\n\r\nBody";
        let context = Arc::new(Mutex::new(Context {
            data: Some(Arc::from(msg.as_slice())),
            ..Context::default()
        }));
        let mut store = make_store(Arc::clone(&context));

        assert_eq!(
            MessageHost::headers_set(&mut store, "X-Test".into(), "ok".into()),
            0
        );
        assert_eq!(
            MessageHost::body_append_text(&mut store, " tail".into(), "utf-8".into()),
            0
        );

        let data = {
            let ctx = context.lock().unwrap();
            ctx.data.as_deref().unwrap().to_vec()
        };
        let parts = MessageParts::from_bytes(&data).expect("parse");

        let headers = parts.headers_view();
        let value = headers
            .get(&HeaderName::new("X-Test").unwrap())
            .map(|v| String::from_utf8_lossy(v.as_bytes()).to_string());
        assert_eq!(value.as_deref(), Some("ok"));

        let body = String::from_utf8_lossy(parts.body_view().as_bytes()).to_string();
        assert!(body.ends_with("Body tail"));
    }
}
