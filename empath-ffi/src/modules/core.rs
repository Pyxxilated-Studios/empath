use std::any::Any;
use std::borrow::Cow;

use empath_common::context::{Context, ModuleContext};
use empath_common::status::Status;
use empath_tracing::traced;
use serde::{Deserialize, Serialize};

use super::traits::ModuleInitContext;
use super::{Error, Event, validate};
use crate::modules::traits::Module;

/// Core validation handlers - stateless functions operating on Context
///
/// All validation logic reads from Context fields, making core module
/// completely decoupled from Session implementation. The core module
/// accesses the same data available to all validation modules.
#[derive(Debug, Serialize, Deserialize)]
pub struct Core;

impl Core {
    /// Create a new core validators instance
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Validation handler for CONNECT - sets the service ready banner
    fn validate_connect(validation_context: &mut Context) -> i32 {
        validation_context.response = Some((
            Status::ServiceReady,
            Cow::Owned(validation_context.banner.to_string()),
        ));
        0
    }

    /// Validation handler for EHLO/HELO
    const fn validate_ehlo(_validation_context: &mut Context) -> i32 {
        0
    }

    /// Validation handler for MAIL FROM - validates SIZE parameter per RFC 1870
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
                Cow::Owned(format!(
                    "5.2.3 Declared message size exceeds maximum (declared: {} bytes, maximum: {} bytes)",
                    declared_size, validation_context.max_message_size
                )),
            ));
            return 1;
        }

        validation_context.response = Some((Status::Ok, Cow::Borrowed("Ok")));
        0
    }

    /// Validation handler for RCPT TO
    fn validate_rcpt_to(validation_context: &mut Context) -> i32 {
        validation_context.response = Some((Status::Ok, Cow::Borrowed("Ok")));
        0
    }

    /// Validation handler for DATA
    fn validate_data(validation_context: &mut Context) -> i32 {
        validation_context.response = Some((Status::Ok, Cow::Borrowed("Ok: queued")));
        0
    }

    /// Validation handler for pre-spool phase
    const fn validate_pre_spool(_validation_context: &mut Context) -> i32 {
        0
    }

    /// Validation handler for STARTTLS
    const fn validate_start_tls(_validation_context: &mut Context) -> i32 {
        // TLS validation happens elsewhere
        0
    }

    /// Validation handler for AUTH
    const fn validate_auth(_validation_context: &mut Context) -> i32 {
        0
    }
}

impl Default for Core {
    fn default() -> Self {
        Self::new()
    }
}

#[typetag::serde]
impl Module for Core {
    fn init(&mut self, _context: &ModuleInitContext) -> Result<(), Error> {
        // Core module is stateless - no initialization required
        Ok(())
    }

    #[traced(instrument(level = tracing::Level::TRACE, skip_all), timing)]
    fn emit(&self, event: Event, context: &mut Context) -> i32 {
        match event {
            Event::Validate(validate::Event::Connect) => Self::validate_connect(context),
            Event::Validate(validate::Event::Ehlo) => Self::validate_ehlo(context),
            Event::Validate(validate::Event::MailFrom) => Self::validate_mail_from(context),
            Event::Validate(validate::Event::RcptTo) => Self::validate_rcpt_to(context),
            Event::Validate(validate::Event::Data) => Self::validate_data(context),
            Event::Validate(validate::Event::PreSpool) => Self::validate_pre_spool(context),
            Event::Validate(validate::Event::StartTls) => Self::validate_start_tls(context),
            Event::Validate(validate::Event::Auth) => Self::validate_auth(context),
            Event::Event(_) => 0, // Core module doesn't handle connection lifecycle events
        }
    }

    fn module_context(&self) -> ModuleContext {
        ModuleContext::new("core", "core")
    }

    fn display_name(&self) -> String {
        "Core Module".to_string()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
