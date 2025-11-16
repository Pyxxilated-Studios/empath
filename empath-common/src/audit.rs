//! Audit logging for message lifecycle events
//!
//! This module provides structured audit logging for compliance and security monitoring.
//! All events are logged as JSON with configurable PII redaction.
//!
//! ## Audit Events
//!
//! - `MessageReceived`: Message accepted via SMTP and spooled
//! - `DeliveryAttempt`: Delivery attempt to remote MX server
//! - `DeliverySuccess`: Successful message delivery
//! - `DeliveryFailure`: Permanent delivery failure after exhausting retries
//!
//! ## PII Redaction
//!
//! Email addresses (sender, recipients) can be redacted based on the `AuditConfig`
//! to comply with privacy regulations (GDPR, HIPAA, etc.).

use std::sync::Arc;

use serde::{Deserialize, Serialize};

/// Audit logging configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "Configuration flags are intentionally bool-heavy"
)]
pub struct AuditConfig {
    /// Enable audit logging for message lifecycle events
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Redact sender email addresses from audit logs (PII protection)
    #[serde(default)]
    pub redact_sender: bool,

    /// Redact recipient email addresses from audit logs (PII protection)
    #[serde(default)]
    pub redact_recipients: bool,

    /// Redact message content preview from audit logs (PII protection)
    #[serde(default = "default_true")]
    pub redact_message_content: bool,
}

impl Default for AuditConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            redact_sender: false,
            redact_recipients: false,
            redact_message_content: true,
        }
    }
}

const fn default_true() -> bool {
    true
}

/// Global audit configuration (thread-safe)
static AUDIT_CONFIG: std::sync::OnceLock<Arc<AuditConfig>> = std::sync::OnceLock::new();

/// Initialize audit logging with configuration
pub fn init(config: AuditConfig) {
    AUDIT_CONFIG.get_or_init(|| Arc::new(config));
}

/// Get the current audit configuration
#[must_use]
pub fn config() -> Arc<AuditConfig> {
    AUDIT_CONFIG
        .get()
        .cloned()
        .unwrap_or_else(|| Arc::new(AuditConfig::default()))
}

/// Redact email address if redaction is enabled
#[must_use]
pub fn redact_email(email: &str, redact: bool) -> String {
    if redact {
        // Keep domain but redact local part
        if let Some((_, domain)) = email.split_once('@') {
            format!("[REDACTED]@{domain}")
        } else {
            "[REDACTED]".to_string()
        }
    } else {
        email.to_string()
    }
}

/// Redact multiple email addresses
#[must_use]
pub fn redact_emails(emails: &[String], redact: bool) -> Vec<String> {
    emails.iter().map(|e| redact_email(e, redact)).collect()
}

/// Log message received event
///
/// Logged when a message is accepted via SMTP and spooled for delivery.
///
/// # Fields
/// - `message_id`: Unique message identifier (ULID)
/// - `sender`: Email sender (redacted if configured)
/// - `recipients`: Email recipients (redacted if configured)
/// - `recipient_count`: Number of recipients
/// - `size`: Message size in bytes
/// - `from_ip`: Client IP address
pub fn log_message_received(
    message_id: &str,
    sender: &str,
    recipients: &[String],
    size: usize,
    from_ip: &str,
) {
    let config = config();
    if !config.enabled {
        return;
    }

    let redacted_sender = redact_email(sender, config.redact_sender);
    let redacted_recipients = redact_emails(recipients, config.redact_recipients);

    tracing::event!(
        tracing::Level::INFO,
        event = "MessageReceived",
        message_id = %message_id,
        sender = %redacted_sender,
        recipients = ?redacted_recipients,
        recipient_count = recipients.len(),
        size = size,
        from_ip = %from_ip,
        "Audit: Message received and spooled"
    );
}

/// Log delivery attempt event
///
/// Logged for each delivery attempt to a remote MX server.
///
/// # Fields
/// - `message_id`: Unique message identifier
/// - `domain`: Recipient domain
/// - `server`: MX server address (host:port)
/// - `delivery_attempt`: Attempt number (1-based)
pub fn log_delivery_attempt(message_id: &str, domain: &str, server: &str, attempt: usize) {
    let config = config();
    if !config.enabled {
        return;
    }

    tracing::event!(
        tracing::Level::INFO,
        event = "DeliveryAttempt",
        message_id = %message_id,
        domain = %domain,
        server = %server,
        delivery_attempt = attempt,
        "Audit: Delivery attempt"
    );
}

/// Log delivery success event
///
/// Logged when a message is successfully delivered to a remote MX server.
///
/// # Fields
/// - `message_id`: Unique message identifier
/// - `domain`: Recipient domain
/// - `server`: MX server address (host:port)
/// - `delivery_attempt`: Final attempt number
/// - `duration_ms`: Total delivery duration in milliseconds
pub fn log_delivery_success(
    message_id: &str,
    domain: &str,
    server: &str,
    attempt: usize,
    duration_ms: u128,
) {
    let config = config();
    if !config.enabled {
        return;
    }

    tracing::event!(
        tracing::Level::INFO,
        event = "DeliverySuccess",
        message_id = %message_id,
        domain = %domain,
        server = %server,
        delivery_attempt = attempt,
        duration_ms = duration_ms,
        "Audit: Delivery successful"
    );
}

/// Log delivery failure event
///
/// Logged when a message permanently fails after exhausting retry attempts.
///
/// # Fields
/// - `message_id`: Unique message identifier
/// - `domain`: Recipient domain
/// - `error`: Error description
/// - `delivery_attempt`: Final attempt number
/// - `status`: Final delivery status (Failed/Retry)
pub fn log_delivery_failure(
    message_id: &str,
    domain: &str,
    error: &str,
    attempt: usize,
    status: &str,
) {
    let config = config();
    if !config.enabled {
        return;
    }

    tracing::event!(
        tracing::Level::WARN,
        event = "DeliveryFailure",
        message_id = %message_id,
        domain = %domain,
        error = %error,
        delivery_attempt = attempt,
        status = %status,
        "Audit: Delivery failed"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_redact_email() {
        assert_eq!(
            redact_email("user@example.com", true),
            "[REDACTED]@example.com"
        );
        assert_eq!(redact_email("user@example.com", false), "user@example.com");
        assert_eq!(redact_email("invalid", true), "[REDACTED]");
        assert_eq!(redact_email("invalid", false), "invalid");
    }

    #[test]
    fn test_redact_emails() {
        let emails = vec![
            "user1@example.com".to_string(),
            "user2@example.org".to_string(),
        ];

        let redacted = redact_emails(&emails, true);
        assert_eq!(redacted[0], "[REDACTED]@example.com");
        assert_eq!(redacted[1], "[REDACTED]@example.org");

        let not_redacted = redact_emails(&emails, false);
        assert_eq!(not_redacted[0], "user1@example.com");
        assert_eq!(not_redacted[1], "user2@example.org");
    }

    #[test]
    fn test_default_config() {
        let config = AuditConfig::default();
        assert!(config.enabled);
        assert!(!config.redact_sender);
        assert!(!config.redact_recipients);
        assert!(config.redact_message_content);
    }

    #[test]
    fn test_audit_disabled() {
        // Initialize with disabled config
        init(AuditConfig {
            enabled: false,
            redact_sender: false,
            redact_recipients: false,
            redact_message_content: false,
        });

        // These should not panic even when disabled
        log_message_received(
            "test-id",
            "sender@example.com",
            &["rcpt@example.com".to_string()],
            1024,
            "192.168.1.1",
        );
        log_delivery_attempt("test-id", "example.com", "mx1.example.com:25", 1);
        log_delivery_success("test-id", "example.com", "mx1.example.com:25", 1, 1000);
        log_delivery_failure("test-id", "example.com", "connection refused", 1, "Failed");
    }
}
