#![no_main]

extern crate alloc;

empath_wasm_guest::bindings!();

struct Validator;

export!(Validator);

fn maybe_trigger_event_error(ctx: &Context) {
    if ctx.session_id().starts_with("panic-event") {
        panic!("intentional event panic for testing");
    }
    if let Some(flag) = host::transaction_get("panic_event") {
        panic!("intentional event panic via transaction flag: {flag}");
    }
}

impl ValidationGuest for Validator {
    /// Validate connection - called when client connects
    fn validate_connect(ctx: Context) -> ValidationResult {
        host::log(format!(
            "WASM: validate_connect called for session ID: {}",
            ctx.session_id()
        ));

        host::session_set("connection_validated", "true");

        accept()
    }

    /// Validate EHLO/HELO command
    fn validate_ehlo(ctx: Context) -> ValidationResult {
        host::log(format!(
            "WASM: validate_ehlo called (extended: {})",
            ctx.is_extended()
        ));

        accept()
    }

    /// Validate MAIL FROM command (sender)
    fn validate_mail_from(ctx: Context) -> ValidationResult {
        let sender = ctx.sender().unwrap_or("<none>");
        host::log(format!("WASM: validate_mail_from - sender: {sender}"));

        if sender.ends_with("@spam.example.com") {
            host::log("WASM: Blocking sender from spam.example.com");

            host::transaction_set("rejection_reason", "sender_blocked_spam_domain");

            return reject(
                550,
                "Sender from spam.example.com is not allowed",
            );
        }

        host::transaction_set("validated_sender", sender);

        accept()
    }

    /// Validate RCPT TO command (recipient)
    fn validate_rcpt_to(ctx: Context) -> ValidationResult {
        host::log(format!(
            "WASM: validate_rcpt_to - recipients: {:?}",
            ctx.recipients()
        ));

        if ctx.recipients().len() > 10 {
            host::log("WASM: Too many recipients");

            host::transaction_set("rejection_reason", "too_many_recipients");

            return reject(
                452,
                "Too many recipients (max 10)",
            );
        }

        host::transaction_set("recipient_count", &ctx.recipients().len().to_string());

        accept()
    }

    /// Validate DATA command (message content)
    fn validate_data(ctx: Context) -> ValidationResult {
        if let Some(data) = ctx.data() {
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

        if let Some(data) = ctx.data() {
            if let Some(rest) = data.strip_prefix(b"alloc:") {
                if let Some(size) =
                    core::str::from_utf8(rest).ok().and_then(|s| s.parse::<usize>().ok())
                {
                    let mut buffer = Vec::with_capacity(size);
                    for i in 0..size {
                        buffer.push((i % 256) as u8);
                    }
                    core::mem::forget(buffer);
                }
            }
        }

        let data_size = ctx.data().map_or(0, |d| d.len());

        host::log(format!(
            "WASM: validate_data - message size: {data_size} bytes"
        ));

        if data_size > 1_000_000 {
            host::log("WASM: Message too large");

            host::transaction_set("rejection_reason", "message_too_large");

            return reject(
                552,
                "Message exceeds maximum size (1MB)",
            );
        }

        if let Some(data) = ctx.data() {
            if let Ok(msg_str) = core::str::from_utf8(data) {
                let spam_keywords = ["viagra", "pills", "enlarge"];
                for keyword in &spam_keywords {
                    if msg_str.to_lowercase().contains(keyword) {
                        host::log(format!("WASM: Spam keyword detected: {keyword}"));

                        host::transaction_set("spam_keyword", keyword);

                        return reject(
                            550,
                            "Message contains spam content",
                        );
                    }
                }
            }
        }

        host::transaction_set("message_size", &data_size.to_string());

        if let Some(sender) = host::transaction_get("validated_sender") {
            host::log(format!(
                "WASM: Message from previously validated sender: {sender}"
            ));
        }

        accept()
    }

    /// Validate STARTTLS command
    fn validate_starttls(ctx: Context) -> ValidationResult {
        host::log(format!(
            "WASM: validate_starttls called for session {}",
            ctx.session_id()
        ));

        accept()
    }
}

impl EventsGuest for Validator {
    /// Called when a client connection is opened
    fn on_connection_opened(ctx: Context) {
        maybe_trigger_event_error(&ctx);
        host::log(format!(
            "WASM Event: Connection opened - session ID: {}",
            ctx.session_id()
        ));

        host::session_set("connection_opened_at", &format!("{}", ctx.session_id()));
    }

    /// Called when a client connection is closed
    fn on_connection_closed(ctx: Context) {
        maybe_trigger_event_error(&ctx);
        host::log(format!(
            "WASM Event: Connection closed - session ID: {}",
            ctx.session_id()
        ));

        if host::session_get("connection_opened_at").is_some() {
            host::log("WASM Event: Connection was previously tracked");
        }
    }

    /// Called before attempting delivery to a mail server
    fn on_delivery_attempt(ctx: Context) {
        maybe_trigger_event_error(&ctx);
        host::log(format!(
            "WASM Event: Delivery attempt for message {}",
            ctx.session_id()
        ));

        if let Some(spam_keyword) = host::transaction_get("spam_keyword") {
            host::log(format!(
                "WASM Event: Delivering message flagged for spam keyword: {}",
                spam_keyword
            ));
        }
    }

    /// Called when delivery succeeds
    fn on_delivery_success(ctx: Context) {
        maybe_trigger_event_error(&ctx);
        host::log(format!(
            "WASM Event: Delivery succeeded for message {}",
            ctx.session_id()
        ));
    }

    /// Called when delivery fails
    fn on_delivery_failure(ctx: Context) {
        maybe_trigger_event_error(&ctx);
        host::log(format!(
            "WASM Event: Delivery failed for message {}",
            ctx.session_id()
        ));
    }

    /// Called when an SMTP command returns an error response
    fn on_smtp_error(ctx: Context) {
        maybe_trigger_event_error(&ctx);
        host::log(format!(
            "WASM Event: SMTP error for session {}",
            ctx.session_id()
        ));
    }

    /// Called when a complete message is received
    fn on_smtp_message_received(ctx: Context) {
        maybe_trigger_event_error(&ctx);
        let data_size = ctx.data().map_or(0, |d| d.len());

        host::log(format!(
            "WASM Event: Message received - {} bytes from sender: {}",
            data_size,
            ctx.sender().unwrap_or("<none>")
        ));
    }

    /// Called after a DNS lookup completes
    fn on_dns_lookup(ctx: Context) {
        maybe_trigger_event_error(&ctx);
        host::log(format!(
            "WASM Event: DNS lookup completed for session {}",
            ctx.session_id()
        ));
    }
}
