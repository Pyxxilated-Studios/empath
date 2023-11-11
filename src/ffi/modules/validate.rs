use crate::{internal, smtp::context::Context};

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
// cbindgen:prefix-with-name=Validate
pub enum Event {
    Connect,
    MailFrom,
    Data,
}

#[repr(C)]
#[allow(clippy::struct_field_names)]
pub struct Validators {
    pub validate_connect: Option<unsafe extern "C" fn(&mut Context) -> i32>,
    pub validate_mail_from: Option<unsafe extern "C" fn(&mut Context) -> i32>,
    pub validate_data: Option<unsafe extern "C" fn(&mut Context) -> i32>,
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
            _ => None,
        }
        .inspect(|v| internal!("{event:?} = {v}"))
        .unwrap_or_default()
    }
}
