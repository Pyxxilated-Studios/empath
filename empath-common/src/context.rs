use std::{ffi::CString, fmt::Debug};

use mailparse::MailAddrList;

#[repr(C)]
pub struct FFIString {
    len: usize,
    data: *const i8,
}

impl Drop for FFIString {
    fn drop(&mut self) {
        unsafe { std::mem::drop(CString::from_raw(self.data.cast_mut())) }
    }
}

#[repr(C)]
pub struct FFIStringVector {
    len: usize,
    data: *const FFIString,
}

impl Drop for FFIStringVector {
    fn drop(&mut self) {
        let _ = unsafe { Vec::from_raw_parts(self.data.cast_mut(), self.len, self.len) };
    }
}

#[derive(Default, Debug)]
pub struct ValidationContext {
    pub id: String,
    pub mail_from: Option<MailAddrList>,
    pub rcpt_to: Option<MailAddrList>,
    pub data: Option<Vec<u8>>,
}

impl ValidationContext {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn message(&self) -> String {
        self.data.as_deref().map_or_else(String::default, |data| {
            std::str::from_utf8(data).map_or_else(|_| format!("{:#?}", self.data), str::to_string)
        })
    }

    pub fn sender(&self) -> String {
        self.mail_from
            .clone()
            .map(|f| f.to_string())
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
pub extern "C" fn validation_context_get_id(vctx: &ValidationContext) -> FFIString {
    let id = CString::new(vctx.id()).expect("Invalid CString");
    let len = vctx.id().len();
    let data = id.into_raw();

    FFIString { len, data }
}

#[no_mangle]
pub extern "C" fn validation_context_get_recipients(vctx: &ValidationContext) -> FFIStringVector {
    let rcpts = vctx
        .recipients()
        .iter()
        .map(|rcpt| {
            let len = rcpt.len();
            let rcpt = CString::new(rcpt.as_str()).expect("Invalid string");
            let data = rcpt.into_raw();
            FFIString { len, data }
        })
        .collect::<Vec<_>>();

    let (data, len, _) = rcpts.into_raw_parts();

    FFIStringVector { len, data }
}

#[no_mangle]
pub extern "C" fn free_string(ffi_string: FFIString) {
    std::mem::drop(ffi_string);
}

#[no_mangle]
pub extern "C" fn free_string_vector(ffi_vector: FFIStringVector) {
    std::mem::drop(ffi_vector);
}

#[cfg(test)]
mod test {
    use crate::context::{
        validation_context_get_id, validation_context_get_recipients, ValidationContext,
    };
    use std::ffi::{CStr, CString};

    #[test]
    fn test_id() {
        let mut vctx = ValidationContext::default();
        vctx.id = String::from("Testing");

        unsafe {
            let ffi_string = std::mem::ManuallyDrop::new(validation_context_get_id(&vctx));

            assert_eq!(
                CString::from_raw(ffi_string.data.cast_mut()),
                CString::new(vctx.id()).unwrap()
            );

            // This does not need to be called, because the above `CString::from_raw` does
            // what would need to be done here.
            // free_string(ffi_string);
        }
    }

    #[test]
    fn test_recipients() {
        let mut vctx = ValidationContext::default();

        let mut recipients = mailparse::addrparse("test@gmail.com").unwrap();
        recipients.extend_from_slice(&mailparse::addrparse("test@test.com").unwrap()[..]);
        vctx.rcpt_to = Some(recipients);

        let buffer = validation_context_get_recipients(&vctx);
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
}
