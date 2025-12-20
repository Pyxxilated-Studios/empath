use std::{borrow::Cow, ffi::CStr, sync::Arc};

use empath_common::{
    address::Address,
    context::Context,
    message_api::{
        BodyLimits, BodyOpts, Charset, HeaderLimits, HeaderName, HeaderValue, NewlineStyle,
    },
    status::Status,
};

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
        *Arc::make_mut(&mut validate_context.envelope).sender_mut() = None;
        return true;
    }

    let sender = unsafe { CStr::from_ptr(sender) };

    match sender.to_str() {
        Ok(sender) => match empath_common::address_parser::parse_forward_path(sender) {
            Ok(mailbox) => {
                *Arc::make_mut(&mut validate_context.envelope).sender_mut() =
                    Some(Address::from(mailbox));
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
            crate::string::String::try_from(data.as_ref()).unwrap_or_default()
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
    // TLS state is session context
    validate_context.session.contains_key("tls")
}

#[unsafe(no_mangle)]
#[allow(clippy::module_name_repetitions)]
pub extern "C" fn em_context_tls_protocol(validate_context: &Context) -> crate::string::String {
    // TLS protocol is session context
    validate_context
        .session
        .get("protocol")
        .map_or_else(crate::string::String::default, crate::string::String::from)
}

#[unsafe(no_mangle)]
#[allow(clippy::module_name_repetitions)]
pub extern "C" fn em_context_tls_cipher(validate_context: &Context) -> crate::string::String {
    // TLS cipher is session context
    validate_context
        .session
        .get("cipher")
        .map_or_else(crate::string::String::default, crate::string::String::from)
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
                // Default to transaction context for backward compatibility
                .is_ok_and(|key| validate_context.transaction.contains_key(key))
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
                let value = CStr::from_ptr(value)
                    .to_str()
                    .map_or_else(|_| String::new(), String::from);
                // Default to transaction context for backward compatibility
                *validate_context
                    .transaction
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
                .and_then(|key| validate_context.transaction.get(key))
                .map_or_else(crate::string::String::default, std::convert::Into::into)
        }
    }
}

fn header_name_from_cstr(name: &CStr) -> Option<HeaderName> {
    name.to_str().ok().and_then(|n| HeaderName::new(n).ok())
}

// ============================================================================
// Session Context Functions
// ============================================================================

/// Set session context (persists across RSET)
///
/// # Safety
///
/// Provided with a null pointer, simply return false
#[unsafe(no_mangle)]
#[allow(clippy::module_name_repetitions)]
pub unsafe extern "C" fn em_context_session_set(
    validate_context: &mut Context,
    key: *const libc::c_char,
    value: *const libc::c_char,
) -> bool {
    if key.is_null() || value.is_null() {
        false
    } else {
        unsafe {
            CStr::from_ptr(key).to_str().is_ok_and(|key| {
                let value = CStr::from_ptr(value)
                    .to_str()
                    .map_or_else(|_| String::new(), String::from);
                *validate_context
                    .session
                    .entry(Cow::Owned(key.to_string()))
                    .or_default() = value;
                true
            })
        }
    }
}

/// Get session context
///
/// # Safety
///
/// Provided with a null pointer, simply return a default value
#[unsafe(no_mangle)]
#[allow(clippy::module_name_repetitions)]
pub unsafe extern "C" fn em_context_session_get(
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
                .and_then(|key| validate_context.session.get(key))
                .map_or_else(crate::string::String::default, std::convert::Into::into)
        }
    }
}

/// Check if session context key exists
///
/// # Safety
///
/// Provided with a null pointer, simply return false
#[unsafe(no_mangle)]
#[allow(clippy::module_name_repetitions)]
pub unsafe extern "C" fn em_context_session_exists(
    validate_context: &Context,
    key: *const libc::c_char,
) -> bool {
    if key.is_null() {
        false
    } else {
        unsafe {
            CStr::from_ptr(key)
                .to_str()
                .is_ok_and(|key| validate_context.session.contains_key(key))
        }
    }
}

// ============================================================================
// Transaction Context Functions
// ============================================================================

/// Set transaction context (cleared on RSET)
///
/// # Safety
///
/// Provided with a null pointer, simply return false
#[unsafe(no_mangle)]
#[allow(clippy::module_name_repetitions)]
pub unsafe extern "C" fn em_context_transaction_set(
    validate_context: &mut Context,
    key: *const libc::c_char,
    value: *const libc::c_char,
) -> bool {
    if key.is_null() || value.is_null() {
        false
    } else {
        unsafe {
            CStr::from_ptr(key).to_str().is_ok_and(|key| {
                let value = CStr::from_ptr(value)
                    .to_str()
                    .map_or_else(|_| String::new(), String::from);
                *validate_context
                    .transaction
                    .entry(Cow::Owned(key.to_string()))
                    .or_default() = value;
                true
            })
        }
    }
}

/// Get transaction context
///
/// # Safety
///
/// Provided with a null pointer, simply return a default value
#[unsafe(no_mangle)]
#[allow(clippy::module_name_repetitions)]
pub unsafe extern "C" fn em_context_transaction_get(
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
                .and_then(|key| validate_context.transaction.get(key))
                .map_or_else(crate::string::String::default, std::convert::Into::into)
        }
    }
}

/// Check if transaction context key exists
///
/// # Safety
///
/// Provided with a null pointer, simply return false
#[unsafe(no_mangle)]
#[allow(clippy::module_name_repetitions)]
pub unsafe extern "C" fn em_context_transaction_exists(
    validate_context: &Context,
    key: *const libc::c_char,
) -> bool {
    if key.is_null() {
        false
    } else {
        unsafe {
            CStr::from_ptr(key)
                .to_str()
                .is_ok_and(|key| validate_context.transaction.contains_key(key))
        }
    }
}

// ============================================================================
// Message mutation helpers (headers + body)
// ============================================================================

/// # Safety
/// `validate_context` must point to a valid context, and `name` must be a valid
/// null-terminated C string when non-null.
#[unsafe(no_mangle)]
#[allow(clippy::module_name_repetitions)]
pub unsafe extern "C" fn em_message_headers_get(
    validate_context: &mut Context,
    name: *const libc::c_char,
) -> crate::string::StringVector {
    if name.is_null() {
        return crate::string::StringVector::default();
    }

    let name = unsafe { CStr::from_ptr(name) };
    let Some(hname) = header_name_from_cstr(name) else {
        return crate::string::StringVector::default();
    };

    let values = match validate_context.with_message_parts_mut(|parts| {
        let view = parts.headers_view();
        view.get_all(&hname)
            .map(|v| String::from_utf8_lossy(v.as_bytes()).to_string())
            .collect::<Vec<_>>()
    }) {
        Ok(Some(vals)) => vals,
        Ok(None) | Err(_) => Vec::new(),
    };

    values.into()
}

/// # Safety
/// `validate_context` must point to a valid context, and `name` must be a valid
/// null-terminated C string when non-null.
#[unsafe(no_mangle)]
#[allow(clippy::module_name_repetitions)]
pub unsafe extern "C" fn em_message_headers_contains(
    validate_context: &mut Context,
    name: *const libc::c_char,
) -> bool {
    if name.is_null() {
        return false;
    }

    let name = unsafe { CStr::from_ptr(name) };
    let Some(hname) = header_name_from_cstr(name) else {
        return false;
    };

    match validate_context.with_message_parts_mut(|parts| parts.headers_view().contains(&hname)) {
        Ok(Some(result)) => result,
        Ok(None) | Err(_) => false,
    }
}

/// # Safety
/// `validate_context` must point to a valid context. `name` and `value` must
/// point to valid null-terminated C strings when non-null.
#[unsafe(no_mangle)]
#[allow(clippy::module_name_repetitions)]
pub unsafe extern "C" fn em_message_headers_set(
    validate_context: &mut Context,
    name: *const libc::c_char,
    value: *const libc::c_char,
) -> bool {
    if name.is_null() || value.is_null() {
        return false;
    }

    let name = unsafe { CStr::from_ptr(name) };
    let value = unsafe { CStr::from_ptr(value) };

    let Some(hname) = header_name_from_cstr(name) else {
        return false;
    };
    let Some(hval) = HeaderValue::try_from(value).ok() else {
        return false;
    };

    matches!(
        validate_context.with_message_parts_mut(|parts| {
            let mut headers = parts.headers_mut(HeaderLimits::default(), None);
            headers.set(&hname, &hval)
        }),
        Ok(Some(Ok(())))
    )
}

/// # Safety
/// `validate_context` must point to a valid context. `name` and `value` must
/// point to valid null-terminated C strings when non-null.
#[unsafe(no_mangle)]
#[allow(clippy::module_name_repetitions)]
pub unsafe extern "C" fn em_message_headers_append(
    validate_context: &mut Context,
    name: *const libc::c_char,
    value: *const libc::c_char,
) -> bool {
    if name.is_null() || value.is_null() {
        return false;
    }

    let name = unsafe { CStr::from_ptr(name) };
    let value = unsafe { CStr::from_ptr(value) };

    let Some(hname) = header_name_from_cstr(name) else {
        return false;
    };
    let Some(hval) = HeaderValue::try_from(value).ok() else {
        return false;
    };

    matches!(
        validate_context.with_message_parts_mut(|parts| {
            let mut headers = parts.headers_mut(HeaderLimits::default(), None);
            headers.append(&hname, &hval)
        }),
        Ok(Some(Ok(())))
    )
}

/// # Safety
/// `validate_context` must point to a valid context. `name` and `value` must
/// point to valid null-terminated C strings when non-null.
#[unsafe(no_mangle)]
#[allow(clippy::module_name_repetitions)]
pub unsafe extern "C" fn em_message_headers_set_if_absent(
    validate_context: &mut Context,
    name: *const libc::c_char,
    value: *const libc::c_char,
) -> bool {
    if name.is_null() || value.is_null() {
        return false;
    }

    let name = unsafe { CStr::from_ptr(name) };
    let value = unsafe { CStr::from_ptr(value) };

    let Some(hname) = header_name_from_cstr(name) else {
        return false;
    };
    let Some(hval) = HeaderValue::try_from(value).ok() else {
        return false;
    };

    match validate_context.with_message_parts_mut(|parts| {
        let mut headers = parts.headers_mut(HeaderLimits::default(), None);
        headers.set_if_absent(&hname, &hval)
    }) {
        Ok(Some(Ok(updated))) => updated,
        _ => false,
    }
}

/// # Safety
/// `validate_context` must point to a valid context. `name` must point to a
/// valid null-terminated C string when non-null.
#[unsafe(no_mangle)]
#[allow(clippy::module_name_repetitions)]
pub unsafe extern "C" fn em_message_headers_remove(
    validate_context: &mut Context,
    name: *const libc::c_char,
) -> u32 {
    if name.is_null() {
        return 0;
    }

    let name = unsafe { CStr::from_ptr(name) };
    let Some(hname) = header_name_from_cstr(name) else {
        return 0;
    };

    match validate_context.with_message_parts_mut(|parts| {
        let mut headers = parts.headers_mut(HeaderLimits::default(), None);
        u32::try_from(headers.remove(&hname)).unwrap_or(u32::MAX)
    }) {
        Ok(Some(count)) => count,
        Ok(None) | Err(_) => 0,
    }
}

/// # Safety
/// `validate_context` must point to a valid context.
#[unsafe(no_mangle)]
#[allow(clippy::module_name_repetitions)]
pub unsafe extern "C" fn em_message_headers_ensure_message_id(
    validate_context: &mut Context,
) -> bool {
    match validate_context.with_message_parts_mut(|parts| {
        let mut headers = parts.headers_mut(HeaderLimits::default(), None);
        headers.ensure_message_id().map(|()| true)
    }) {
        Ok(Some(Ok(result))) => result,
        _ => false,
    }
}

/// # Safety
/// `validate_context` must point to a valid context. `value` must point to a
/// valid null-terminated C string when non-null.
#[unsafe(no_mangle)]
#[allow(clippy::module_name_repetitions)]
pub unsafe extern "C" fn em_message_headers_set_content_type(
    validate_context: &mut Context,
    value: *const libc::c_char,
) -> bool {
    if value.is_null() {
        return false;
    }
    let value = unsafe { CStr::from_ptr(value) };
    let Some(hval) = HeaderValue::try_from(value).ok() else {
        return false;
    };
    let Ok(hname) = HeaderName::new("Content-Type") else {
        return false;
    };

    matches!(
        validate_context.with_message_parts_mut(|parts| {
            let mut headers = parts.headers_mut(HeaderLimits::default(), None);
            headers.set(&hname, &hval)
        }),
        Ok(Some(Ok(())))
    )
}

/// # Safety
/// `validate_context` must point to a valid context. `value` must point to a
/// valid null-terminated C string when non-null.
#[unsafe(no_mangle)]
#[allow(clippy::module_name_repetitions)]
pub unsafe extern "C" fn em_message_headers_set_transfer_encoding(
    validate_context: &mut Context,
    value: *const libc::c_char,
) -> bool {
    if value.is_null() {
        return false;
    }
    let value = unsafe { CStr::from_ptr(value) };
    let Some(hval) = HeaderValue::try_from(value).ok() else {
        return false;
    };
    let Ok(hname) = HeaderName::new("Content-Transfer-Encoding") else {
        return false;
    };

    matches!(
        validate_context.with_message_parts_mut(|parts| {
            let mut headers = parts.headers_mut(HeaderLimits::default(), None);
            headers.set(&hname, &hval)
        }),
        Ok(Some(Ok(())))
    )
}

fn mutate_body<F>(validate_context: &mut Context, f: F) -> bool
where
    F: FnMut(
        &mut empath_common::message_api::BodyMut<'_>,
    ) -> Result<(), empath_common::message_api::BodyError>,
{
    let mut op = f;
    matches!(
        validate_context.with_message_parts_mut(|parts| {
            let mut body = parts.body_mut(BodyLimits::default(), None);
            op(&mut body)
        }),
        Ok(Some(Ok(())))
    )
}

/// # Safety
/// `validate_context` must point to a valid context. If `data` is non-null, it
/// must reference `len` bytes of valid memory.
#[unsafe(no_mangle)]
#[allow(clippy::module_name_repetitions)]
pub unsafe extern "C" fn em_message_body_set_bytes(
    validate_context: &mut Context,
    data: *const u8,
    len: usize,
) -> bool {
    let slice = if data.is_null() {
        &[]
    } else {
        // SAFETY: Caller guarantees `data` is valid for `len` bytes.
        unsafe { std::slice::from_raw_parts(data, len) }
    };
    mutate_body(validate_context, |body| {
        body.set_bytes(slice, BodyOpts::default())
    })
}

/// # Safety
/// `validate_context` must point to a valid context. If `data` is non-null, it
/// must reference `len` bytes of valid memory.
#[unsafe(no_mangle)]
#[allow(clippy::module_name_repetitions)]
pub unsafe extern "C" fn em_message_body_append_bytes(
    validate_context: &mut Context,
    data: *const u8,
    len: usize,
) -> bool {
    let slice = if data.is_null() {
        &[]
    } else {
        // SAFETY: Caller guarantees `data` is valid for `len` bytes.
        unsafe { std::slice::from_raw_parts(data, len) }
    };
    mutate_body(validate_context, |body| {
        body.append_bytes(slice, BodyOpts::default())
    })
}

/// # Safety
/// `validate_context` must point to a valid context. `text` and `charset`
/// pointers must be valid null-terminated C strings when non-null.
#[unsafe(no_mangle)]
#[allow(clippy::module_name_repetitions)]
pub unsafe extern "C" fn em_message_body_set_text(
    validate_context: &mut Context,
    text: *const libc::c_char,
    charset: *const libc::c_char,
) -> bool {
    if text.is_null() {
        return false;
    }
    let text = unsafe { CStr::from_ptr(text) };
    let charset = if charset.is_null() {
        None
    } else {
        Some(unsafe { CStr::from_ptr(charset) })
    };
    let charset = Charset::from(charset);

    text.to_str().ok().is_some_and(|txt| {
        mutate_body(validate_context, |body| {
            body.set_text(txt, &charset, BodyOpts::default())
        })
    })
}

/// # Safety
/// `validate_context` must point to a valid context. `text` and `charset`
/// pointers must be valid null-terminated C strings when non-null.
#[unsafe(no_mangle)]
#[allow(clippy::module_name_repetitions)]
pub unsafe extern "C" fn em_message_body_append_text(
    validate_context: &mut Context,
    text: *const libc::c_char,
    charset: *const libc::c_char,
) -> bool {
    if text.is_null() {
        return false;
    }
    let text = unsafe { CStr::from_ptr(text) };
    let charset = if charset.is_null() {
        None
    } else {
        Some(unsafe { CStr::from_ptr(charset) })
    };
    let charset = Charset::from(charset);

    text.to_str().ok().is_some_and(|txt| {
        mutate_body(validate_context, |body| {
            body.append_text(txt, &charset, BodyOpts::default())
        })
    })
}

/// # Safety
/// `validate_context` must point to a valid context. `html` and `charset`
/// pointers must be valid null-terminated C strings when non-null.
#[unsafe(no_mangle)]
#[allow(clippy::module_name_repetitions)]
pub unsafe extern "C" fn em_message_body_set_html(
    validate_context: &mut Context,
    html: *const libc::c_char,
    charset: *const libc::c_char,
) -> bool {
    if html.is_null() {
        return false;
    }
    let html = unsafe { CStr::from_ptr(html) };
    let charset = if charset.is_null() {
        None
    } else {
        Some(unsafe { CStr::from_ptr(charset) })
    };
    let charset = Charset::from(charset);

    html.to_str().ok().is_some_and(|txt| {
        mutate_body(validate_context, |body| {
            body.set_html(txt, &charset, BodyOpts::default())
        })
    })
}

/// # Safety
/// `validate_context` must point to a valid context. `html` and `charset`
/// pointers must be valid null-terminated C strings when non-null.
#[unsafe(no_mangle)]
#[allow(clippy::module_name_repetitions)]
pub unsafe extern "C" fn em_message_body_append_html(
    validate_context: &mut Context,
    html: *const libc::c_char,
    charset: *const libc::c_char,
) -> bool {
    if html.is_null() {
        return false;
    }
    let html = unsafe { CStr::from_ptr(html) };
    let charset = if charset.is_null() {
        None
    } else {
        Some(unsafe { CStr::from_ptr(charset) })
    };
    let charset = Charset::from(charset);

    html.to_str().ok().is_some_and(|txt| {
        mutate_body(validate_context, |body| {
            body.append_html(txt, &charset, BodyOpts::default())
        })
    })
}

/// # Safety
/// `validate_context` must point to a valid context. `text`, `html`, and
/// `charset` pointers must be valid null-terminated C strings when non-null.
#[unsafe(no_mangle)]
#[allow(clippy::module_name_repetitions)]
pub unsafe extern "C" fn em_message_body_set_alternative(
    validate_context: &mut Context,
    text: *const libc::c_char,
    html: *const libc::c_char,
    charset: *const libc::c_char,
) -> bool {
    if text.is_null() || html.is_null() {
        return false;
    }
    let text = unsafe { CStr::from_ptr(text) };
    let html = unsafe { CStr::from_ptr(html) };
    let charset = if charset.is_null() {
        None
    } else {
        Some(unsafe { CStr::from_ptr(charset) })
    };
    let charset = Charset::from(charset);

    match (text.to_str(), html.to_str()) {
        (Ok(text_str), Ok(html_str)) => mutate_body(validate_context, |body| {
            body.set_alternative(text_str, html_str, &charset, BodyOpts::default())
        }),
        _ => false,
    }
}

/// # Safety
/// `validate_context` must point to a valid context.
#[unsafe(no_mangle)]
#[allow(clippy::module_name_repetitions)]
pub unsafe extern "C" fn em_message_body_newline_style(
    validate_context: &mut Context,
) -> crate::string::String {
    let style =
        match validate_context.with_message_parts_mut(|parts| parts.body_view().newline_style()) {
            Ok(Some(style)) => style,
            Ok(None) | Err(_) => NewlineStyle::Lf,
        };

    let name = match style {
        NewlineStyle::Crlf => "crlf",
        NewlineStyle::Lf => "lf",
        NewlineStyle::Mixed => "mixed",
    };
    name.into()
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
            d.message_id.to_string().into()
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
        sync::Arc,
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
        *Arc::make_mut(&mut validate_context.envelope).recipients_mut() = Some(AddressList(vec![
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
            envelope: Arc::new(Envelope::default()),
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
            envelope: Arc::new(envelope),
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
            envelope: Arc::new(envelope),
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
