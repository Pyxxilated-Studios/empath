//! Configuration types for the Empath MTA.
//!
//! This module provides unified, consolidated configuration types that replace
//! the previously scattered configuration structures across different crates.
//!
//! ## Design Goals
//!
//! 1. **Consistency**: Common patterns for timeouts, TLS, and domain settings
//! 2. **Clarity**: Clear separation between server and client contexts
//! 3. **Backward Compatibility**: Existing configs remain functional during migration
//! 4. **Type Safety**: Strongly typed policies instead of boolean flags
//!
//! ## Modules
//!
//! - [`timeouts`]: Unified timeout configuration for SMTP operations
//! - [`tls`]: TLS policy and certificate validation settings

pub mod timeouts;
pub mod tls;

// Re-export commonly used types
pub use timeouts::{ClientTimeouts, ServerTimeouts, TimeoutConfig};
pub use tls::{TlsCertificatePolicy, TlsConfig, TlsPolicy};
