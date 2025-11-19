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

    /// TLS is required but not available or failed.
    #[error("TLS required: {0}")]
    TlsRequired(String),
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

/// Convert from SMTP `ClientError` to `DeliveryError`.
///
/// This automatic conversion eliminates the need for manual `.map_err()` calls
/// throughout the delivery codebase. Errors are intelligently categorized based
/// on SMTP response codes and error types:
///
/// - **4xx SMTP codes** → Temporary (should retry)
/// - **5xx SMTP codes** → Permanent (should not retry)
/// - **Connection/I/O errors** → Temporary (network issues are transient)
/// - **TLS errors** → Temporary (may succeed with different config)
/// - **Parse/Config errors** → System (internal issues)
///
/// # Examples
///
/// ```ignore
/// // Before (manual conversion):
/// client.connect().await.map_err(|e| {
///     TemporaryError::ConnectionFailed(format!("Failed to connect: {e}"))
/// })?;
///
/// // After (automatic conversion):
/// client.connect().await?;
/// ```
impl From<empath_smtp::client::ClientError> for DeliveryError {
    fn from(error: empath_smtp::client::ClientError) -> Self {
        use empath_smtp::client::ClientError;

        match error {
            // 4xx SMTP codes are temporary failures - retry with backoff
            ClientError::SmtpError { code, message } if (400..500).contains(&code) => {
                Self::Temporary(TemporaryError::SmtpTemporary(format!("{code} {message}")))
            }

            // 5xx SMTP codes are permanent failures - do not retry
            ClientError::SmtpError { code, message } if (500..600).contains(&code) => {
                Self::Permanent(PermanentError::MessageRejected(format!("{code} {message}")))
            }

            // Unexpected response codes (not in standard ranges)
            ClientError::SmtpError { code, message }
            | ClientError::UnexpectedResponse { code, message } => Self::System(
                SystemError::Internal(format!("Unexpected SMTP response: {code} {message}")),
            ),

            // I/O errors are temporary (connection refused, timeout, etc.)
            ClientError::Io(e) => {
                Self::Temporary(TemporaryError::ConnectionFailed(format!("I/O error: {e}")))
            }

            // Connection closed unexpectedly - temporary, retry may succeed
            ClientError::ConnectionClosed => Self::Temporary(TemporaryError::ConnectionFailed(
                "Connection closed unexpectedly".to_string(),
            )),

            // TLS errors are temporary - might succeed with different config or retry
            ClientError::TlsError(msg) => Self::Temporary(TemporaryError::TlsHandshakeFailed(msg)),

            // Parse errors indicate protocol violation or internal bugs - system error
            ClientError::ParseError(msg) => Self::System(SystemError::Internal(format!(
                "SMTP protocol parse error: {msg}"
            ))),

            // Builder/configuration errors are system errors
            ClientError::BuilderError(msg) => Self::System(SystemError::Configuration(format!(
                "SMTP client config error: {msg}"
            ))),

            // UTF-8 errors indicate data corruption or protocol issues
            ClientError::Utf8Error(e) => {
                Self::System(SystemError::Internal(format!("UTF-8 decoding error: {e}")))
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

    #[test]
    fn test_client_error_conversion_4xx() {
        use empath_smtp::client::ClientError;

        // 4xx SMTP codes should be temporary
        let client_err = ClientError::SmtpError {
            code: 421,
            message: "Service not available".to_string(),
        };
        let delivery_err: DeliveryError = client_err.into();
        assert!(delivery_err.is_temporary());
        assert!(!delivery_err.is_permanent());
        assert!(!delivery_err.is_system());
        assert_eq!(
            delivery_err.to_string(),
            "Temporary failure: Temporary SMTP error: 421 Service not available"
        );
    }

    #[test]
    fn test_client_error_conversion_5xx() {
        use empath_smtp::client::ClientError;

        // 5xx SMTP codes should be permanent
        let client_err = ClientError::SmtpError {
            code: 550,
            message: "User not found".to_string(),
        };
        let delivery_err: DeliveryError = client_err.into();
        assert!(delivery_err.is_permanent());
        assert!(!delivery_err.is_temporary());
        assert!(!delivery_err.is_system());
        assert_eq!(
            delivery_err.to_string(),
            "Permanent failure: Message rejected: 550 User not found"
        );
    }

    #[test]
    fn test_client_error_conversion_io() {
        use empath_smtp::client::ClientError;

        // I/O errors should be temporary
        let client_err = ClientError::Io(std::io::Error::new(
            std::io::ErrorKind::ConnectionRefused,
            "connection refused",
        ));
        let delivery_err: DeliveryError = client_err.into();
        assert!(delivery_err.is_temporary());
        assert!(!delivery_err.is_permanent());
        assert!(!delivery_err.is_system());
    }

    #[test]
    fn test_client_error_conversion_connection_closed() {
        use empath_smtp::client::ClientError;

        // Connection closed should be temporary
        let client_err = ClientError::ConnectionClosed;
        let delivery_err: DeliveryError = client_err.into();
        assert!(delivery_err.is_temporary());
        assert_eq!(
            delivery_err.to_string(),
            "Temporary failure: Connection failed: Connection closed unexpectedly"
        );
    }

    #[test]
    fn test_client_error_conversion_tls() {
        use empath_smtp::client::ClientError;

        // TLS errors should be temporary
        let client_err = ClientError::TlsError("Handshake failed".to_string());
        let delivery_err: DeliveryError = client_err.into();
        assert!(delivery_err.is_temporary());
        assert_eq!(
            delivery_err.to_string(),
            "Temporary failure: TLS handshake failed: Handshake failed"
        );
    }

    #[test]
    fn test_client_error_conversion_parse() {
        use empath_smtp::client::ClientError;

        // Parse errors should be system errors
        let client_err = ClientError::ParseError("Invalid response".to_string());
        let delivery_err: DeliveryError = client_err.into();
        assert!(delivery_err.is_system());
        assert!(!delivery_err.is_temporary());
        assert!(!delivery_err.is_permanent());
    }

    #[test]
    fn test_client_error_conversion_builder() {
        use empath_smtp::client::ClientError;

        // Builder errors should be system errors
        let client_err = ClientError::BuilderError("Invalid config".to_string());
        let delivery_err: DeliveryError = client_err.into();
        assert!(delivery_err.is_system());
    }

    #[test]
    fn test_client_error_conversion_unexpected_code() {
        use empath_smtp::client::ClientError;

        // Unexpected response codes (outside 4xx/5xx) should be system errors
        let client_err = ClientError::UnexpectedResponse {
            code: 999,
            message: "Unknown code".to_string(),
        };
        let delivery_err: DeliveryError = client_err.into();
        assert!(delivery_err.is_system());
        assert_eq!(
            delivery_err.to_string(),
            "System error: Internal error: Unexpected SMTP response: 999 Unknown code"
        );
    }
}
