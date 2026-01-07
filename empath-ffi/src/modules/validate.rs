use empath_common::context::Context;
use empath_common::internal;
use empath_tracing::traced;
use serde::{Deserialize, Serialize};

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Deserialize, Serialize)]
// cbindgen:prefix-with-name=Validate
pub enum Event {
    Connect,
    Ehlo,
    MailFrom,
    RcptTo,
    Data,
    PreSpool,
    StartTls,
    Auth,
}

#[derive(Debug)]
#[repr(C)]
pub struct Validators {
    pub validate_connect: Option<unsafe extern "C-unwind" fn(&mut Context) -> i32>,
    pub validate_ehlo: Option<unsafe extern "C-unwind" fn(&mut Context) -> i32>,
    pub validate_mail_from: Option<unsafe extern "C-unwind" fn(&mut Context) -> i32>,
    pub validate_rcpt_to: Option<unsafe extern "C-unwind" fn(&mut Context) -> i32>,
    pub validate_data: Option<unsafe extern "C-unwind" fn(&mut Context) -> i32>,
    pub validate_pre_spool: Option<unsafe extern "C-unwind" fn(&mut Context) -> i32>,
    pub validate_starttls: Option<unsafe extern "C-unwind" fn(&mut Context) -> i32>,
    pub validate_auth: Option<unsafe extern "C-unwind" fn(&mut Context) -> i32>,
}

#[derive(Debug)]
#[repr(C)]
pub struct ModuleValidation {
    pub module_name: *const libc::c_char,
    pub init: super::Init,
    pub validators: Validators,
}

unsafe impl Send for ModuleValidation {}
unsafe impl Sync for ModuleValidation {}

impl ModuleValidation {
    /// Emit an event to this library's validation module
    #[traced(instrument(level = tracing::Level::TRACE, skip_all), timing)]
    pub fn emit(&self, event: super::Event, context: &mut Context) -> i32 {
        match event {
            super::Event::Validate(Event::Connect) => unsafe {
                self.validators.validate_connect.map(|func| func(context))
            },
            super::Event::Validate(Event::Ehlo) => unsafe {
                self.validators.validate_ehlo.map(|func| func(context))
            },
            super::Event::Validate(Event::MailFrom) => unsafe {
                self.validators.validate_mail_from.map(|func| func(context))
            },
            super::Event::Validate(Event::RcptTo) => unsafe {
                self.validators.validate_rcpt_to.map(|func| func(context))
            },
            super::Event::Validate(Event::Data) => unsafe {
                self.validators.validate_data.map(|func| func(context))
            },
            super::Event::Validate(Event::PreSpool) => unsafe {
                self.validators.validate_pre_spool.map(|func| func(context))
            },
            super::Event::Validate(Event::StartTls) => unsafe {
                self.validators.validate_starttls.map(|func| func(context))
            },
            super::Event::Validate(Event::Auth) => unsafe {
                self.validators.validate_auth.map(|func| func(context))
            },
            _ => None,
        }
        .inspect(|v| internal!("{event:?} = {v}"))
        .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use std::ptr;

    use empath_common::context::Context;

    use super::{Event as ValidateEvent, ModuleValidation, Validators};
    use crate::modules::Event as ModuleEvent;
    use crate::string::StringVector;

    unsafe extern "C-unwind" fn validate_connect(_context: &mut Context) -> i32 {
        250
    }

    unsafe extern "C" fn init_stub(_args: StringVector) -> i32 {
        0
    }

    #[test]
    fn emit_returns_validator_result() {
        let module = ModuleValidation {
            module_name: ptr::null(),
            init: init_stub,
            validators: Validators {
                validate_connect: Some(validate_connect),
                validate_ehlo: None,
                validate_mail_from: None,
                validate_rcpt_to: None,
                validate_data: None,
                validate_pre_spool: None,
                validate_starttls: None,
                validate_auth: None,
            },
        };
        let mut context = Context::default();

        let result = module.emit(ModuleEvent::Validate(ValidateEvent::Connect), &mut context);

        assert_eq!(result, 250);
    }

    #[test]
    fn emit_defaults_when_validator_missing() {
        let module = ModuleValidation {
            module_name: ptr::null(),
            init: init_stub,
            validators: Validators {
                validate_connect: None,
                validate_ehlo: None,
                validate_mail_from: None,
                validate_rcpt_to: None,
                validate_data: None,
                validate_pre_spool: None,
                validate_starttls: None,
                validate_auth: None,
            },
        };
        let mut context = Context::default();

        let result = module.emit(ModuleEvent::Validate(ValidateEvent::Ehlo), &mut context);

        assert_eq!(result, 0);
    }
}
