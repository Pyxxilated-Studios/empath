//! Pythonic API for Context access
//!
//! This module provides a `#[pyclass]` wrapper around `empath_common::Context`
//! that exposes a natural Python API with properties and methods.

use std::{borrow::Cow, sync::Arc};

use empath_common::context::Context;
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
