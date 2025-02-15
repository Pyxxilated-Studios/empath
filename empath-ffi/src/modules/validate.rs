use empath_common::{context::Context, internal};
use empath_tracing::traced;
use serde::Deserialize;

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Deserialize)]
// cbindgen:prefix-with-name=Validate
pub enum Event {
    Connect,
    MailFrom,
    Data,
    StartTls,
}

#[repr(C)]
#[allow(clippy::struct_field_names)]
pub struct Validators {
    pub validate_connect: Option<unsafe extern "C-unwind" fn(&mut Context) -> i32>,
    pub validate_mail_from: Option<unsafe extern "C-unwind" fn(&mut Context) -> i32>,
    pub validate_data: Option<unsafe extern "C-unwind" fn(&mut Context) -> i32>,
    pub validate_starttls: Option<unsafe extern "C-unwind" fn(&mut Context) -> i32>,
}

#[repr(C)]
#[allow(clippy::module_name_repetitions)]
pub struct Validation {
    pub module_name: *const libc::c_char,
    pub init: super::Init,
    pub validators: Validators,
}

unsafe impl Send for Validation {}
unsafe impl Sync for Validation {}

impl Validation {
    ///
    /// Emit an event to this library's validation module
    ///
    #[traced(instrument(level = tracing::Level::TRACE, skip_all), timing)]
    pub fn emit(&self, event: super::Event, context: &mut Context) -> i32 {
        match event {
            super::Event::Validate(Event::Data) => unsafe {
                self.validators.validate_data.map(|func| func(context))
            },
            super::Event::Validate(Event::MailFrom) => unsafe {
                self.validators.validate_mail_from.map(|func| func(context))
            },
            super::Event::Validate(Event::Connect) => unsafe {
                self.validators.validate_connect.map(|func| func(context))
            },
            super::Event::Validate(Event::StartTls) => unsafe {
                self.validators.validate_starttls.map(|func| func(context))
            },
            _ => None,
        }
        .inspect(|v| internal!("{event:?} = {v}"))
        .unwrap_or_default()
    }
}
