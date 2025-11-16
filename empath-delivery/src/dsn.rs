//! Delivery Status Notification (DSN) generation per RFC 3464
//!
//! This module generates bounce messages (DSNs) when delivery fails permanently
//! or after max retry attempts are exhausted. DSNs inform senders that their
//! message could not be delivered.
//!
//! # DSN Structure (RFC 3464)
//! ```text
//! multipart/report; report-type="delivery-status"
//! ├── Part 1: text/plain (human-readable explanation)
//! ├── Part 2: message/delivery-status (machine-readable status)
//! └── Part 3: text/rfc822-headers (original message headers)
//! ```

use std::{fmt::Write as _, sync::Arc, time::SystemTime};

use ahash::AHashMap;
use empath_common::{
    DeliveryStatus,
    address::{Address, AddressList},
    context::Context,
    envelope::Envelope,
    tracing::info,
};
use mailparse::MailAddr;
use serde::{Deserialize, Serialize};

use crate::{DeliveryError, DeliveryInfo, error::PermanentError};

/// Configuration for DSN generation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DsnConfig {
    /// Enable/disable DSN generation globally
    pub enabled: bool,
    /// Hostname for Reporting-MTA field (FQDN of this MTA)
    pub reporting_mta: String,
    /// Postmaster email address (for errors sending DSN)
    pub postmaster: String,
}

impl Default for DsnConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            reporting_mta: "localhost".to_string(),
            postmaster: "postmaster@localhost".to_string(),
        }
    }
}

/// Check if a DSN should be generated for this delivery failure
///
/// DSNs are generated for:
/// - Permanent failures (5xx SMTP errors, invalid recipient, domain not found, etc.)
/// - Max retry attempts exhausted (temporary failures that gave up)
///
/// DSNs are NOT generated for:
/// - Temporary failures still in retry state
/// - Messages from null sender (MAIL FROM:<>) - prevents bounce loops
/// - System errors (internal errors that don't indicate delivery failure)
#[must_use]
pub fn should_generate_dsn(
    original_context: &Context,
    delivery_info: &DeliveryInfo,
    error: &DeliveryError,
) -> bool {
    // Don't generate DSN if sender is null (prevents bounce loops)
    // Null sender is indicated by MAIL FROM:<>
    if let Some(sender) = original_context.envelope.sender()
        && let MailAddr::Single(info) = &**sender
        && info.addr.is_empty()
    {
        return false;
    } else if original_context.envelope.sender().is_none() {
        // No sender address
        return false;
    }

    // Generate DSN for permanent failures
    if error.is_permanent() {
        return true;
    }

    // Generate DSN if max retry attempts exhausted
    if matches!(
        delivery_info.status,
        DeliveryStatus::Failed(_) | DeliveryStatus::Expired
    ) {
        return true;
    }

    // Don't generate for temporary failures still in retry
    false
}

/// Generate a DSN message and return a Context ready for spooling
///
/// This function creates a complete RFC 3464 compliant DSN message
/// that will be sent back to the original sender.
///
/// # Arguments
/// * `original_context` - The original message that failed delivery
/// * `delivery_info` - Information about the delivery attempt
/// * `error` - The error that caused delivery failure
/// * `config` - DSN configuration (reporting MTA, postmaster, etc.)
///
/// # Returns
/// A `Context` object ready to be spooled and delivered as a bounce message
///
/// # Errors
/// Returns an error if the DSN message cannot be constructed
pub fn generate_dsn(
    original_context: &Context,
    delivery_info: &DeliveryInfo,
    error: &DeliveryError,
    config: &DsnConfig,
) -> Result<Context, PermanentError> {
    info!(
        message_id = %delivery_info.message_id,
        domain = %delivery_info.recipient_domain,
        error = %error,
        "Generating DSN for failed delivery"
    );

    // Extract sender from original message (this will be the DSN recipient)
    let original_sender = original_context.envelope.sender().ok_or_else(|| {
        PermanentError::InvalidRecipient("No sender in original message".to_string())
    })?;

    // Extract recipient from original message (for DSN body)
    let original_recipients = original_context.envelope.recipients().ok_or_else(|| {
        PermanentError::InvalidRecipient("No recipients in original message".to_string())
    })?;

    // Build the three parts of the DSN
    let human_readable =
        build_human_readable_part(original_sender, original_recipients, delivery_info, error);

    let machine_readable =
        build_machine_readable_part(config, original_recipients, delivery_info, error);

    let original_headers = extract_original_headers(original_context);

    // Generate unique boundary for multipart message
    let boundary = format!(
        "----=_Part_{}_{}",
        ulid::Ulid::new(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_micros()
    );

    // Build the complete DSN message
    let dsn_body = format!(
        "Content-Type: multipart/report; report-type=\"delivery-status\"; boundary=\"{boundary}\"\r\n\
        MIME-Version: 1.0\r\n\
        From: Mail Delivery System <{postmaster}>\r\n\
        To: {sender}\r\n\
        Subject: Delivery Status Notification (Failure)\r\n\
        Auto-Submitted: auto-replied\r\n\
        \r\n\
        This is a multi-part message in MIME format.\r\n\
        \r\n\
        --{boundary}\r\n\
        Content-Type: text/plain; charset=utf-8\r\n\
        Content-Transfer-Encoding: 7bit\r\n\
        \r\n\
        {human_readable}\r\n\
        --{boundary}\r\n\
        Content-Type: message/delivery-status\r\n\
        Content-Transfer-Encoding: 7bit\r\n\
        \r\n\
        {machine_readable}\r\n\
        --{boundary}\r\n\
        Content-Type: text/rfc822-headers\r\n\
        Content-Transfer-Encoding: 7bit\r\n\
        \r\n\
        {original_headers}\r\n\
        --{boundary}--\r\n",
        boundary = boundary,
        postmaster = config.postmaster,
        sender = original_sender,
        human_readable = human_readable,
        machine_readable = machine_readable,
        original_headers = original_headers,
    );

    // Create a new Context for the DSN message
    let mut dsn_context = Context {
        extended: false,
        envelope: Envelope::default(),
        id: ulid::Ulid::new().to_string(),
        data: Some(Arc::from(dsn_body.as_bytes())),
        response: None,
        metadata: AHashMap::default(),
        banner: Arc::from(config.reporting_mta.as_str()),
        max_message_size: 0,
        capabilities: Vec::new(),
        delivery: None,
        tracking_id: None,
    };

    // Set envelope: FROM postmaster, TO original sender
    // Parse postmaster address
    let postmaster_addr = mailparse::addrparse(&config.postmaster)
        .ok()
        .and_then(|mut addrs| addrs.pop())
        .map(Address)
        .ok_or_else(|| {
            PermanentError::InvalidRecipient(format!(
                "Invalid postmaster address: {}",
                config.postmaster
            ))
        })?;

    *dsn_context.envelope.sender_mut() = Some(postmaster_addr);

    // Set recipient to original sender
    *dsn_context.envelope.recipients_mut() = Some(AddressList::from(vec![Address(
        (**original_sender).clone(),
    )]));

    Ok(dsn_context)
}

/// Build the human-readable text part of the DSN (Part 1)
fn build_human_readable_part(
    original_sender: &Address,
    original_recipients: &AddressList,
    delivery_info: &DeliveryInfo,
    error: &DeliveryError,
) -> String {
    let recipient_list = original_recipients
        .iter()
        .map(|addr| match &**addr {
            MailAddr::Single(info) => info.addr.as_str(),
            MailAddr::Group(group) => group.group_name.as_str(),
        })
        .collect::<Vec<_>>()
        .join(", ");

    format!(
        "This is the mail system at host {recipient_list}.\n\
        \n\
        I'm sorry to have to inform you that your message could not\n\
        be delivered to one or more recipients. It's attached below.\n\
        \n\
        For further assistance, please contact <{error}>.\n\
        \n\
        Your message could not be delivered:\n\
        \n\
        {recipient_list}: {error}\n\
        \n\
        Message details:\n\
        - Original sender: {original_sender}\n\
        - Failed recipient(s): {recipient_list}\n\
        - Delivery attempts: {attempts}\n\
        - Domain: {domain}\n\
        {last_server}",
        recipient_list = recipient_list,
        error = error,
        original_sender = original_sender,
        attempts = delivery_info.attempt_count(),
        domain = delivery_info.recipient_domain,
        last_server =
            delivery_info
                .current_mail_server()
                .map_or_else(String::new, |server| format!(
                    "- Last server attempted: {}",
                    server.address()
                ))
    )
}

/// Build the machine-readable delivery status part (Part 2)
fn build_machine_readable_part(
    config: &DsnConfig,
    original_recipients: &AddressList,
    delivery_info: &DeliveryInfo,
    error: &DeliveryError,
) -> String {
    // Determine SMTP status code from error type
    let (status_code, action) = if error.is_permanent() {
        ("5.0.0", "failed")
    } else {
        ("4.0.0", "failed") // Temporary failure that exhausted retries
    };

    // Per-message fields (mandatory: Reporting-MTA)
    let mut dsn = format!("Reporting-MTA: dns; {}\r\n", config.reporting_mta);

    // Add arrival date if available
    if let Ok(duration) = delivery_info
        .queued_at
        .duration_since(SystemTime::UNIX_EPOCH)
    {
        let timestamp = chrono::DateTime::<chrono::Utc>::from_timestamp(
            duration.as_secs().try_into().unwrap_or(0),
            duration.subsec_nanos(),
        )
        .map_or_else(|| "unknown".to_string(), |dt| dt.to_rfc2822());
        let _ = write!(dsn, "Arrival-Date: {timestamp}\r\n");
    }

    // Per-recipient fields (one group per recipient)
    for recipient in original_recipients.iter() {
        let recipient_addr = match &**recipient {
            MailAddr::Single(info) => &info.addr,
            MailAddr::Group(group) => &group.group_name,
        };

        dsn.push_str("\r\n"); // Blank line separates per-recipient groups

        // Final-Recipient (mandatory)
        let _ = write!(dsn, "Final-Recipient: rfc822; {recipient_addr}\r\n");

        // Action (mandatory)
        let _ = write!(dsn, "Action: {action}\r\n");

        // Status (mandatory) - enhanced status code
        let _ = write!(dsn, "Status: {status_code}\r\n");

        // Diagnostic-Code (optional but helpful)
        let _ = write!(dsn, "Diagnostic-Code: smtp; {error}\r\n");

        // Remote-MTA (if available)
        if let Some(server) = delivery_info.current_mail_server() {
            let _ = write!(dsn, "Remote-MTA: dns; {}\r\n", server.host);
        }

        // Last-Attempt-Date (if available)
        if let Some(last_attempt) = delivery_info.attempts.last()
            && let Ok(duration) = last_attempt
                .timestamp
                .duration_since(SystemTime::UNIX_EPOCH)
        {
            let timestamp = chrono::DateTime::<chrono::Utc>::from_timestamp(
                duration.as_secs().try_into().unwrap_or(0),
                duration.subsec_nanos(),
            )
            .map_or_else(|| "unknown".to_string(), |dt| dt.to_rfc2822());
            let _ = write!(dsn, "Last-Attempt-Date: {timestamp}\r\n");
        }
    }

    dsn
}

/// Extract original message headers for Part 3
///
/// Returns the first 1KB of the original message headers to avoid
/// including large message bodies in the DSN.
fn extract_original_headers(original_context: &Context) -> String {
    original_context.data.as_ref().map_or_else(
        || String::from("(No message data available)"),
        |data| {
            // Find the end of headers (double CRLF)
            let header_end = data
                .windows(4)
                .position(|w| w == b"\r\n\r\n")
                .unwrap_or_else(|| data.len().min(1024)); // Limit to 1KB

            String::from_utf8_lossy(&data[..header_end]).to_string()
        },
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use empath_common::{DeliveryAttempt, domain::Domain};
    use empath_spool::SpooledMessageId;

    use super::*;
    use crate::{TemporaryError, types::DeliveryInfo};

    #[test]
    fn test_should_generate_dsn_permanent_failure() {
        let mut context = Context::default();
        let sender = mailparse::addrparse("sender@example.com")
            .unwrap()
            .remove(0);
        *context.envelope.sender_mut() = Some(Address(sender));

        let info = DeliveryInfo {
            message_id: SpooledMessageId::new(ulid::Ulid::new()),
            recipient_domain: Domain::new("example.com"),
            status: DeliveryStatus::Failed("Permanent failure".to_string()),
            attempts: vec![],
            queued_at: SystemTime::now(),
            next_retry_at: None,
            current_server_index: 0,
            mail_servers: Arc::new(vec![]),
        };

        let error = DeliveryError::Permanent(PermanentError::InvalidRecipient(
            "user@example.com".to_string(),
        ));

        assert!(should_generate_dsn(&context, &info, &error));
    }

    #[test]
    fn test_should_not_generate_dsn_null_sender() {
        let mut context = Context::default();
        // Null sender (bounce message)
        let sender = MailAddr::Single(mailparse::SingleInfo {
            addr: String::new(),
            display_name: None,
        });
        *context.envelope.sender_mut() = Some(Address(sender));

        let info = DeliveryInfo {
            message_id: SpooledMessageId::new(ulid::Ulid::new()),
            recipient_domain: Domain::new("example.com"),
            status: DeliveryStatus::Failed("Permanent failure".to_string()),
            attempts: vec![],
            queued_at: SystemTime::now(),
            next_retry_at: None,
            current_server_index: 0,
            mail_servers: Arc::new(vec![]),
        };

        let error = DeliveryError::Permanent(PermanentError::InvalidRecipient(
            "user@example.com".to_string(),
        ));

        assert!(!should_generate_dsn(&context, &info, &error));
    }

    #[test]
    fn test_should_not_generate_dsn_temporary_in_retry() {
        let mut context = Context::default();
        let sender = mailparse::addrparse("sender@example.com")
            .unwrap()
            .remove(0);
        *context.envelope.sender_mut() = Some(Address(sender));

        let info = DeliveryInfo {
            message_id: SpooledMessageId::new(ulid::Ulid::new()),
            recipient_domain: Domain::new("example.com"),
            status: DeliveryStatus::Retry {
                attempts: 2,
                last_error: "Temporary failure".to_string(),
            },
            attempts: vec![],
            queued_at: SystemTime::now(),
            next_retry_at: Some(SystemTime::now()),
            current_server_index: 0,
            mail_servers: Arc::new(vec![]),
        };

        let error = DeliveryError::Temporary(TemporaryError::ServerBusy("Server busy".to_string()));

        assert!(!should_generate_dsn(&context, &info, &error));
    }

    #[test]
    fn test_generate_dsn_creates_valid_context() {
        let mut context = Context::default();
        let sender = mailparse::addrparse("sender@example.com")
            .unwrap()
            .remove(0);
        *context.envelope.sender_mut() = Some(Address(sender));

        let recipient = mailparse::addrparse("recipient@example.com")
            .unwrap()
            .remove(0);
        *context.envelope.recipients_mut() = Some(AddressList::from(vec![Address(recipient)]));

        context.data = Some(Arc::from(
            b"From: sender@example.com\r\nTo: recipient@example.com\r\nSubject: Test\r\n\r\nBody"
                .as_slice(),
        ));

        let info = DeliveryInfo {
            message_id: SpooledMessageId::new(ulid::Ulid::new()),
            recipient_domain: Domain::new("example.com"),
            status: DeliveryStatus::Failed("Permanent failure".to_string()),
            attempts: vec![DeliveryAttempt {
                timestamp: SystemTime::now(),
                error: Some("Connection refused".to_string()),
                server: "mx.example.com:25".to_string(),
            }],
            queued_at: SystemTime::now(),
            next_retry_at: None,
            current_server_index: 0,
            mail_servers: Arc::new(vec![]),
        };

        let error = DeliveryError::Permanent(PermanentError::InvalidRecipient(
            "recipient@example.com".to_string(),
        ));

        let config = DsnConfig::default();

        let dsn = generate_dsn(&context, &info, &error, &config).unwrap();

        // Verify DSN envelope
        assert!(dsn.envelope.sender().is_some());
        assert!(dsn.envelope.recipients().is_some());
        assert_eq!(dsn.envelope.recipients().unwrap().len(), 1);

        // Verify DSN has message data
        assert!(dsn.data.is_some());

        // Verify DSN contains required parts
        let dsn_body = String::from_utf8_lossy(dsn.data.as_ref().unwrap());
        assert!(dsn_body.contains("multipart/report"));
        assert!(dsn_body.contains("delivery-status"));
        assert!(dsn_body.contains("Reporting-MTA"));
        assert!(dsn_body.contains("Final-Recipient"));
        assert!(dsn_body.contains("Action: failed"));
        assert!(dsn_body.contains("Status: 5.0.0"));
    }
}
