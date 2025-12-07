#![no_main]

// Generate bindings from the shared WIT contract.
wit_bindgen::generate!({
    world: "validator",
    path: "../../../wit",
});

use exports::empath::validator::{
    events::Guest as EventsGuest,
    validation::{Context, Guest as ValidationGuest, ValidationResult},
};

struct Validator;

export!(Validator);

impl ValidationGuest for Validator {
    fn validate_connect(_ctx: Context) -> ValidationResult {
        ValidationResult::Accept
    }

    fn validate_ehlo(_ctx: Context) -> ValidationResult {
        ValidationResult::Accept
    }

    fn validate_mail_from(_ctx: Context) -> ValidationResult {
        ValidationResult::Accept
    }

    fn validate_rcpt_to(_ctx: Context) -> ValidationResult {
        ValidationResult::Accept
    }

    fn validate_data(_ctx: Context) -> ValidationResult {
        let _ = empath::validator::message::headers_set("X-Wasm-Test", "ok");
        let _ = empath::validator::message::body_append_text(" appended", "utf-8");
        ValidationResult::Accept
    }

    fn validate_starttls(_ctx: Context) -> ValidationResult {
        ValidationResult::Accept
    }
}

impl EventsGuest for Validator {
    fn on_connection_opened(_ctx: Context) {}
    fn on_connection_closed(_ctx: Context) {}
    fn on_delivery_attempt(_ctx: Context) {}
    fn on_delivery_success(_ctx: Context) {}
    fn on_delivery_failure(_ctx: Context) {}
    fn on_smtp_error(_ctx: Context) {}
    fn on_smtp_message_received(_ctx: Context) {}
    fn on_dns_lookup(_ctx: Context) {}
}
