#![no_main]

// Generate bindings from the shared WIT contract.
wit_bindgen::generate!({
    world: "validator",
    path: "../../../wit",
});

use exports::empath::validator::{
    events::{EventContext, Guest as EventsGuest},
    validation::{Guest as ValidationGuest, ValidationContext, ValidationResult},
};

struct Validator;

export!(Validator);

impl ValidationGuest for Validator {
    fn validate_connect(_ctx: ValidationContext) -> ValidationResult {
        ValidationResult::Accept
    }

    fn validate_ehlo(_ctx: ValidationContext) -> ValidationResult {
        ValidationResult::Accept
    }

    fn validate_mail_from(_ctx: ValidationContext) -> ValidationResult {
        ValidationResult::Accept
    }

    fn validate_rcpt_to(_ctx: ValidationContext) -> ValidationResult {
        ValidationResult::Accept
    }

    fn validate_data(_ctx: ValidationContext) -> ValidationResult {
        let _ = empath::validator::message::headers_set("X-Wasm-Test", "ok");
        let _ = empath::validator::message::body_append_text(" appended", "utf-8");
        ValidationResult::Accept
    }

    fn validate_starttls(_ctx: ValidationContext) -> ValidationResult {
        ValidationResult::Accept
    }
}

impl EventsGuest for Validator {
    fn on_connection_opened(_ctx: EventContext) {}
    fn on_connection_closed(_ctx: EventContext) {}
    fn on_delivery_attempt(_ctx: EventContext) {}
    fn on_delivery_success(_ctx: EventContext) {}
    fn on_delivery_failure(_ctx: EventContext) {}
    fn on_smtp_error(_ctx: EventContext) {}
    fn on_smtp_message_received(_ctx: EventContext) {}
    fn on_dns_lookup(_ctx: EventContext) {}
}
