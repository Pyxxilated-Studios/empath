#![feature(bstr, slice_pattern, result_option_map_or_default)]

use core::slice::SlicePattern;
use std::{borrow::Cow, ffi::CStr};

use empath_common::{address::Address, context::Context, status::Status};

pub mod modules;
pub mod string;

pub type InitFunc = unsafe fn() -> isize;
pub type ValidateData = unsafe fn(&Context) -> isize;

/// Retrieve the id associated with this context
///
/// This is the only way to retrieve the id for the context in an
/// ffi-compatible way. Any other way should be retrieved by
/// accessing the id member directly.
///
#[unsafe(no_mangle)]
#[allow(clippy::module_name_repetitions)]
pub extern "C" fn em_context_get_id(validate_context: &Context) -> crate::string::String {
    validate_context.id().into()
}

#[unsafe(no_mangle)]
#[allow(clippy::module_name_repetitions)]
pub extern "C" fn em_context_get_recipients(
    validate_context: &Context,
) -> crate::string::StringVector {
    validate_context.recipients().into()
}

#[unsafe(no_mangle)]
#[allow(clippy::module_name_repetitions)]
pub extern "C" fn em_context_get_sender(validate_context: &Context) -> crate::string::String {
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
#[unsafe(no_mangle)]
#[allow(clippy::module_name_repetitions)]
pub unsafe extern "C" fn em_context_set_sender(
    validate_context: &mut Context,
    sender: *const libc::c_char,
) -> bool {
    if sender.is_null() {
        *validate_context.envelope.sender_mut() = None;
        return true;
    }

    let sender = unsafe { CStr::from_ptr(sender) };

    match sender.to_str() {
        Ok(sender) => match empath_common::address_parser::parse_forward_path(sender) {
            Ok(mailbox) => {
                *validate_context.envelope.sender_mut() = Some(Address::from(mailbox));
                true
            }
            Err(_err) => false,
        },
        Err(_err) => false,
    }
}

#[unsafe(no_mangle)]
#[allow(clippy::module_name_repetitions)]
pub extern "C" fn em_context_get_data(validate_context: &Context) -> crate::string::String {
    validate_context
        .data
        .as_ref()
        .map_or_else(Default::default, |data| {
            crate::string::String::try_from(data.as_slice()).unwrap_or_default()
        })
}

///
/// # Safety
///
/// Even if provided with a null pointer, that would simply set the response to `None`
///
#[unsafe(no_mangle)]
#[allow(clippy::module_name_repetitions)]
pub unsafe extern "C" fn em_context_set_response(
    validate_context: &mut Context,
    status: u32,
    response: *const libc::c_char,
) -> bool {
    validate_context.response = if response.is_null() {
        None
    } else {
        Some((Status::from(status), unsafe {
            Cow::Owned(
                CStr::from_ptr(response)
                    .to_owned()
                    .to_string_lossy()
                    .to_string(),
            )
        }))
    };

    true
}

#[unsafe(no_mangle)]
#[allow(clippy::module_name_repetitions)]
pub extern "C" fn em_context_is_tls(validate_context: &Context) -> bool {
    validate_context.metadata.contains_key("tls")
}

#[unsafe(no_mangle)]
#[allow(clippy::module_name_repetitions)]
pub extern "C" fn em_context_tls_protocol(validate_context: &Context) -> crate::string::String {
    validate_context
        .metadata
        .get("protocol")
        .map_or_default(crate::string::String::from)
}

#[unsafe(no_mangle)]
#[allow(clippy::module_name_repetitions)]
pub extern "C" fn em_context_tls_cipher(validate_context: &Context) -> crate::string::String {
    validate_context
        .metadata
        .get("cipher")
        .map_or_default(crate::string::String::from)
}

///
/// # Safety
///
/// Provided with a null pointer, simply return false
///
#[unsafe(no_mangle)]
#[allow(clippy::module_name_repetitions)]
pub unsafe extern "C" fn em_context_exists(
    validate_context: &Context,
    key: *const libc::c_char,
) -> bool {
    if key.is_null() {
        false
    } else {
        unsafe {
            CStr::from_ptr(key)
                .to_str()
                .is_ok_and(|key| validate_context.metadata.contains_key(key))
        }
    }
}

///
/// # Safety
///
/// Provided with a null pointer, simply return false
///
#[unsafe(no_mangle)]
#[allow(clippy::module_name_repetitions)]
pub unsafe extern "C" fn em_context_set(
    validate_context: &mut Context,
    key: *const libc::c_char,
    value: *const libc::c_char,
) -> bool {
    if key.is_null() || value.is_null() {
        false
    } else {
        unsafe {
            CStr::from_ptr(key).to_str().is_ok_and(|key| {
                let value = CStr::from_ptr(value).to_str().map_or_default(String::from);
                *validate_context
                    .metadata
                    .entry(Cow::Owned(key.to_string()))
                    .or_default() = value;
                true
            })
        }
    }
}

///
/// # Safety
///
/// Provided with a null pointer, simply return a default value
///
#[unsafe(no_mangle)]
#[allow(clippy::module_name_repetitions)]
pub unsafe extern "C" fn em_context_get(
    validate_context: &Context,
    key: *const libc::c_char,
) -> crate::string::String {
    if key.is_null() {
        crate::string::String::default()
    } else {
        unsafe {
            CStr::from_ptr(key)
                .to_str()
                .ok()
                .and_then(|key| validate_context.metadata.get(key))
                .map_or_else(crate::string::String::default, std::convert::Into::into)
        }
    }
}

// ============================================================================
// Delivery Context Accessors
// ============================================================================

/// Check if delivery context is present
#[unsafe(no_mangle)]
#[allow(clippy::module_name_repetitions)]
pub const extern "C" fn em_context_has_delivery(validate_context: &Context) -> bool {
    validate_context.delivery.is_some()
}

/// Get the message ID from delivery context
#[unsafe(no_mangle)]
#[allow(clippy::module_name_repetitions)]
pub extern "C" fn em_delivery_get_message_id(validate_context: &Context) -> crate::string::String {
    validate_context
        .delivery
        .as_ref()
        .map_or_else(crate::string::String::default, |d| {
            d.message_id.as_str().into()
        })
}

/// Get the delivery domain from delivery context
#[unsafe(no_mangle)]
#[allow(clippy::module_name_repetitions)]
pub extern "C" fn em_delivery_get_domain(validate_context: &Context) -> crate::string::String {
    validate_context
        .delivery
        .as_ref()
        .map_or_else(crate::string::String::default, |d| d.domain.as_ref().into())
}

/// Get the delivery server from delivery context
#[unsafe(no_mangle)]
#[allow(clippy::module_name_repetitions)]
pub extern "C" fn em_delivery_get_server(validate_context: &Context) -> crate::string::String {
    validate_context
        .delivery
        .as_ref()
        .map_or_else(crate::string::String::default, |d| {
            d.server
                .as_ref()
                .map_or_else(crate::string::String::default, |s| s.as_str().into())
        })
}

/// Get the delivery error from delivery context
#[unsafe(no_mangle)]
#[allow(clippy::module_name_repetitions)]
pub extern "C" fn em_delivery_get_error(validate_context: &Context) -> crate::string::String {
    validate_context
        .delivery
        .as_ref()
        .map_or_else(crate::string::String::default, |d| {
            d.error
                .as_ref()
                .map_or_else(crate::string::String::default, |e| e.as_str().into())
        })
}

/// Get the delivery attempt count from delivery context
/// Returns 0 if no delivery context or no attempt count is set
#[unsafe(no_mangle)]
#[allow(clippy::module_name_repetitions)]
pub extern "C" fn em_delivery_get_attempts(validate_context: &Context) -> u32 {
    validate_context
        .delivery
        .as_ref()
        .and_then(|d| d.attempts)
        .unwrap_or(0)
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod test {
    use std::{
        borrow::Cow,
        ffi::{CStr, CString},
        ptr::null,
    };

    use empath_common::{
        address::{Address, AddressList},
        context::Context,
        envelope::Envelope,
        status::Status,
    };

    use super::{
        em_context_exists, em_context_get, em_context_get_data, em_context_get_id,
        em_context_get_recipients, em_context_set, em_context_set_response, em_context_set_sender,
    };

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

        let recipient1 =
            empath_common::address_parser::parse_forward_path("<test@gmail.com>").unwrap();
        let recipient2 =
            empath_common::address_parser::parse_forward_path("<test@test.com>").unwrap();
        *validate_context.envelope.recipients_mut() = Some(AddressList(vec![
            Address::from(recipient1),
            Address::from(recipient2),
        ]));

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
            assert!(em_context_set_sender(
                &mut validate_context,
                cstr!("<test@test.com>")
            ));
            let expected_mailbox =
                empath_common::address_parser::parse_forward_path("<test@test.com>").unwrap();
            assert_eq!(
                validate_context.envelope.sender(),
                Some(&Address::from(expected_mailbox))
            );
        }
    }

    #[test]
    fn test_null_sender() {
        let mut envelope = Envelope::default();
        let mailbox = empath_common::address_parser::parse_forward_path("<test@test.com>").unwrap();
        *envelope.sender_mut() = Some(Address::from(mailbox));

        let mut validate_context = Context {
            id: String::from("Testing"),
            envelope,
            ..Default::default()
        };

        unsafe {
            assert!(em_context_set_sender(&mut validate_context, null()));
            assert_eq!(validate_context.envelope.sender(), None);
        }
    }

    #[test]
    fn test_invalid_sender() {
        let mailbox = empath_common::address_parser::parse_forward_path("<test@test.com>").unwrap();
        let original_sender = Address::from(mailbox);
        let mut envelope = Envelope::default();
        *envelope.sender_mut() = Some(original_sender.clone());

        let mut validate_context = Context {
            id: String::from("Testing"),
            envelope,
            ..Default::default()
        };

        unsafe {
            assert!(!em_context_set_sender(&mut validate_context, cstr!("---")));
            assert_eq!(validate_context.envelope.sender(), Some(&original_sender));
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
            Some((Status::Ok, Cow::Owned("Test Response".to_owned())))
        );

        let mut validate_context = Context {
            response: Some((Status::Ok, Cow::Owned("Test".to_owned()))),
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
