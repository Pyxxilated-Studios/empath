use std::bstr;

use empath_common::{context::Context, status::Status};
use empath_tracing::traced;

use super::{Event, validate};

/// Core validation handlers - stateless functions operating on Context
///
/// All validation logic reads from Context fields, making core module
/// completely decoupled from Session implementation. The core module
/// accesses the same data available to all validation modules.
pub struct CoreValidators;

impl CoreValidators {
    /// Create a new core validators instance
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl Default for CoreValidators {
    fn default() -> Self {
        Self::new()
    }
}

/// Validation handler functions - read all state from Context
fn validate_connect(validation_context: &mut Context) -> i32 {
    validation_context.response = Some((Status::ServiceReady, validation_context.banner.clone()));
    0
}

fn validate_ehlo(validation_context: &mut Context) -> i32 {
    validation_context.response = Some((
        Status::Ok,
        format!(
            "{} says hello to {}",
            validation_context.banner,
            bstr::ByteStr::new(&validation_context.id())
        ),
    ));

    0
}

fn validate_mail_from(validation_context: &mut Context) -> i32 {
    // Validate SIZE parameter (RFC 1870)
    if validation_context.max_message_size > 0
        && let Some(mail_params) = validation_context.envelope.mail_params()
        && let Some(Some(size_str)) = mail_params.get("SIZE")
        && let Ok(declared_size) = size_str.parse::<usize>()
        && declared_size > validation_context.max_message_size
    {
        validation_context.response = Some((
            Status::ExceededStorage,
            format!(
                "5.2.3 Declared message size exceeds maximum (declared: {} bytes, maximum: {} bytes)",
                declared_size, validation_context.max_message_size
            ),
        ));
        return 1;
    }

    validation_context.response = Some((Status::Ok, "Ok".to_string()));
    0
}

fn validate_rcpt_to(validation_context: &mut Context) -> i32 {
    validation_context.response = Some((Status::Ok, "Ok".to_string()));
    0
}

fn validate_data(validation_context: &mut Context) -> i32 {
    validation_context.response = Some((Status::Ok, "Ok: queued".to_string()));
    0
}

const fn validate_start_tls(_validation_context: &mut Context) -> i32 {
    // TLS validation happens elsewhere
    0
}

/// Dispatch validation event to core module handlers
#[traced(instrument(level = tracing::Level::TRACE, skip_all), timing)]
pub(super) fn emit(
    _validators: &CoreValidators,
    event: Event,
    validation_context: &mut Context,
) -> i32 {
    match event {
        Event::Validate(validate::Event::Connect) => validate_connect(validation_context),
        Event::Validate(validate::Event::Ehlo) => validate_ehlo(validation_context),
        Event::Validate(validate::Event::MailFrom) => validate_mail_from(validation_context),
        Event::Validate(validate::Event::RcptTo) => validate_rcpt_to(validation_context),
        Event::Validate(validate::Event::Data) => validate_data(validation_context),
        Event::Validate(validate::Event::StartTls) => validate_start_tls(validation_context),
        Event::Event(_) => 0, // Core module doesn't handle connection lifecycle events
    }
}
