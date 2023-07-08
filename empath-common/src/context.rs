use std::{ffi::CStr, fmt::Debug};

use mailparse::MailAddrList;

use crate::{ffi, internal};

#[derive(Default, Debug)]
pub struct Context {
    pub id: String,
    pub mail_from: Option<MailAddrList>,
    pub rcpt_to: Option<MailAddrList>,
    pub data: Option<Vec<u8>>,
    pub data_response: Option<String>,
}

impl Context {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn message(&self) -> String {
        self.data.as_deref().map_or_else(Default::default, |data| {
            std::str::from_utf8(data).map_or_else(|_| format!("{:#?}", self.data), str::to_string)
        })
    }

    pub fn sender(&self) -> String {
        self.mail_from
            .clone()
            .map(|sender| sender.to_string())
            .unwrap_or_default()
    }

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
pub extern "C" fn context_get_id(vctx: &Context) -> ffi::string::String {
    vctx.id().into()
}

#[no_mangle]
#[allow(clippy::module_name_repetitions)]
pub extern "C" fn context_get_recipients(vctx: &Context) -> ffi::string::StringVector {
    vctx.recipients().into()
}

#[no_mangle]
#[allow(clippy::module_name_repetitions)]
pub extern "C" fn context_get_sender(vctx: &Context) -> ffi::string::String {
    vctx.sender().into()
}

///
/// Set the sender for the message. A special value of NULL will set the
/// sender to the NULL Sender.
///
/// # Safety
///
/// This should be able to be passed any valid pointer, and a valid vctx, to
/// set the sender
///
#[no_mangle]
#[allow(clippy::module_name_repetitions)]
pub unsafe extern "C" fn context_set_sender(
    vctx: &mut Context,
    sender: *const libc::c_char,
) -> i32 {
    if sender.is_null() {
        vctx.mail_from = None;
        return 0;
    }

    let sender = CStr::from_ptr(sender);

    match sender.to_str() {
        Ok(sender) => match mailparse::addrparse(sender) {
            Ok(sender) => {
                vctx.mail_from = Some(sender);
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
pub extern "C" fn context_get_data(vctx: &Context) -> ffi::string::String {
    vctx.data.as_ref().map_or_else(Default::default, |data| {
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
pub unsafe extern "C" fn context_set_data_response(
    vctx: &mut Context,
    response: *const libc::c_char,
) -> i32 {
    if response.is_null() {
        vctx.data_response = None;
    } else {
        let response = CStr::from_ptr(response);
        vctx.data_response = Some(response.to_owned().to_string_lossy().to_string());
    }

    0
}

#[cfg(test)]
mod test {
    use crate::context::{
        context_get_data, context_get_id, context_get_recipients, context_set_data_response,
        Context,
    };
    use std::{
        ffi::{CStr, CString},
        ptr::null,
    };

    use super::context_set_sender;

    macro_rules! cstr {
        ($st:literal) => {
            concat!($st, "\0").as_ptr().cast()
        };
    }

    #[test]
    fn test_id() {
        let vctx = Context {
            id: String::from("Testing"),
            ..Default::default()
        };

        unsafe {
            let ffi_string = std::mem::ManuallyDrop::new(context_get_id(&vctx));

            assert_eq!(
                CString::from_raw(ffi_string.data.cast_mut()),
                CString::new(vctx.id()).unwrap()
            );
        }
    }

    #[test]
    fn test_recipients() {
        let mut vctx = Context::default();

        let mut recipients = mailparse::addrparse("test@gmail.com").unwrap();
        recipients.extend_from_slice(&mailparse::addrparse("test@test.com").unwrap()[..]);
        vctx.rcpt_to = Some(recipients);

        let buffer = context_get_recipients(&vctx);
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
        let mut vctx = Context {
            id: String::from("Testing"),
            mail_from: None,
            ..Default::default()
        };

        unsafe {
            assert_eq!(context_set_sender(&mut vctx, cstr!("test@test.com")), 0);
            assert_eq!(
                vctx.mail_from,
                Some(mailparse::addrparse("test@test.com").unwrap())
            );
        }
    }

    #[test]
    fn test_null_sender() {
        let mut vctx = Context {
            id: String::from("Testing"),
            mail_from: Some(mailparse::addrparse("test@test.com").unwrap()),
            ..Default::default()
        };

        unsafe {
            assert_eq!(context_set_sender(&mut vctx, null()), 0);
            assert_eq!(vctx.mail_from, None);
        }
    }

    #[test]
    fn test_invalid_sender() {
        let sender = mailparse::addrparse("test@test.com").unwrap();

        let mut vctx = Context {
            id: String::from("Testing"),
            mail_from: Some(sender.clone()),
            ..Default::default()
        };

        unsafe {
            assert_eq!(context_set_sender(&mut vctx, cstr!("---")), 1);
            assert_eq!(vctx.mail_from, Some(sender));
        }
    }

    #[test]
    fn test_data() {
        let data = b"Testing Data".to_vec();

        let vctx = Context {
            data: Some(data.clone()),
            ..Default::default()
        };

        unsafe {
            assert_eq!(
                CStr::from_ptr(context_get_data(&vctx).data).to_owned(),
                CString::from_vec_unchecked(data)
            );
        }
    }

    #[test]
    fn test_no_data() {
        let vctx = Context {
            data: None,
            ..Default::default()
        };

        assert_eq!(context_get_data(&vctx).data, null());
    }

    #[test]
    fn test_set_data_response() {
        let mut vctx = Context::default();

        let ans = unsafe { context_set_data_response(&mut vctx, cstr!("Test Response")) };
        assert_eq!(ans, 0);
        assert_eq!(vctx.data_response, Some("Test Response".to_string()));
    }

    #[test]
    fn test_set_null_data_response() {
        let mut vctx = Context {
            data_response: Some("Test".to_string()),
            ..Default::default()
        };

        let ans = unsafe { context_set_data_response(&mut vctx, null()) };
        assert_eq!(ans, 0);
        assert_eq!(vctx.data_response, None);
    }
}
