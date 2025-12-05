#![no_main]

// Generate bindings from the WIT file
wit_bindgen::generate!({
    world: "validator",
    path: "wit",
});

// Import the generated types
use exports::empath::validator::{
    events::Guest as EventsGuest,
    validation::{Context, Guest as ValidationGuest, Rejection, ValidationResult},
};

struct Validator;

export!(Validator);

// Implement validation callbacks
impl ValidationGuest for Validator {
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
        // Trigger a deterministic busy loop when the message begins with "timeout"
        if let Some(data) = &ctx.data {
            if data.starts_with(b"timeout") {
                let mut spins: u64 = 0;
                loop {
                    spins = spins.wrapping_add(1);
                    if spins % 1_000_000 == 0 {
                        core::hint::spin_loop();
                    }
                }
            }
        }

        // Allocate caller-specified bytes when prefixed with "alloc:" to exercise memory limits.
        if let Some(data) = &ctx.data {
            if let Some(rest) = data.strip_prefix(b"alloc:") {
                if let Some(size) =
                    std::str::from_utf8(rest).ok().and_then(|s| s.parse::<usize>().ok())
                {
                    let mut buffer = Vec::with_capacity(size);
                    // Touch each byte to ensure growth occurs
                    for i in 0..size {
                        buffer.push((i % 256) as u8);
                    }
                    // Prevent compiler from optimizing the buffer away
                    std::mem::forget(buffer);
                }
            }
        }

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

// Implement event callbacks
impl EventsGuest for Validator {
    /// Called when a client connection is opened
    fn on_connection_opened(ctx: Context) {
        empath::validator::host::log(&format!(
            "WASM Event: Connection opened - session ID: {}",
            ctx.id
        ));

        // Track connection metrics in session context
        empath::validator::host::session_set("connection_opened_at", &format!("{}", ctx.id));
    }

    /// Called when a client connection is closed
    fn on_connection_closed(ctx: Context) {
        empath::validator::host::log(&format!(
            "WASM Event: Connection closed - session ID: {}",
            ctx.id
        ));

        // Could log connection duration, bytes transferred, etc.
        if let Some(_opened_at) = empath::validator::host::session_get("connection_opened_at") {
            empath::validator::host::log("WASM Event: Connection was previously tracked");
        }
    }

    /// Called before attempting delivery to a mail server
    fn on_delivery_attempt(ctx: Context) {
        empath::validator::host::log(&format!(
            "WASM Event: Delivery attempt for message {}",
            ctx.id
        ));

        // Could check transaction context for spam scores, etc.
        if let Some(spam_keyword) = empath::validator::host::transaction_get("spam_keyword") {
            empath::validator::host::log(&format!(
                "WASM Event: Delivering message flagged for spam keyword: {}",
                spam_keyword
            ));
        }
    }

    /// Called when delivery succeeds
    fn on_delivery_success(ctx: Context) {
        empath::validator::host::log(&format!(
            "WASM Event: Delivery succeeded for message {}",
            ctx.id
        ));

        // Could update metrics, notify external systems, etc.
    }

    /// Called when delivery fails
    fn on_delivery_failure(ctx: Context) {
        empath::validator::host::log(&format!(
            "WASM Event: Delivery failed for message {}",
            ctx.id
        ));

        // Could implement custom retry logic, alerting, etc.
    }

    /// Called when an SMTP command returns an error response
    fn on_smtp_error(ctx: Context) {
        empath::validator::host::log(&format!(
            "WASM Event: SMTP error for session {}",
            ctx.id
        ));
    }

    /// Called when a complete message is received
    fn on_smtp_message_received(ctx: Context) {
        let data_size = ctx.data.as_ref().map_or(0, |d| d.len());

        empath::validator::host::log(&format!(
            "WASM Event: Message received - {} bytes from sender: {}",
            data_size,
            ctx.sender.as_deref().unwrap_or("<none>")
        ));

        // Could trigger post-processing, virus scanning, archiving, etc.
    }

    /// Called after a DNS lookup completes
    fn on_dns_lookup(ctx: Context) {
        empath::validator::host::log(&format!(
            "WASM Event: DNS lookup completed for session {}",
            ctx.id
        ));
    }
}
