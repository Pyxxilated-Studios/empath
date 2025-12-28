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

use empath_common::{
    audit,
    context::{Context, OUTBOUND_BINDING_CONTEXT_KEY},
    internal,
    message_api::{
        BodyError, BodyLimits, BodyMut, BodyOpts, Charset, HeaderError, HeaderLimits, HeaderName,
        HeaderValue, HeadersMut, MutationAudit, MutationKind, NewlineStyle,
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
        let recipients = ctx.envelope.recipients().map_or_else(Vec::new, |addrs| {
            let mut recipients = Vec::with_capacity(addrs.len());
            for addr in addrs.iter() {
                recipients.push(addr.to_string());
            }
            recipients
        });
        let mut transaction = Vec::with_capacity(ctx.transaction.len());
        for (key, value) in &ctx.transaction {
            transaction.push((key.to_string(), value.clone()));
        }

        Self {
            extended: ctx.extended,
            id: ctx.id.clone(),
            sender: ctx.envelope.sender().map(ToString::to_string),
            recipients,
            // Do not inline message data; modules can request body bytes via host functions.
            data: None,
            max_message_size: ctx.max_message_size,
            transaction,
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
    /// Configured outbound bindings for validation helpers
    pub outbound_bindings: Arc<[String]>,
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
        outbound_bindings: Arc<[String]>,
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

            postcard::to_allocvec(&serializable).map_err(WasmError::Protocol)?
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
            outbound_bindings,
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

    fn outbound_bindings(&mut self) -> Vec<String> {
        self.outbound_bindings.to_vec()
    }

    fn set_outbound_binding(
        &mut self,
        binding: crate::bindings::empath::validator::host::OutboundBinding,
    ) -> bool {
        let Ok(mut ctx) = self.try_lock_context() else {
            return false;
        };

        match binding {
            crate::bindings::empath::validator::host::OutboundBinding::Default => {
                ctx.transaction.remove(OUTBOUND_BINDING_CONTEXT_KEY);
                true
            }
            crate::bindings::empath::validator::host::OutboundBinding::Named(name) => {
                if !self
                    .outbound_bindings
                    .iter()
                    .any(|binding| binding == &name)
                {
                    return false;
                }
                ctx.transaction
                    .insert(Cow::Borrowed(OUTBOUND_BINDING_CONTEXT_KEY), name);
                true
            }
        }
    }
}

impl crate::bindings::empath::validator::message::Host for WasmStore {
    fn raw_len(
        &mut self,
    ) -> std::result::Result<u64, crate::bindings::empath::validator::message::ReadError> {
        use crate::bindings::empath::validator::message::ReadError;

        self.try_lock_context()
            .map_err(|_| ReadError::HostUnavailable)
            .and_then(|ctx| {
                let data = ctx.data.as_deref().ok_or(ReadError::MissingData)?;
                Ok(data.len() as u64)
            })
    }

    fn raw_read(
        &mut self,
        offset: u64,
        len: u32,
    ) -> std::result::Result<Vec<u8>, crate::bindings::empath::validator::message::ReadError> {
        use crate::bindings::empath::validator::message::ReadError;

        self.try_lock_context()
            .map_err(|_| ReadError::HostUnavailable)
            .and_then(|ctx| {
                let data = ctx.data.as_deref().ok_or(ReadError::MissingData)?;
                slice_bytes(data, offset, len)
            })
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
        self.with_headers_mut(&name, &value, "set", |mut headers, n, v| {
            headers.set(&n, &v)
        })
    }

    fn headers_append(&mut self, name: String, value: String) -> i32 {
        self.with_headers_mut(&name, &value, "append", |mut headers, n, v| {
            headers.append(&n, &v)
        })
    }

    fn headers_set_if_absent(
        &mut self,
        name: String,
        value: String,
    ) -> std::result::Result<bool, i32> {
        let Ok(mut ctx) = self.try_lock_context() else {
            return Err(-1);
        };
        let module_context = module_context_snapshot(&ctx);
        let mut audit_log = module_context
            .as_ref()
            .map(|module| MutationAudit::new(module.1.clone()));

        let result = ctx.with_message_parts_mut(|parts| {
            let mut h = parts.headers_mut(HeaderLimits::default(), audit_log.as_mut());
            let hname = HeaderName::new(name.clone())?;
            let hval = HeaderValue::new(value.as_bytes())?;
            h.set_if_absent(&hname, &hval)
        });

        match result {
            Ok(Some(Ok(updated))) => {
                if updated
                    && let (Some(module_context), Some(audit_log)) =
                        (module_context.as_ref(), audit_log.as_ref())
                {
                    record_mutation_records(&ctx, module_context, audit_log);
                }
                Ok(updated)
            }
            Ok(Some(Err(err))) => {
                record_mutation_failure(
                    module_context.as_ref(),
                    "header",
                    "set",
                    header_error_kind(err),
                );
                Err(-1)
            }
            Ok(None) => {
                record_mutation_failure(module_context.as_ref(), "header", "set", "missing_data");
                Err(-1)
            }
            Err(_) => {
                record_mutation_failure(module_context.as_ref(), "header", "set", "parse_error");
                Err(-1)
            }
        }
    }

    fn headers_remove(&mut self, name: String) -> u32 {
        let Ok(hname) = HeaderName::new(name) else {
            return 0;
        };
        self.try_lock_context()
            .ok()
            .and_then(|mut ctx| {
                let module_context = module_context_snapshot(&ctx);
                let mut audit_log = module_context
                    .as_ref()
                    .map(|module| MutationAudit::new(module.1.clone()));

                let result = ctx.with_message_parts_mut(|parts| {
                    let mut h = parts.headers_mut(HeaderLimits::default(), audit_log.as_mut());
                    u32::try_from(h.remove(&hname)).unwrap_or(u32::MAX)
                });

                match result {
                    Ok(Some(count)) => {
                        if let (Some(module_context), Some(audit_log)) =
                            (module_context.as_ref(), audit_log.as_ref())
                        {
                            record_mutation_records(&ctx, module_context, audit_log);
                        }
                        Some(count)
                    }
                    Ok(None) => {
                        record_mutation_failure(
                            module_context.as_ref(),
                            "header",
                            "remove",
                            "missing_data",
                        );
                        None
                    }
                    Err(_) => {
                        record_mutation_failure(
                            module_context.as_ref(),
                            "header",
                            "remove",
                            "parse_error",
                        );
                        None
                    }
                }
            })
            .unwrap_or(0)
    }

    fn headers_ensure_message_id(&mut self) -> i32 {
        self.try_lock_context().map_or(-1, |mut ctx| {
            let module_context = module_context_snapshot(&ctx);
            let mut audit_log = module_context
                .as_ref()
                .map(|module| MutationAudit::new(module.1.clone()));

            let result = ctx.with_message_parts_mut(|parts| {
                let mut h = parts.headers_mut(HeaderLimits::default(), audit_log.as_mut());
                h.ensure_message_id()
            });

            match result {
                Ok(Some(Ok(()))) => {
                    if let (Some(module_context), Some(audit_log)) =
                        (module_context.as_ref(), audit_log.as_ref())
                    {
                        record_mutation_records(&ctx, module_context, audit_log);
                    }
                    0
                }
                Ok(Some(Err(err))) => {
                    record_mutation_failure(
                        module_context.as_ref(),
                        "header",
                        "set",
                        header_error_kind(err),
                    );
                    -1
                }
                Ok(None) => {
                    record_mutation_failure(
                        module_context.as_ref(),
                        "header",
                        "set",
                        "missing_data",
                    );
                    -1
                }
                Err(_) => {
                    record_mutation_failure(
                        module_context.as_ref(),
                        "header",
                        "set",
                        "parse_error",
                    );
                    -1
                }
            }
        })
    }

    fn headers_set_content_type(&mut self, value: String) -> i32 {
        self.with_headers_mut("Content-Type", &value, "set", |mut headers, n, v| {
            headers.set(&n, &v)
        })
    }

    fn headers_set_transfer_encoding(&mut self, value: String) -> i32 {
        self.with_headers_mut(
            "Content-Transfer-Encoding",
            &value,
            "set",
            |mut headers, n, v| headers.set(&n, &v),
        )
    }

    fn body_len(
        &mut self,
    ) -> std::result::Result<u64, crate::bindings::empath::validator::message::ReadError> {
        use crate::bindings::empath::validator::message::ReadError;

        let mut ctx = self
            .try_lock_context()
            .map_err(|_| ReadError::HostUnavailable)?;
        match ctx.with_message_parts_mut(|parts| parts.body_view().as_bytes().len() as u64) {
            Ok(Some(size)) => Ok(size),
            Ok(None) => Err(ReadError::MissingData),
            Err(_) => Err(ReadError::ParseFailed),
        }
    }

    fn body_read(
        &mut self,
        offset: u64,
        len: u32,
    ) -> std::result::Result<Vec<u8>, crate::bindings::empath::validator::message::ReadError> {
        use crate::bindings::empath::validator::message::ReadError;

        let mut ctx = self
            .try_lock_context()
            .map_err(|_| ReadError::HostUnavailable)?;
        match ctx.with_message_parts_mut(|parts| {
            let body = parts.body_view().as_bytes();
            slice_bytes(body, offset, len)
        }) {
            Ok(Some(result)) => result,
            Ok(None) => Err(ReadError::MissingData),
            Err(_) => Err(ReadError::ParseFailed),
        }
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
        self.with_body_mut(data.as_slice(), "set-bytes", |mut body, buf| {
            body.set_bytes(buf, BodyOpts::default())
        })
    }

    fn body_append_bytes(&mut self, data: Vec<u8>) -> i32 {
        self.with_body_mut(data.as_slice(), "append-bytes", |mut body, buf| {
            body.append_bytes(buf, BodyOpts::default())
        })
    }

    fn body_set_text(&mut self, text: String, charset: String) -> i32 {
        let bytes = text.into_bytes();
        self.with_body_mut(bytes.as_slice(), "set-bytes", |mut body, buf| {
            let charset = charset_from_string(&charset);
            body.set_text(&String::from_utf8_lossy(buf), &charset, BodyOpts::default())
        })
    }

    fn body_append_text(&mut self, text: String, charset: String) -> i32 {
        let bytes = text.into_bytes();
        self.with_body_mut(bytes.as_slice(), "append-bytes", |mut body, buf| {
            let charset = charset_from_string(&charset);
            body.append_text(&String::from_utf8_lossy(buf), &charset, BodyOpts::default())
        })
    }

    fn body_set_html(&mut self, html: String, charset: String) -> i32 {
        let bytes = html.into_bytes();
        self.with_body_mut(bytes.as_slice(), "set-bytes", |mut body, buf| {
            let charset = charset_from_string(&charset);
            body.set_html(&String::from_utf8_lossy(buf), &charset, BodyOpts::default())
        })
    }

    fn body_append_html(&mut self, html: String, charset: String) -> i32 {
        let bytes = html.into_bytes();
        self.with_body_mut(bytes.as_slice(), "append-bytes", |mut body, buf| {
            let charset = charset_from_string(&charset);
            body.append_html(&String::from_utf8_lossy(buf), &charset, BodyOpts::default())
        })
    }

    fn body_set_alternative(&mut self, text: String, html: String, charset: String) -> i32 {
        self.try_lock_context().map_or(-1, |mut ctx| {
            let module_context = module_context_snapshot(&ctx);
            let mut audit_log = module_context
                .as_ref()
                .map(|module| MutationAudit::new(module.1.clone()));

            let result = ctx.with_message_parts_mut(|parts| {
                let mut body = parts.body_mut(BodyLimits::default(), audit_log.as_mut());
                let charset = charset_from_string(&charset);
                body.set_alternative(&text, &html, &charset, BodyOpts::default())
            });

            match result {
                Ok(Some(Ok(()))) => {
                    if let (Some(module_context), Some(audit_log)) =
                        (module_context.as_ref(), audit_log.as_ref())
                    {
                        record_mutation_records(&ctx, module_context, audit_log);
                    }
                    0
                }
                Ok(Some(Err(err))) => {
                    record_mutation_failure(
                        module_context.as_ref(),
                        "body",
                        "set-alternative",
                        body_error_kind(err),
                    );
                    -1
                }
                Ok(None) => {
                    record_mutation_failure(
                        module_context.as_ref(),
                        "body",
                        "set-alternative",
                        "missing_data",
                    );
                    -1
                }
                Err(_) => {
                    record_mutation_failure(
                        module_context.as_ref(),
                        "body",
                        "set-alternative",
                        "parse_error",
                    );
                    -1
                }
            }
        })
    }
}

impl WasmStore {
    fn with_headers_mut<F>(&self, name: &str, value: &str, action: &str, mut f: F) -> i32
    where
        F: FnMut(HeadersMut<'_>, HeaderName, HeaderValue) -> std::result::Result<(), HeaderError>,
    {
        self.try_lock_context().map_or(-1, |mut ctx| {
            let module_context = module_context_snapshot(&ctx);
            let mut audit_log = module_context
                .as_ref()
                .map(|module| MutationAudit::new(module.1.clone()));

            let result = ctx.with_message_parts_mut(|parts| {
                let h = parts.headers_mut(HeaderLimits::default(), audit_log.as_mut());
                let hname = HeaderName::new(name)?;
                let hval = HeaderValue::new(value.as_bytes())?;
                f(h, hname, hval)
            });

            match result {
                Ok(Some(Ok(()))) => {
                    if let (Some(module_context), Some(audit_log)) =
                        (module_context.as_ref(), audit_log.as_ref())
                    {
                        record_mutation_records(&ctx, module_context, audit_log);
                    }
                    0
                }
                Ok(Some(Err(err))) => {
                    record_mutation_failure(
                        module_context.as_ref(),
                        "header",
                        action,
                        header_error_kind(err),
                    );
                    -1
                }
                Ok(None) => {
                    record_mutation_failure(
                        module_context.as_ref(),
                        "header",
                        action,
                        "missing_data",
                    );
                    -1
                }
                Err(_) => {
                    record_mutation_failure(
                        module_context.as_ref(),
                        "header",
                        action,
                        "parse_error",
                    );
                    -1
                }
            }
        })
    }

    fn with_body_mut<F>(&self, data: &[u8], action: &str, mut f: F) -> i32
    where
        F: FnMut(BodyMut<'_>, &[u8]) -> std::result::Result<(), BodyError>,
    {
        self.try_lock_context().map_or(-1, |mut ctx| {
            let module_context = module_context_snapshot(&ctx);
            let mut audit_log = module_context
                .as_ref()
                .map(|module| MutationAudit::new(module.1.clone()));

            let result = ctx.with_message_parts_mut(|parts| {
                let body = parts.body_mut(BodyLimits::default(), audit_log.as_mut());
                f(body, data)
            });

            match result {
                Ok(Some(Ok(()))) => {
                    if let (Some(module_context), Some(audit_log)) =
                        (module_context.as_ref(), audit_log.as_ref())
                    {
                        record_mutation_records(&ctx, module_context, audit_log);
                    }
                    0
                }
                Ok(Some(Err(err))) => {
                    record_mutation_failure(
                        module_context.as_ref(),
                        "body",
                        action,
                        body_error_kind(err),
                    );
                    -1
                }
                Ok(None) => {
                    record_mutation_failure(
                        module_context.as_ref(),
                        "body",
                        action,
                        "missing_data",
                    );
                    -1
                }
                Err(_) => {
                    record_mutation_failure(module_context.as_ref(), "body", action, "parse_error");
                    -1
                }
            }
        })
    }
}

fn module_context_snapshot(context: &Context) -> Option<(String, String)> {
    context
        .module_context
        .as_ref()
        .map(|module| (module.module_type.clone(), module.module_name.clone()))
}

fn record_mutation_records(
    context: &Context,
    module_context: &(String, String),
    audit_log: &MutationAudit,
) {
    let (module_type, module_name) = module_context;
    for record in audit_log.records() {
        audit::log_module_mutation(context.id(), module_type, module_name, record);
        if let Some(metrics) = empath_metrics::try_metrics() {
            let mutation_kind = match record.kind {
                MutationKind::Header { .. } => "header",
                MutationKind::Body => "body",
            };
            metrics.modules.record_mutation(
                module_type,
                module_name,
                mutation_kind,
                &record.action,
                "success",
                None,
            );
        }
    }
}

fn record_mutation_failure(
    module_context: Option<&(String, String)>,
    mutation_kind: &str,
    action: &str,
    error_kind: &str,
) {
    let Some((module_type, module_name)) = module_context else {
        return;
    };
    if let Some(metrics) = empath_metrics::try_metrics() {
        metrics.modules.record_mutation(
            module_type,
            module_name,
            mutation_kind,
            action,
            "failure",
            Some(error_kind),
        );
    }
}

const fn header_error_kind(err: HeaderError) -> &'static str {
    match err {
        HeaderError::TooMany => "too_many",
        HeaderError::NameTooLong => "name_too_long",
        HeaderError::ValueTooLong => "value_too_long",
        HeaderError::TotalSizeExceeded => "total_size_exceeded",
        HeaderError::InvalidName => "invalid_name",
        HeaderError::InvalidValue => "invalid_value",
        HeaderError::SignatureProtected => "signature_protected",
    }
}

const fn body_error_kind(err: BodyError) -> &'static str {
    match err {
        BodyError::TooLarge => "too_large",
        BodyError::GrowthExceeded => "growth_exceeded",
        BodyError::InvalidCharset => "invalid_charset",
        BodyError::InvalidTransferEncoding => "invalid_transfer_encoding",
        BodyError::MultipartError => "multipart_error",
    }
}

fn slice_bytes(
    data: &[u8],
    offset: u64,
    len: u32,
) -> std::result::Result<Vec<u8>, crate::bindings::empath::validator::message::ReadError> {
    use crate::bindings::empath::validator::message::ReadError;

    let Ok(start) = usize::try_from(offset) else {
        return Err(ReadError::OutOfRange);
    };
    if start >= data.len() {
        return Err(ReadError::OutOfRange);
    }
    let Ok(mut length) = usize::try_from(len) else {
        return Err(ReadError::OutOfRange);
    };
    let remaining = data.len() - start;
    if length > remaining {
        length = remaining;
    }
    Ok(data[start..start + length].to_vec())
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
        let outbound_bindings = Arc::from(Vec::<String>::new().into_boxed_slice());
        WasmStore::new(
            context,
            1024,
            Duration::from_secs(1),
            wasi_ctx,
            outbound_bindings,
        )
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
    fn raw_read_exposes_message_data() {
        let context = Context {
            data: Some(Arc::from(b"hello world".to_vec())),
            ..Default::default()
        };

        let mut store = make_store(Arc::new(Mutex::new(context)));

        assert_eq!(MessageHost::raw_len(&mut store), Ok(11));
        assert_eq!(
            MessageHost::raw_read(&mut store, 6, 5),
            Ok(b"world".to_vec()),
            "raw_read should surface original message data"
        );
    }

    #[test]
    fn body_read_errors_on_parse_failure() {
        let context = Context {
            data: Some(Arc::from(b"Subject: Broken Body".to_vec())),
            ..Default::default()
        };
        let mut store = make_store(Arc::new(Mutex::new(context)));

        assert_eq!(
            MessageHost::body_len(&mut store),
            Err(crate::bindings::empath::validator::message::ReadError::ParseFailed)
        );
        assert_eq!(
            MessageHost::body_read(&mut store, 0, 8),
            Err(crate::bindings::empath::validator::message::ReadError::ParseFailed)
        );
    }

    #[test]
    fn raw_read_reports_missing_data() {
        let mut store = make_store(Arc::new(Mutex::new(Context::default())));

        assert_eq!(
            MessageHost::raw_len(&mut store),
            Err(crate::bindings::empath::validator::message::ReadError::MissingData)
        );
        assert_eq!(
            MessageHost::raw_read(&mut store, 0, 4),
            Err(crate::bindings::empath::validator::message::ReadError::MissingData)
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
