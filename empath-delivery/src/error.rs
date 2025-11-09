//! Typed error handling for delivery operations.
//!
//! This module provides structured error types that distinguish between:
//! - Permanent failures (5xx SMTP codes) - don't retry
//! - Temporary failures (4xx SMTP codes) - retry with backoff
//! - System errors - internal errors

use thiserror::Error;

use crate::DnsError;

/// Top-level delivery error type.
///
/// This error type provides clear categorization of failures to enable
/// appropriate retry logic and error reporting.
#[derive(Debug, Error)]
pub enum DeliveryError {
    /// Permanent failure that should not be retried (e.g., 5xx SMTP codes).
    #[error("Permanent failure: {0}")]
    Permanent(#[from] PermanentError),

    /// Temporary failure that can be retried with backoff (e.g., 4xx SMTP codes).
    #[error("Temporary failure: {0}")]
    Temporary(#[from] TemporaryError),

    /// System-level error (I/O, internal errors, etc.).
    #[error("System error: {0}")]
    System(#[from] SystemError),
}

/// Permanent errors that should not be retried.
///
/// These typically correspond to 5xx SMTP response codes or unrecoverable
/// configuration issues.
#[derive(Debug, Error)]
pub enum PermanentError {
    /// Recipient address is invalid or rejected by the server.
    #[error("Invalid recipient: {0}")]
    InvalidRecipient(String),

    /// Domain does not exist or has no mail servers.
    #[error("Domain not found: {0}")]
    DomainNotFound(String),

    /// Message was rejected by the server (e.g., policy violation, spam).
    #[error("Message rejected: {0}")]
    MessageRejected(String),

    /// No mail servers found for the domain (no MX, A, or AAAA records).
    #[error("No mail servers available for domain: {0}")]
    NoMailServers(String),

    /// SMTP authentication failed.
    #[error("Authentication failed: {0}")]
    AuthenticationFailed(String),

    /// Message size exceeds server limits.
    #[error("Message too large: {0}")]
    MessageTooLarge(String),
}

/// Temporary errors that should be retried with exponential backoff.
///
/// These typically correspond to 4xx SMTP response codes or transient
/// network issues.
#[derive(Debug, Error)]
pub enum TemporaryError {
    /// Failed to establish connection to the mail server.
    #[error("Connection failed: {0}")]
    ConnectionFailed(String),

    /// Server is temporarily busy or unavailable.
    #[error("Server busy: {0}")]
    ServerBusy(String),

    /// Rate limit exceeded.
    #[error("Rate limited: {0}")]
    RateLimited(String),

    /// DNS lookup failed (temporary network issue).
    #[error("DNS lookup failed: {0}")]
    DnsLookupFailed(String),

    /// Connection timed out.
    #[error("Connection timed out: {0}")]
    Timeout(String),

    /// Server returned a temporary failure code.
    #[error("Temporary SMTP error: {0}")]
    SmtpTemporary(String),

    /// TLS handshake failed.
    #[error("TLS handshake failed: {0}")]
    TlsHandshakeFailed(String),
}

/// System-level errors that indicate internal problems.
#[derive(Debug, Error)]
pub enum SystemError {
    /// Failed to read message from spool.
    #[error("Spool read error: {0}")]
    SpoolRead(String),

    /// Failed to write to spool.
    #[error("Spool write error: {0}")]
    SpoolWrite(String),

    /// Delivery processor not initialized.
    #[error("Delivery processor not initialized: {0}")]
    NotInitialized(String),

    /// Message not found in queue.
    #[error("Message not found in queue: {0}")]
    MessageNotFound(String),

    /// Invalid configuration.
    #[error("Configuration error: {0}")]
    Configuration(String),

    /// I/O error.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Other internal errors.
    #[error("Internal error: {0}")]
    Internal(String),
}

impl DeliveryError {
    /// Returns `true` if this error is temporary and should be retried.
    #[must_use]
    pub const fn is_temporary(&self) -> bool {
        matches!(self, Self::Temporary(_))
    }

    /// Returns `true` if this error is permanent and should not be retried.
    #[must_use]
    pub const fn is_permanent(&self) -> bool {
        matches!(self, Self::Permanent(_))
    }

    /// Returns `true` if this is a system error.
    #[must_use]
    pub const fn is_system(&self) -> bool {
        matches!(self, Self::System(_))
    }
}

/// Convert from `DnsError` to `DeliveryError`.
///
/// DNS errors are categorized as either permanent or temporary based on
/// whether they are likely to succeed on retry.
impl From<DnsError> for DeliveryError {
    fn from(error: DnsError) -> Self {
        match error {
            DnsError::NoMailServers(domain) => {
                Self::Permanent(PermanentError::NoMailServers(domain))
            }
            DnsError::DomainNotFound(domain) => {
                Self::Permanent(PermanentError::DomainNotFound(domain))
            }
            DnsError::Timeout(msg) => Self::Temporary(TemporaryError::Timeout(msg)),
            DnsError::LookupFailed(err) => {
                Self::Temporary(TemporaryError::DnsLookupFailed(err.to_string()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_delivery_error_is_temporary() {
        let error = DeliveryError::Temporary(TemporaryError::ConnectionFailed(
            "Connection refused".to_string(),
        ));
        assert!(error.is_temporary());
        assert!(!error.is_permanent());
        assert!(!error.is_system());
    }

    #[test]
    fn test_delivery_error_is_permanent() {
        let error = DeliveryError::Permanent(PermanentError::InvalidRecipient(
            "user@example.com".to_string(),
        ));
        assert!(!error.is_temporary());
        assert!(error.is_permanent());
        assert!(!error.is_system());
    }

    #[test]
    fn test_delivery_error_is_system() {
        let error = DeliveryError::System(SystemError::Internal("Internal error".to_string()));
        assert!(!error.is_temporary());
        assert!(!error.is_permanent());
        assert!(error.is_system());
    }

    #[test]
    fn test_dns_error_conversion() {
        // Test NoMailServers -> Permanent
        let dns_err = DnsError::NoMailServers("example.com".to_string());
        let delivery_err: DeliveryError = dns_err.into();
        assert!(delivery_err.is_permanent());

        // Test DomainNotFound -> Permanent
        let dns_err = DnsError::DomainNotFound("example.com".to_string());
        let delivery_err: DeliveryError = dns_err.into();
        assert!(delivery_err.is_permanent());

        // Test Timeout -> Temporary
        let dns_err = DnsError::Timeout("example.com".to_string());
        let delivery_err: DeliveryError = dns_err.into();
        assert!(delivery_err.is_temporary());
    }

    #[test]
    fn test_error_display() {
        let error = DeliveryError::Temporary(TemporaryError::ServerBusy(
            "Server temporarily unavailable".to_string(),
        ));
        assert_eq!(
            error.to_string(),
            "Temporary failure: Server busy: Server temporarily unavailable"
        );

        let error = DeliveryError::Permanent(PermanentError::InvalidRecipient(
            "user@example.com".to_string(),
        ));
        assert_eq!(
            error.to_string(),
            "Permanent failure: Invalid recipient: user@example.com"
        );
    }
}
