//! Centralized context key constants.
//!
//! This module provides public constants for all context keys used across
//! the Empath codebase. Using these constants instead of raw string literals
//! provides compile-time safety when keys change.
//!
//! # Organization
//!
//! Keys are organized into submodules by category:
//! - [`session`]: Session-scoped keys (TLS state, authentication)
//! - [`transaction`]: Transaction-scoped keys (SPF, DMARC, delivery)
//! - [`otel`]: OpenTelemetry trace correlation keys
//!
//! # Example
//!
//! ```ignore
//! use empath_common::context_keys::{session, transaction};
//!
//! // Use typed keys instead of string literals
//! ctx.session_set(session::TLS, "true");
//! ctx.transaction_set(transaction::SPF_RESULT, "pass");
//! ```

/// Session-scoped context keys.
///
/// These keys persist for the lifetime of a connection but are NOT
/// persisted to the spool. They are cleared on disconnect.
pub mod session {
    /// TLS enabled flag ("true" or "false").
    pub const TLS: &str = "tls";
    /// TLS protocol version (e.g., "TLSv1.3").
    pub const TLS_PROTOCOL: &str = "protocol";
    /// TLS cipher suite name.
    pub const TLS_CIPHER: &str = "cipher";

    /// Authentication enabled flag ("true" or "false").
    pub const AUTH: &str = "auth";
    /// Authentication mechanism used (e.g., "PLAIN", "LOGIN").
    pub const AUTH_MECHANISM: &str = "auth_mechanism";
    /// Authenticated username.
    pub const AUTH_USER: &str = "auth_user";
    /// Authorization identity (if different from auth user).
    pub const AUTHZID: &str = "authzid";
    /// Authentication password (sensitive - handle with care).
    pub const AUTH_PASSWORD: &str = "auth_password";
}

/// Transaction-scoped context keys.
///
/// These keys are cleared on RSET and ARE persisted to the spool.
/// They travel with the message through its lifecycle.
pub mod transaction {
    /// Outbound binding name for delivery routing.
    pub const OUTBOUND_BINDING: &str = "outbound_binding";
    /// Delivery priority tier ("high", "normal", "low").
    pub const PRIORITY_TIER: &str = "priority_tier";

    // SPF validation results
    /// SPF result code ("pass", "fail", "softfail", "neutral", "none", "temperror", "permerror").
    pub const SPF_RESULT: &str = "spf_result";
    /// SPF domain checked.
    pub const SPF_DOMAIN: &str = "spf_domain";
    /// SPF HELO domain checked.
    pub const SPF_HELO_DOMAIN: &str = "spf_helo_domain";
    /// SPF explanation string (if provided by sender's policy).
    pub const SPF_EXPLANATION: &str = "spf_explanation";

    // DMARC validation results
    /// DMARC domain (organizational domain).
    pub const DMARC_DOMAIN: &str = "dmarc_domain";
    /// DMARC result code ("pass", "fail", "none").
    pub const DMARC_RESULT: &str = "dmarc_result";
    /// DMARC policy ("none", "quarantine", "reject").
    pub const DMARC_POLICY: &str = "dmarc_policy";
    /// Whether SPF is aligned with DMARC domain ("true" or "false").
    pub const DMARC_SPF_ALIGNED: &str = "dmarc_spf_aligned";
    /// Whether DKIM is aligned with DMARC domain ("true" or "false").
    pub const DMARC_DKIM_ALIGNED: &str = "dmarc_dkim_aligned";
    /// DMARC error message if evaluation failed.
    pub const DMARC_ERROR: &str = "dmarc_error";
    /// DMARC failure reason.
    pub const DMARC_REASON: &str = "dmarc_reason";
    /// DMARC action taken ("none", "quarantine", "reject").
    pub const DMARC_ACTION: &str = "dmarc_action";
    /// Whether this message was sampled for DMARC reporting ("true" or "false").
    pub const DMARC_SAMPLED: &str = "dmarc_sampled";
    /// DMARC sample rate in basis points (0-10000).
    pub const DMARC_SAMPLE_RATE_BPS: &str = "dmarc_sample_rate_bps";
}

/// OpenTelemetry trace correlation keys.
///
/// These keys are used to correlate logs and traces across
/// the message lifecycle.
pub mod otel {
    /// OpenTelemetry trace ID for distributed tracing.
    pub const TRACE_ID: &str = "trace_id";
    /// OpenTelemetry span ID for distributed tracing.
    pub const SPAN_ID: &str = "span_id";
}
