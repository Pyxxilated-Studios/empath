//! Business logic handler for SMTP sessions.
//!
//! This module provides the `BusinessHandler` trait that separates business
//! logic (validation, spooling, module dispatch) from protocol state management
//! (FSM) and I/O orchestration.

mod email_auth;
mod handler;
mod helpers;
mod traits;
mod validation;

#[cfg(test)]
mod tests;

// Re-export public types
pub use handler::DefaultSmtpTransactionHandler;
pub use helpers::requires_validation;
pub use traits::SmtpTransactionHandler;
