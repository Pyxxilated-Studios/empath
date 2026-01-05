//! Shared delivery error types for outbound SMTP transactions.

use thiserror::Error;

use crate::client::ClientError;

/// Top-level delivery error type.
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

    /// DNSSEC validation failed for a lookup.
    #[error("DNSSEC validation failed: {0}")]
    DnssecValidationFailed(String),
}

/// Temporary errors that should be retried with exponential backoff.
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

    /// Delivery policy violation (MTA-STS, TLS policy, etc.).
    #[error("Policy violation: {0}")]
    PolicyViolation(String),
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

impl From<ClientError> for DeliveryError {
    fn from(error: ClientError) -> Self {
        match error {
            ClientError::SmtpError { code, message } if (400..500).contains(&code) => {
                Self::Temporary(TemporaryError::SmtpTemporary(format!("{code} {message}")))
            }
            ClientError::SmtpError { code, message } if (500..600).contains(&code) => {
                Self::Permanent(PermanentError::MessageRejected(format!("{code} {message}")))
            }
            ClientError::SmtpError { code, message }
            | ClientError::UnexpectedResponse { code, message } => Self::System(
                SystemError::Internal(format!("Unexpected SMTP response: {code} {message}")),
            ),
            ClientError::Io(e) => {
                Self::Temporary(TemporaryError::ConnectionFailed(format!("I/O error: {e}")))
            }
            ClientError::ConnectionClosed => Self::Temporary(TemporaryError::ConnectionFailed(
                "Connection closed unexpectedly".to_string(),
            )),
            ClientError::TlsError(msg) => Self::Temporary(TemporaryError::TlsHandshakeFailed(msg)),
            ClientError::ParseError(msg) => Self::System(SystemError::Internal(format!(
                "SMTP protocol parse error: {msg}"
            ))),
            ClientError::BuilderError(msg) => Self::System(SystemError::Configuration(format!(
                "SMTP client config error: {msg}"
            ))),
            ClientError::Utf8Error(e) => {
                Self::System(SystemError::Internal(format!("UTF-8 decoding error: {e}")))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_temporary_with_message(error: DeliveryError, needle: &str) {
        match error {
            DeliveryError::Temporary(TemporaryError::SmtpTemporary(msg))
            | DeliveryError::Temporary(TemporaryError::ConnectionFailed(msg))
            | DeliveryError::Temporary(TemporaryError::TlsHandshakeFailed(msg)) => {
                assert!(
                    msg.contains(needle),
                    "expected temporary error message to contain '{needle}', got '{msg}'"
                );
            }
            other => panic!("expected temporary error, got {other:?}"),
        }
    }

    fn assert_system_with_message(error: DeliveryError, needle: &str) {
        match error {
            DeliveryError::System(SystemError::Internal(msg))
            | DeliveryError::System(SystemError::Configuration(msg)) => {
                assert!(
                    msg.contains(needle),
                    "expected system error message to contain '{needle}', got '{msg}'"
                );
            }
            other => panic!("expected system error, got {other:?}"),
        }
    }

    #[test]
    fn smtp_4xx_maps_to_temporary_smtp_error() {
        let error = DeliveryError::from(ClientError::SmtpError {
            code: 421,
            message: "Service not available".to_string(),
        });

        assert_temporary_with_message(error, "421");
    }

    #[test]
    fn smtp_5xx_maps_to_permanent_message_rejected() {
        let error = DeliveryError::from(ClientError::SmtpError {
            code: 550,
            message: "User unknown".to_string(),
        });

        match error {
            DeliveryError::Permanent(PermanentError::MessageRejected(msg)) => {
                assert!(
                    msg.contains("550") && msg.contains("User unknown"),
                    "expected message rejection to include SMTP context, got '{msg}'"
                );
            }
            other => panic!("expected permanent error, got {other:?}"),
        }
    }

    #[test]
    fn unexpected_response_maps_to_system_internal() {
        let error = DeliveryError::from(ClientError::UnexpectedResponse {
            code: 250,
            message: "OK".to_string(),
        });

        assert_system_with_message(error, "Unexpected SMTP response");
    }

    #[test]
    fn io_error_maps_to_temporary_connection_failure() {
        let error = DeliveryError::from(ClientError::Io(std::io::Error::new(
            std::io::ErrorKind::ConnectionRefused,
            "refused",
        )));

        assert_temporary_with_message(error, "I/O error");
    }

    #[test]
    fn connection_closed_maps_to_temporary_connection_failure() {
        let error = DeliveryError::from(ClientError::ConnectionClosed);

        assert_temporary_with_message(error, "Connection closed unexpectedly");
    }

    #[test]
    fn tls_error_maps_to_temporary_tls_handshake_failure() {
        let error = DeliveryError::from(ClientError::TlsError("handshake".to_string()));

        assert_temporary_with_message(error, "handshake");
    }

    #[test]
    fn parse_error_maps_to_system_internal() {
        let error = DeliveryError::from(ClientError::ParseError("bad response".to_string()));

        assert_system_with_message(error, "SMTP protocol parse error");
    }

    #[test]
    fn builder_error_maps_to_system_configuration() {
        let error = DeliveryError::from(ClientError::BuilderError("bad config".to_string()));

        assert_system_with_message(error, "SMTP client config error");
    }

    #[test]
    fn utf8_error_maps_to_system_internal() {
        let bytes = vec![0xF0, 0x28, 0x8C, 0x28];
        let bad_utf8 = std::str::from_utf8(&bytes).unwrap_err();
        let error = DeliveryError::from(ClientError::Utf8Error(bad_utf8));

        assert_system_with_message(error, "UTF-8 decoding error");
    }
}
