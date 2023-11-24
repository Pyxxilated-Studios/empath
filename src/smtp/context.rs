use core::slice::SlicePattern;
use std::{collections::HashMap, ffi::CStr, fmt::Debug, sync::Arc};

use mailparse::MailAddr;

use crate::{ffi, internal};

use super::{envelope::Envelope, status::Status};

#[derive(Default, Debug)]
pub struct Context {
    pub extended: bool,
    pub envelope: Envelope,
    pub id: String,
    pub data: Option<Arc<[u8]>>,
    pub response: Option<(Status, String)>,
    #[allow(clippy::struct_field_names)]
    pub context: HashMap<String, String>,
}

impl Context {
    /// Returns a reference to the id of this [`Context`].
    #[inline]
    pub fn id(&self) -> &str {
        &self.id
    }

    #[expect(dead_code)]
    #[inline]
    pub fn message(&self) -> String {
        self.data.as_deref().map_or_else(Default::default, |data| {
            charset::Charset::for_encoding(encoding_rs::UTF_8)
                .decode(data)
                .0
                .to_string()
        })
    }

    /// Returns the sender of this [`Context`].
    #[inline]
    pub fn sender(&self) -> String {
        self.envelope
            .sender()
            .clone()
            .map(|sender| match sender {
                MailAddr::Single(addr) => addr.to_string(),
                MailAddr::Group(_) => String::default(),
            })
            .unwrap_or_default()
    }

    /// Returns the recipients of this [`Context`].
    pub fn recipients(&self) -> Vec<String> {
        self.envelope
            .recipients()
            .clone()
            .map(|addrs| {
                addrs
                    .iter()
                    .map(|addr| match addr {
                        mailparse::MailAddr::Group(group) => {
                            format!("RCPT TO:{}", group.group_name)
                        }
                        mailparse::MailAddr::Single(single) => {
                            format!(
                                "RCPT TO:{}{}",
                                single.display_name.clone().unwrap_or_default(),
                                single.addr
                            )
                        }
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// Retrieve the id associated with this context
///
/// This is the only way to retrieve the id for the context in an
/// ffi-compatible way. Any other way should be retrieved by
/// accessing the id member directly.
///
#[no_mangle]
#[allow(clippy::module_name_repetitions)]
pub extern "C" fn em_context_get_id(validate_context: &Context) -> ffi::string::String {
    validate_context.id().into()
}

#[no_mangle]
#[allow(clippy::module_name_repetitions)]
pub extern "C" fn em_context_get_recipients(
    validate_context: &Context,
) -> ffi::string::StringVector {
    validate_context.recipients().into()
}

#[no_mangle]
#[allow(clippy::module_name_repetitions)]
pub extern "C" fn em_context_get_sender(validate_context: &Context) -> ffi::string::String {
    validate_context.sender().into()
}

///
/// Set the sender for the message. A special value of NULL will set the
/// sender to the NULL Sender.
///
/// # Safety
///
/// This should be able to be passed any valid pointer, and a valid `validate_context`, to
/// set the sender
///
#[no_mangle]
#[allow(clippy::module_name_repetitions)]
pub unsafe extern "C" fn em_context_set_sender(
    validate_context: &mut Context,
    sender: *const libc::c_char,
) -> bool {
    if sender.is_null() {
        *validate_context.envelope.sender_mut() = None;
        return true;
    }

    let sender = CStr::from_ptr(sender);

    match sender.to_str() {
        Ok(sender) => match mailparse::addrparse(sender) {
            Ok(sender) => {
                *validate_context.envelope.sender_mut() = Some(sender[0].clone());
                true
            }
            Err(err) => {
                internal!("Invalid sender: {:?} :: {}", sender, err.to_string());
                false
            }
        },
        Err(err) => {
            internal!("Invalid sender: {:?} :: {}", sender, err.to_string());
            false
        }
    }
}

#[no_mangle]
#[allow(clippy::module_name_repetitions)]
pub extern "C" fn em_context_get_data(validate_context: &Context) -> ffi::string::String {
    validate_context
        .data
        .as_ref()
        .map_or_else(Default::default, |data| {
            ffi::string::String::try_from(data.as_slice()).unwrap_or_default()
        })
}

///
/// # Safety
///
/// Even if provided with a null pointer, that would simply set the response to `None`
///
#[no_mangle]
#[allow(clippy::module_name_repetitions)]
pub unsafe extern "C" fn em_context_set_response(
    validate_context: &mut Context,
    status: u32,
    response: *const libc::c_char,
) -> bool {
    validate_context.response = if response.is_null() {
        None
    } else {
        Some((
            Status::from(status),
            CStr::from_ptr(response)
                .to_owned()
                .to_string_lossy()
                .to_string(),
        ))
    };

    true
}

///
/// # Safety
///
/// Provided with a null pointer, simply return false
///
#[no_mangle]
#[allow(clippy::module_name_repetitions)]
pub unsafe extern "C" fn em_context_exists(
    validate_context: &Context,
    key: *const libc::c_char,
) -> bool {
    if key.is_null() {
        false
    } else {
        CStr::from_ptr(key)
            .to_str()
            .map_or(false, |key| validate_context.context.contains_key(key))
    }
}

///
/// # Safety
///
/// Provided with a null pointer, simply return false
///
#[no_mangle]
#[allow(clippy::module_name_repetitions)]
pub unsafe extern "C" fn em_context_set(
    validate_context: &mut Context,
    key: *const libc::c_char,
    value: *const libc::c_char,
) -> bool {
    if key.is_null() || value.is_null() {
        false
    } else {
        CStr::from_ptr(key).to_str().map_or(false, |key| {
            let value = CStr::from_ptr(value)
                .to_str()
                .map(String::from)
                .unwrap_or_default();
            *validate_context.context.entry(key.to_string()).or_default() = value;
            true
        })
    }
}

///
/// # Safety
///
/// Provided with a null pointer, simply return a default value
///
#[no_mangle]
#[allow(clippy::module_name_repetitions)]
pub unsafe extern "C" fn em_context_get(
    validate_context: &Context,
    key: *const libc::c_char,
) -> ffi::string::String {
    if key.is_null() {
        ffi::string::String::default()
    } else {
        CStr::from_ptr(key)
            .to_str()
            .ok()
            .and_then(|key| validate_context.context.get(key))
            .map_or_else(ffi::string::String::default, std::convert::Into::into)
    }
}

#[cfg(test)]
mod test {
    use std::{
        ffi::{CStr, CString},
        ptr::null,
    };

    use crate::smtp::{
        context::{
            em_context_exists, em_context_get, em_context_get_data, em_context_get_id,
            em_context_get_recipients, em_context_set, em_context_set_response, Context,
        },
        envelope::Envelope,
        status::Status,
    };

    use super::em_context_set_sender;

    macro_rules! cstr {
        ($st:literal) => {
            concat!($st, "\0").as_ptr().cast()
        };
    }

    #[test]
    fn test_id() {
        let validate_context = Context {
            id: String::from("Testing"),
            ..Default::default()
        };

        unsafe {
            let ffi_string = std::mem::ManuallyDrop::new(em_context_get_id(&validate_context));

            assert_eq!(
                CString::from_raw(ffi_string.data.cast_mut()),
                CString::new(validate_context.id()).unwrap()
            );
        }
    }

    #[test]
    fn test_recipients() {
        let mut validate_context = Context::default();

        let mut recipients = mailparse::addrparse("test@gmail.com").unwrap();
        recipients.extend_from_slice(&mailparse::addrparse("test@test.com").unwrap()[..]);
        *validate_context.envelope.recipients_mut() = Some(recipients);

        let buffer = em_context_get_recipients(&validate_context);
        assert_eq!(buffer.len, 2);

        unsafe {
            let data = std::mem::ManuallyDrop::new(Vec::from_raw_parts(
                buffer.data.cast_mut(),
                buffer.len,
                buffer.len,
            ));

            assert_eq!(
                CStr::from_ptr(data[0].data.cast_mut()).to_owned(),
                CString::new("RCPT TO:test@gmail.com").unwrap()
            );

            assert_eq!(
                CStr::from_ptr(data[1].data.cast_mut()).to_owned(),
                CString::new("RCPT TO:test@test.com").unwrap()
            );
        }
    }

    #[test]
    fn test_set_sender() {
        let mut validate_context = Context {
            id: String::from("Testing"),
            envelope: Envelope::default(),
            ..Default::default()
        };

        unsafe {
            assert!(em_context_set_sender(&mut validate_context, cstr!("test@test.com")));
            assert_eq!(
                validate_context.envelope.sender(),
                &Some(mailparse::addrparse("test@test.com").unwrap()[0].clone())
            );
        }
    }

    #[test]
    fn test_null_sender() {
        let mut envelope = Envelope::default();
        *envelope.sender_mut() = Some(mailparse::addrparse("test@test.com").unwrap()[0].clone());

        let mut validate_context =
            Context {
                id: String::from("Testing"),
                envelope,
                ..Default::default()
            };

        unsafe {
            assert!(em_context_set_sender(&mut validate_context, null()));
            assert_eq!(validate_context.envelope.sender(), &None);
        }
    }

    #[test]
    fn test_invalid_sender() {
        let sender = mailparse::addrparse("test@test.com").unwrap();
        let mut envelope = Envelope::default();
        *envelope.sender_mut() = Some(sender[0].clone());

        let mut validate_context =
            Context {
                id: String::from("Testing"),
                envelope,
                ..Default::default()
            };

        unsafe {
            assert!(!em_context_set_sender(&mut validate_context, cstr!("---")));
            assert_eq!(validate_context.envelope.sender(), &Some(sender[0].clone()));
        }
    }

    #[test]
    fn test_data() {
        let data = b"Testing Data".to_vec();

        let validate_context = Context {
            data: Some(data.clone().into()),
            ..Default::default()
        };

        unsafe {
            assert_eq!(
                CStr::from_ptr(em_context_get_data(&validate_context).data).to_owned(),
                CString::from_vec_unchecked(data)
            );
        }

        let validate_context = Context {
            data: None,
            ..Default::default()
        };

        assert_eq!(em_context_get_data(&validate_context).data, null());
    }

    #[test]
    fn test_set_data_response() {
        let mut validate_context = Context::default();

        let ans = unsafe {
            em_context_set_response(
                &mut validate_context,
                Status::Ok.into(),
                cstr!("Test Response"),
            )
        };
        assert!(ans);
        assert_eq!(
            validate_context.response,
            Some((Status::Ok, "Test Response".to_owned()))
        );

        let mut validate_context = Context {
            response: Some((Status::Ok, "Test".to_owned())),
            ..Default::default()
        };

        let ans =
            unsafe { em_context_set_response(&mut validate_context, Status::Ok.into(), null()) };
        assert!(ans);
        assert_eq!(validate_context.response, None);
    }

    #[test]
    fn test_context() {
        let mut validate_context = Context::default();

        let ans = unsafe { em_context_set(&mut validate_context, cstr!("test"), cstr!("true")) };
        unsafe {
            assert!(ans);
            assert!(em_context_exists(&validate_context, cstr!("test")));
            assert_eq!(
                CStr::from_ptr(em_context_get(&validate_context, cstr!("test")).data).to_owned(),
                CString::from_vec_unchecked(b"true".to_vec())
            );
        }

        let ans = unsafe { em_context_set(&mut validate_context, null(), null()) };
        assert!(!ans);

        let ans = unsafe { em_context_set(&mut validate_context, cstr!("null"), null()) };
        unsafe {
            assert!(!ans);
            assert!(!em_context_exists(&validate_context, cstr!("null")));
        }

        unsafe {
            assert!(!em_context_exists(&validate_context, null()));
        }
    }
}
