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

use crate::{
    Domain,
    email::{EmailAddress, ServerId},
    message_api::{MutationKind, MutationRecord},
};

/// Audit logging configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditConfig {
    /// Enable audit logging for message lifecycle events
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Redaction settings for audit logs
    #[serde(flatten)]
    pub redaction: AuditRedaction,
}

/// Redaction configuration for audit logs
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditRedaction {
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
            redaction: AuditRedaction::default(),
        }
    }
}

impl Default for AuditRedaction {
    fn default() -> Self {
        Self {
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
pub fn redact_email(email: &EmailAddress, redact: bool) -> String {
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
pub fn redact_emails(emails: &[EmailAddress], redact: bool) -> Vec<String> {
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
    sender: Option<&EmailAddress>,
    recipients: &[EmailAddress],
    size: usize,
    from_ip: &str,
) {
    let config = config();
    if !config.enabled {
        return;
    }

    let redacted_sender = sender.map_or_else(String::new, |s| {
        redact_email(s, config.redaction.redact_sender)
    });
    let redacted_recipients = redact_emails(recipients, config.redaction.redact_recipients);

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

/// Log module mutation event
///
/// Logged when a module mutates headers or body content.
///
/// # Fields
/// - `message_id`: Unique message identifier
/// - `module_type`: Module runtime type (e.g., wasm, python, `shared_library`)
/// - `module_name`: Module name or path
/// - `mutation_kind`: Header or body mutation
/// - `action`: Mutation action (set/append/remove/etc.)
/// - `header_name`: Header name (header mutations only)
/// - `old_len`: Previous size (bytes)
/// - `new_len`: New size (bytes)
pub fn log_module_mutation(
    message_id: &str,
    module_type: &str,
    module_name: &str,
    record: &MutationRecord,
) {
    let config = config();
    if !config.enabled {
        return;
    }

    let (mutation_kind, header_name) = match &record.kind {
        MutationKind::Header { name } => ("header", Some(name.as_str())),
        MutationKind::Body => ("body", None),
    };

    tracing::event!(
        tracing::Level::INFO,
        event = "ModuleMutation",
        message_id = %message_id,
        module_type = %module_type,
        module_name = %module_name,
        mutation_kind = %mutation_kind,
        action = %record.action,
        header_name = ?header_name,
        old_len = record.old_len,
        new_len = record.new_len,
        "Audit: Module mutation"
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
pub fn log_delivery_attempt(message_id: &str, domain: &Domain, server: &ServerId, attempt: usize) {
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
    domain: &Domain,
    server: &ServerId,
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
    domain: &Domain,
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
    use crate::email::EmailValidationError;

    #[test]
    fn test_redact_email() -> Result<(), EmailValidationError> {
        let email = EmailAddress::new("user@example.com")?;
        assert_eq!(redact_email(&email, true), "[REDACTED]@example.com");
        assert_eq!(redact_email(&email, false), "user@example.com");
        Ok(())
    }

    #[test]
    fn test_redact_emails() -> Result<(), EmailValidationError> {
        let email_one = EmailAddress::new("user1@example.com")?;
        let email_two = EmailAddress::new("user2@example.org")?;
        let emails = vec![email_one, email_two];

        let redacted = redact_emails(&emails, true);
        assert_eq!(redacted[0], "[REDACTED]@example.com");
        assert_eq!(redacted[1], "[REDACTED]@example.org");

        let not_redacted = redact_emails(&emails, false);
        assert_eq!(not_redacted[0], "user1@example.com");
        assert_eq!(not_redacted[1], "user2@example.org");
        Ok(())
    }

    #[test]
    fn test_default_config() {
        let config = AuditConfig::default();
        assert!(config.enabled);
        assert!(!config.redaction.redact_sender);
        assert!(!config.redaction.redact_recipients);
        assert!(config.redaction.redact_message_content);
    }

    #[test]
    fn test_audit_disabled() -> Result<(), EmailValidationError> {
        // Initialize with disabled config
        init(AuditConfig {
            enabled: false,
            redaction: AuditRedaction {
                redact_sender: false,
                redact_recipients: false,
                redact_message_content: false,
            },
        });

        let sender = EmailAddress::new("sender@example.com")?;
        let recipient = EmailAddress::new("rcpt@example.com")?;
        let recipients = vec![recipient];
        let domain = Domain::new("example.com");
        let server = ServerId::new("mx1.example.com:25")?;

        // These should not panic even when disabled
        log_message_received("test-id", Some(&sender), &recipients, 1024, "192.168.1.1");
        log_delivery_attempt("test-id", &domain, &server, 1);
        log_delivery_success("test-id", &domain, &server, 1, 1000);
        log_delivery_failure("test-id", &domain, "connection refused", 1, "Failed");
        Ok(())
    }
}
