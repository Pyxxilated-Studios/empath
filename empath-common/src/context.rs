use core::slice::SlicePattern;
use std::{collections::HashMap, ffi::CStr, fmt::Debug, sync::Arc};

use mailparse::MailAddrList;

use crate::{ffi, internal};

#[derive(Default, Debug)]
pub struct Context {
    pub id: String,
    pub mail_from: Option<MailAddrList>,
    pub rcpt_to: Option<MailAddrList>,
    pub data: Option<Arc<[u8]>>,
    pub data_response: Option<String>,
    pub context: HashMap<String, String>,
}

impl Context {
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn message(&self) -> String {
        self.data.as_deref().map_or_else(Default::default, |data| {
            std::str::from_utf8(data).map_or_else(|_| format!("{:#?}", self.data), str::to_string)
        })
    }

    #[must_use]
    pub fn sender(&self) -> String {
        self.mail_from
            .clone()
            .map(|sender| sender.to_string())
            .unwrap_or_default()
    }

    #[must_use]
    pub fn recipients(&self) -> Vec<String> {
        self.rcpt_to
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
) -> i32 {
    if sender.is_null() {
        validate_context.mail_from = None;
        return 0;
    }

    let sender = CStr::from_ptr(sender);

    match sender.to_str() {
        Ok(sender) => match mailparse::addrparse(sender) {
            Ok(sender) => {
                validate_context.mail_from = Some(sender);
                0
            }
            Err(err) => {
                internal!("Invalid sender: {:?} :: {}", sender, err.to_string());
                1
            }
        },
        Err(err) => {
            internal!("Invalid sender: {:?} :: {}", sender, err.to_string());
            1
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
pub unsafe extern "C" fn em_context_set_data_response(
    validate_context: &mut Context,
    response: *const libc::c_char,
) -> i32 {
    if response.is_null() {
        validate_context.data_response = None;
    } else {
        let response = CStr::from_ptr(response);
        validate_context.data_response = Some(response.to_owned().to_string_lossy().to_string());
    }

    0
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

    use crate::context::{
        em_context_exists, em_context_get, em_context_get_data, em_context_get_id,
        em_context_get_recipients, em_context_set, em_context_set_data_response, Context,
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
        validate_context.rcpt_to = Some(recipients);

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
            mail_from: None,
            ..Default::default()
        };

        unsafe {
            assert_eq!(
                em_context_set_sender(&mut validate_context, cstr!("test@test.com")),
                0
            );
            assert_eq!(
                validate_context.mail_from,
                Some(mailparse::addrparse("test@test.com").unwrap())
            );
        }
    }

    #[test]
    fn test_null_sender() {
        let mut validate_context = Context {
            id: String::from("Testing"),
            mail_from: Some(mailparse::addrparse("test@test.com").unwrap()),
            ..Default::default()
        };

        unsafe {
            assert_eq!(em_context_set_sender(&mut validate_context, null()), 0);
            assert_eq!(validate_context.mail_from, None);
        }
    }

    #[test]
    fn test_invalid_sender() {
        let sender = mailparse::addrparse("test@test.com").unwrap();

        let mut validate_context = Context {
            id: String::from("Testing"),
            mail_from: Some(sender.clone()),
            ..Default::default()
        };

        unsafe {
            assert_eq!(
                em_context_set_sender(&mut validate_context, cstr!("---")),
                1
            );
            assert_eq!(validate_context.mail_from, Some(sender));
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

        let ans =
            unsafe { em_context_set_data_response(&mut validate_context, cstr!("Test Response")) };
        assert_eq!(ans, 0);
        assert_eq!(
            validate_context.data_response,
            Some("Test Response".to_string())
        );

        let mut validate_context = Context {
            data_response: Some("Test".to_string()),
            ..Default::default()
        };

        let ans = unsafe { em_context_set_data_response(&mut validate_context, null()) };
        assert_eq!(ans, 0);
        assert_eq!(validate_context.data_response, None);
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
