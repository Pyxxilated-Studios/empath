#![no_main]

// Generate bindings from the WIT file
wit_bindgen::generate!({
    world: "validator",
    path: "wit",
});

// Import the generated types
use exports::empath::validator::validation::{Context, Guest, Rejection, ValidationResult};

struct Validator;

export!(Validator);

impl Guest for Validator {
    /// Validate connection - called when client connects
    fn validate_connect(ctx: Context) -> ValidationResult {
        // Use the host log function to output debug information
        empath::validator::host::log(&format!(
            "WASM: validate_connect called for session ID: {}",
            ctx.id
        ));

        // Track connection in session context
        empath::validator::host::session_set("connection_validated", "true");

        // Accept all connections
        ValidationResult::Accept
    }

    /// Validate EHLO/HELO command
    fn validate_ehlo(ctx: Context) -> ValidationResult {
        empath::validator::host::log(&format!(
            "WASM: validate_ehlo called (extended: {})",
            ctx.extended
        ));

        // Accept all EHLO/HELO commands
        ValidationResult::Accept
    }

    /// Validate MAIL FROM command (sender)
    fn validate_mail_from(ctx: Context) -> ValidationResult {
        let sender = ctx.sender.as_deref().unwrap_or("<none>");
        empath::validator::host::log(&format!("WASM: validate_mail_from - sender: {}", sender));

        // Example: Block emails from spam.example.com
        if sender.ends_with("@spam.example.com") {
            empath::validator::host::log("WASM: Blocking sender from spam.example.com");

            // Store rejection reason in transaction context (persisted to spool)
            empath::validator::host::transaction_set(
                "rejection_reason",
                "sender_blocked_spam_domain",
            );

            return ValidationResult::Reject(Rejection {
                code: 550,
                message: "Sender from spam.example.com is not allowed".to_string(),
            });
        }

        // Store validated sender in transaction context
        empath::validator::host::transaction_set("validated_sender", sender);

        ValidationResult::Accept
    }

    /// Validate RCPT TO command (recipient)
    fn validate_rcpt_to(ctx: Context) -> ValidationResult {
        empath::validator::host::log(&format!(
            "WASM: validate_rcpt_to - recipients: {:?}",
            ctx.recipients
        ));

        // Example: Limit number of recipients
        if ctx.recipients.len() > 10 {
            empath::validator::host::log("WASM: Too many recipients");

            empath::validator::host::transaction_set("rejection_reason", "too_many_recipients");

            return ValidationResult::Reject(Rejection {
                code: 452,
                message: "Too many recipients (max 10)".to_string(),
            });
        }

        // Track recipient count in transaction context
        empath::validator::host::transaction_set(
            "recipient_count",
            &ctx.recipients.len().to_string(),
        );

        ValidationResult::Accept
    }

    /// Validate DATA command (message content)
    fn validate_data(ctx: Context) -> ValidationResult {
        // Check if we have message data
        let data_size = ctx.data.as_ref().map_or(0, |d| d.len());

        empath::validator::host::log(&format!(
            "WASM: validate_data - message size: {} bytes",
            data_size
        ));

        // Example: Check message size
        if data_size > 1_000_000 {
            empath::validator::host::log("WASM: Message too large");

            empath::validator::host::transaction_set("rejection_reason", "message_too_large");

            return ValidationResult::Reject(Rejection {
                code: 552,
                message: "Message exceeds maximum size (1MB)".to_string(),
            });
        }

        // Example: Simple spam check - look for spam keywords in message
        if let Some(data) = &ctx.data {
            if let Ok(msg_str) = std::str::from_utf8(data) {
                let spam_keywords = ["viagra", "pills", "enlarge"];
                for keyword in &spam_keywords {
                    if msg_str.to_lowercase().contains(keyword) {
                        empath::validator::host::log(&format!(
                            "WASM: Spam keyword detected: {}",
                            keyword
                        ));

                        empath::validator::host::transaction_set(
                            "spam_keyword",
                            keyword,
                        );

                        return ValidationResult::Reject(Rejection {
                            code: 550,
                            message: "Message contains spam content".to_string(),
                        });
                    }
                }
            }
        }

        // Store message size in transaction context
        empath::validator::host::transaction_set("message_size", &data_size.to_string());

        // Check if we previously validated the sender
        if let Some(sender) = empath::validator::host::transaction_get("validated_sender") {
            empath::validator::host::log(&format!(
                "WASM: Message from previously validated sender: {}",
                sender
            ));
        }

        ValidationResult::Accept
    }

    /// Validate STARTTLS command
    fn validate_starttls(ctx: Context) -> ValidationResult {
        empath::validator::host::log(&format!(
            "WASM: validate_starttls called for session {}",
            ctx.id
        ));

        // Accept STARTTLS (TLS upgrade)
        ValidationResult::Accept
    }
}
