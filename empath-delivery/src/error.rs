//! Typed error handling for delivery operations.
//!
//! This module re-exports shared delivery error types and provides conversions
//! from delivery-specific errors.

pub use empath_smtp_client::{DeliveryError, PermanentError, SystemError, TemporaryError};

use crate::DnsError;

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
            DnsError::DnssecValidationFailed { target, status } => match status {
                crate::dns::DnssecStatus::Indeterminate => {
                    Self::Temporary(TemporaryError::DnsLookupFailed(format!(
                        "DNSSEC validation indeterminate for {target}"
                    )))
                }
                _ => Self::Permanent(PermanentError::DnssecValidationFailed(format!(
                    "{target} ({status})"
                ))),
            },
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
}
