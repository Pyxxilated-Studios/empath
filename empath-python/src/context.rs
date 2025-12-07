//! Pythonic API for Context access
//!
//! This module provides a `#[pyclass]` wrapper around `empath_common::Context`
//! that exposes a natural Python API with properties and methods.

use std::{borrow::Cow, sync::Arc};

use empath_common::{
    context::Context,
    error::MessageParseError,
    message_api::{
        BodyLimits, BodyOpts, Charset, HeaderLimits, HeaderName, HeaderValue, NewlineStyle,
    },
};
use pyo3::{exceptions::PyValueError, prelude::*};

/// Python wrapper for Context
///
/// This class provides a Pythonic interface to SMTP message context.
/// It exposes envelope data, message content, and metadata with automatic
/// memory management via `PyO3`.
#[pyclass(name = "Context")]
pub struct PyContext {
    /// Reference to the underlying Rust context
    ///
    /// We use Arc to allow cheap cloning when passing to Python callbacks.
    /// The context is mutable via interior mutability patterns when needed.
    inner: Arc<parking_lot::RwLock<Context>>,
}

impl PyContext {
    /// Create a new Python context wrapper
    #[must_use]
    pub fn new(context: Context) -> Self {
        Self {
            inner: Arc::new(parking_lot::RwLock::new(context)),
        }
    }

    /// Get a reference to the inner context
    ///
    /// # Panics
    /// Panics if the lock is poisoned (which shouldn't happen with `parking_lot`)
    #[must_use]
    pub fn inner(&self) -> Arc<parking_lot::RwLock<Context>> {
        Arc::clone(&self.inner)
    }

    fn with_headers_mut<F, R>(
        &self,
        name: &str,
        value: &str,
        mut f: F,
    ) -> PyResult<Option<Result<R, empath_common::message_api::HeaderError>>>
    where
        F: FnMut(
            &mut empath_common::message_api::HeadersMut<'_>,
            &HeaderName,
            &HeaderValue,
        ) -> Result<R, empath_common::message_api::HeaderError>,
    {
        let hname = HeaderName::new(name).map_err(Self::map_header_error)?;
        let hval = HeaderValue::new(value.as_bytes()).map_err(Self::map_header_error)?;
        let mut ctx = self.inner.write();
        ctx.with_message_parts_mut(|parts| {
            let mut headers = parts.headers_mut(HeaderLimits::default(), None);
            f(&mut headers, &hname, &hval)
        })
        .map_err(|err| Self::map_parse_error(&err))
    }

    fn with_body_mut<F, R>(
        &self,
        mut f: F,
    ) -> PyResult<Option<Result<R, empath_common::message_api::BodyError>>>
    where
        F: FnMut(
            &mut empath_common::message_api::BodyMut<'_>,
        ) -> Result<R, empath_common::message_api::BodyError>,
    {
        let mut ctx = self.inner.write();
        ctx.with_message_parts_mut(|parts| {
            let mut body = parts.body_mut(BodyLimits::default(), None);
            f(&mut body)
        })
        .map_err(|err| Self::map_parse_error(&err))
    }

    fn map_parse_error(err: &MessageParseError) -> PyErr {
        PyValueError::new_err(format!("Failed to parse message for mutation: {err}"))
    }

    fn map_header_error(err: empath_common::message_api::HeaderError) -> PyErr {
        let reason = match err {
            empath_common::message_api::HeaderError::TooMany => "too many headers",
            empath_common::message_api::HeaderError::NameTooLong => "header name too long",
            empath_common::message_api::HeaderError::ValueTooLong => "header value too long",
            empath_common::message_api::HeaderError::TotalSizeExceeded => {
                "headers exceed total size limit"
            }
            empath_common::message_api::HeaderError::InvalidName => "invalid header name",
            empath_common::message_api::HeaderError::InvalidValue => "invalid header value",
            empath_common::message_api::HeaderError::SignatureProtected => {
                "header protected by signature"
            }
        };
        PyValueError::new_err(reason)
    }

    fn map_body_error(err: empath_common::message_api::BodyError) -> PyErr {
        let reason = match err {
            empath_common::message_api::BodyError::TooLarge => "body exceeds maximum length",
            empath_common::message_api::BodyError::GrowthExceeded => "body growth exceeds limit",
            empath_common::message_api::BodyError::InvalidCharset => "invalid charset",
            empath_common::message_api::BodyError::InvalidTransferEncoding => {
                "invalid transfer encoding"
            }
            empath_common::message_api::BodyError::MultipartError => "invalid multipart body",
        };
        PyValueError::new_err(reason)
    }

    fn parse_charset(charset: &str) -> Charset {
        let normalized = charset.to_ascii_lowercase();
        match normalized.as_str() {
            "ascii" | "us-ascii" => Charset::Ascii,
            "utf-8" | "utf8" => Charset::Utf8,
            _ => Charset::Other(Cow::Owned(charset.to_string())),
        }
    }
}

#[pymethods]
impl PyContext {
    // ===== Envelope Properties =====

    /// Get the sender address (MAIL FROM)
    #[getter]
    fn sender(&self) -> Option<String> {
        self.inner
            .read()
            .envelope
            .sender()
            .map(std::string::ToString::to_string)
    }

    /// Get the list of recipient addresses (RCPT TO)
    #[getter]
    fn recipients(&self) -> Vec<String> {
        self.inner
            .read()
            .envelope
            .recipients()
            .map_or_else(Vec::new, |recipients| {
                recipients.iter().map(ToString::to_string).collect()
            })
    }

    // ===== Message Data =====

    /// Get the message data as bytes
    #[getter]
    fn data(&self) -> Option<Vec<u8>> {
        self.inner
            .read()
            .data
            .as_ref()
            .map(|arc: &Arc<[u8]>| arc.as_ref().to_vec())
    }

    /// Get the message data as a UTF-8 string
    ///
    /// Returns None if data is not valid UTF-8
    #[getter]
    fn data_str(&self) -> Option<String> {
        self.inner.read().data.as_ref().and_then(|arc: &Arc<[u8]>| {
            std::str::from_utf8(arc.as_ref())
                .ok()
                .map(ToString::to_string)
        })
    }

    // ===== Message mutation helpers =====

    /// Get all values for a header (case-insensitive).
    fn headers_get(&self, name: &str) -> PyResult<Vec<String>> {
        let hname = HeaderName::new(name).map_err(Self::map_header_error)?;
        let mut ctx = self.inner.write();
        ctx.with_message_parts_mut(|parts| {
            let view = parts.headers_view();
            view.get_all(&hname)
                .map(|v| String::from_utf8_lossy(v.as_bytes()).to_string())
                .collect::<Vec<_>>()
        })
        .map_err(|err| Self::map_parse_error(&err))
        .map(Option::unwrap_or_default)
    }

    /// Check if a header exists.
    fn headers_contains(&self, name: &str) -> PyResult<bool> {
        let hname = HeaderName::new(name).map_err(Self::map_header_error)?;
        let mut ctx = self.inner.write();
        ctx.with_message_parts_mut(|parts| {
            let view = parts.headers_view();
            view.contains(&hname)
        })
        .map_err(|err| Self::map_parse_error(&err))
        .map(|res| res.unwrap_or(false))
    }

    /// Set a header value (replaces existing).
    #[allow(clippy::redundant_closure_for_method_calls)]
    fn headers_set(&self, name: &str, value: &str) -> PyResult<bool> {
        match self.with_headers_mut(name, value, |headers, hname, hval| headers.set(hname, hval)) {
            Ok(Some(Ok(()))) => Ok(true),
            Ok(Some(Err(err))) => Err(Self::map_header_error(err)),
            Ok(None) => Ok(false),
            Err(err) => Err(err),
        }
    }

    /// Append a header value (does not replace existing).
    #[allow(clippy::redundant_closure_for_method_calls)]
    fn headers_append(&self, name: &str, value: &str) -> PyResult<bool> {
        match self.with_headers_mut(name, value, |headers, hname, hval| {
            headers.append(hname, hval)
        }) {
            Ok(Some(Ok(()))) => Ok(true),
            Ok(Some(Err(err))) => Err(Self::map_header_error(err)),
            Ok(None) => Ok(false),
            Err(err) => Err(err),
        }
    }

    /// Set a header only if missing. Returns True if newly set.
    #[allow(clippy::redundant_closure_for_method_calls)]
    fn headers_set_if_absent(&self, name: &str, value: &str) -> PyResult<bool> {
        match self.with_headers_mut(name, value, |headers, hname, hval| {
            headers.set_if_absent(hname, hval)
        }) {
            Ok(Some(Ok(updated))) => Ok(updated),
            Ok(Some(Err(err))) => Err(Self::map_header_error(err)),
            Ok(None) => Ok(false),
            Err(err) => Err(err),
        }
    }

    /// Remove all occurrences of a header. Returns number removed.
    fn headers_remove(&self, name: &str) -> PyResult<u32> {
        let hname = HeaderName::new(name).map_err(Self::map_header_error)?;
        let mut ctx = self.inner.write();
        ctx.with_message_parts_mut(|parts| {
            let mut headers = parts.headers_mut(HeaderLimits::default(), None);
            u32::try_from(headers.remove(&hname)).unwrap_or(u32::MAX)
        })
        .map_err(|err| Self::map_parse_error(&err))
        .map(|res| res.unwrap_or(0))
    }

    /// Ensure Message-ID exists (sets if missing).
    fn headers_ensure_message_id(&self) -> PyResult<bool> {
        let mut ctx = self.inner.write();
        match ctx.with_message_parts_mut(|parts| {
            let mut headers = parts.headers_mut(HeaderLimits::default(), None);
            headers.ensure_message_id().map(|()| true)
        }) {
            Ok(Some(Ok(updated))) => Ok(updated),
            Ok(Some(Err(err))) => Err(Self::map_header_error(err)),
            Ok(None) => Ok(false),
            Err(err) => Err(Self::map_parse_error(&err)),
        }
    }

    /// Set Content-Type header.
    fn headers_set_content_type(&self, value: &str) -> PyResult<bool> {
        self.headers_set("Content-Type", value)
    }

    /// Set Content-Transfer-Encoding header.
    fn headers_set_transfer_encoding(&self, value: &str) -> PyResult<bool> {
        self.headers_set("Content-Transfer-Encoding", value)
    }

    /// Get the body bytes.
    fn body_bytes(&self) -> PyResult<Vec<u8>> {
        let result = self
            .inner
            .write()
            .with_message_parts_mut(|parts| Ok(parts.body_view().as_bytes().to_vec()));
        match result {
            Ok(Some(Ok(bytes))) => Ok(bytes),
            Ok(Some(Err(err))) => Err(Self::map_body_error(err)),
            Ok(None) => Ok(Vec::new()),
            Err(err) => Err(Self::map_parse_error(&err)),
        }
    }

    /// Get detected newline style ("crlf" | "lf" | "mixed").
    fn body_newline_style(&self) -> PyResult<String> {
        let result = self
            .inner
            .write()
            .with_message_parts_mut(|parts| Ok(parts.body_view().newline_style()));
        let style = match result {
            Ok(Some(Ok(style))) => style,
            Ok(Some(Err(err))) => return Err(Self::map_body_error(err)),
            Ok(None) => NewlineStyle::Lf,
            Err(err) => return Err(Self::map_parse_error(&err)),
        };
        let name = match style {
            NewlineStyle::Crlf => "crlf",
            NewlineStyle::Lf => "lf",
            NewlineStyle::Mixed => "mixed",
        };
        Ok(name.to_string())
    }

    /// Replace the body with raw bytes.
    fn body_set_bytes(&self, data: &[u8]) -> PyResult<bool> {
        match self.with_body_mut(|body| body.set_bytes(data, BodyOpts::default())) {
            Ok(Some(Ok(()))) => Ok(true),
            Ok(Some(Err(err))) => Err(Self::map_body_error(err)),
            Ok(None) => Ok(false),
            Err(err) => Err(err),
        }
    }

    /// Append raw bytes to the body.
    fn body_append_bytes(&self, data: &[u8]) -> PyResult<bool> {
        match self.with_body_mut(|body| body.append_bytes(data, BodyOpts::default())) {
            Ok(Some(Ok(()))) => Ok(true),
            Ok(Some(Err(err))) => Err(Self::map_body_error(err)),
            Ok(None) => Ok(false),
            Err(err) => Err(err),
        }
    }

    /// Replace the body with text using the provided charset.
    fn body_set_text(&self, text: &str, charset: &str) -> PyResult<bool> {
        let charset = Self::parse_charset(charset);
        match self.with_body_mut(|body| body.set_text(text, &charset, BodyOpts::default())) {
            Ok(Some(Ok(()))) => Ok(true),
            Ok(Some(Err(err))) => Err(Self::map_body_error(err)),
            Ok(None) => Ok(false),
            Err(err) => Err(err),
        }
    }

    /// Append text to the body using the provided charset.
    fn body_append_text(&self, text: &str, charset: &str) -> PyResult<bool> {
        let charset = Self::parse_charset(charset);
        match self.with_body_mut(|body| body.append_text(text, &charset, BodyOpts::default())) {
            Ok(Some(Ok(()))) => Ok(true),
            Ok(Some(Err(err))) => Err(Self::map_body_error(err)),
            Ok(None) => Ok(false),
            Err(err) => Err(err),
        }
    }

    /// Replace the body with HTML using the provided charset.
    fn body_set_html(&self, html: &str, charset: &str) -> PyResult<bool> {
        let charset = Self::parse_charset(charset);
        match self.with_body_mut(|body| body.set_html(html, &charset, BodyOpts::default())) {
            Ok(Some(Ok(()))) => Ok(true),
            Ok(Some(Err(err))) => Err(Self::map_body_error(err)),
            Ok(None) => Ok(false),
            Err(err) => Err(err),
        }
    }

    /// Append HTML to the body using the provided charset.
    fn body_append_html(&self, html: &str, charset: &str) -> PyResult<bool> {
        let charset = Self::parse_charset(charset);
        match self.with_body_mut(|body| body.append_html(html, &charset, BodyOpts::default())) {
            Ok(Some(Ok(()))) => Ok(true),
            Ok(Some(Err(err))) => Err(Self::map_body_error(err)),
            Ok(None) => Ok(false),
            Err(err) => Err(err),
        }
    }

    /// Replace the body with multipart/alternative content.
    fn body_set_alternative(&self, text: &str, html: &str, charset: &str) -> PyResult<bool> {
        let charset = Self::parse_charset(charset);
        match self
            .with_body_mut(|body| body.set_alternative(text, html, &charset, BodyOpts::default()))
        {
            Ok(Some(Ok(()))) => Ok(true),
            Ok(Some(Err(err))) => Err(Self::map_body_error(err)),
            Ok(None) => Ok(false),
            Err(err) => Err(err),
        }
    }

    // ===== Client Information =====

    /// Get the client identifier (from HELO/EHLO)
    #[getter]
    fn client_id(&self) -> String {
        self.inner.read().id.clone()
    }

    /// Check if ESMTP extensions are enabled
    #[getter]
    fn extended(&self) -> bool {
        self.inner.read().extended
    }

    // ===== Session Context =====

    /// Get a session context value
    ///
    /// Session context persists across RSET and is NOT persisted to spool.
    /// Used for connection-level data like TLS state.
    fn session_get(&self, key: &str) -> Option<String> {
        self.inner.read().session.get(key).cloned()
    }

    /// Set a session context value
    ///
    /// Session context persists across RSET and is NOT persisted to spool.
    fn session_set(&self, key: &str, value: &str) {
        self.inner
            .write()
            .session
            .insert(Cow::Owned(key.to_string()), value.to_string());
    }

    /// Check if a session context key exists
    fn session_exists(&self, key: &str) -> bool {
        self.inner.read().session.contains_key(key)
    }

    // ===== Transaction Context =====

    /// Get a transaction context value
    ///
    /// Transaction context is cleared on RSET and IS persisted to spool.
    /// Used for message-specific data like spam scores.
    fn transaction_get(&self, key: &str) -> Option<String> {
        self.inner.read().transaction.get(key).cloned()
    }

    /// Set a transaction context value
    ///
    /// Transaction context is cleared on RSET and IS persisted to spool.
    fn transaction_set(&self, key: &str, value: &str) {
        self.inner
            .write()
            .transaction
            .insert(Cow::Owned(key.to_string()), value.to_string());
    }

    /// Check if a transaction context key exists
    fn transaction_exists(&self, key: &str) -> bool {
        self.inner.read().transaction.contains_key(key)
    }

    // ===== TLS Information =====

    /// Check if the connection is using TLS
    #[getter]
    fn is_tls(&self) -> bool {
        self.inner
            .read()
            .session
            .get("tls")
            .is_some_and(|v| v == "true")
    }

    /// Get TLS protocol version (e.g., "TLSv1.3")
    #[getter]
    fn tls_protocol(&self) -> Option<String> {
        self.inner.read().session.get("protocol").cloned()
    }

    /// Get TLS cipher suite
    #[getter]
    fn tls_cipher(&self) -> Option<String> {
        self.inner.read().session.get("cipher").cloned()
    }

    // ===== Rejection Interface =====

    /// Reject the current operation with an SMTP code and message
    ///
    /// # Arguments
    /// * `code` - SMTP status code (e.g., 550)
    /// * `message` - Human-readable rejection message
    ///
    /// # Errors
    /// Returns error if code is invalid
    fn reject(&self, code: u16, message: &str) -> PyResult<()> {
        use empath_common::status::Status;

        if !(400..=599).contains(&code) {
            return Err(PyValueError::new_err(format!(
                "Invalid SMTP code {code}, must be 400-599"
            )));
        }

        let status = Status::from(u32::from(code));
        self.inner.write().response = Some((status, Cow::Owned(message.to_string())));
        Ok(())
    }

    /// Check if this context has been rejected
    #[getter]
    fn is_rejected(&self) -> bool {
        self.inner
            .read()
            .response
            .as_ref()
            .is_some_and(|(status, _)| u32::from(*status) >= 400)
    }

    /// Get rejection details (code, message) if rejected
    #[getter]
    fn rejection(&self) -> Option<(u16, String)> {
        self.inner
            .read()
            .response
            .as_ref()
            .and_then(|(status, message)| {
                let code = u32::from(*status);
                if code >= 400 {
                    u16::try_from(code)
                        .ok()
                        .map(|code| (code, message.to_string()))
                } else {
                    None
                }
            })
    }

    // ===== String Representation =====

    fn __repr__(&self) -> String {
        let (sender, recipient_count, extended) = {
            let ctx = self.inner.read();
            let sender = ctx
                .envelope
                .sender()
                .map_or_else(|| "None".to_string(), ToString::to_string);
            let recipient_count = ctx
                .envelope
                .recipients()
                .map_or(0, |recipients| recipients.len());
            let extended = ctx.extended;
            drop(ctx);
            (sender, recipient_count, extended)
        };

        format!("Context(sender={sender}, recipients={recipient_count}, extended={extended})")
    }
}
