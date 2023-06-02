use std::{ffi::CString, fmt::Debug};

use mailparse::MailAddrList;

#[repr(C)]
pub struct Buffer {
    len: usize,
    data: *const i8,
}

#[derive(Default, Debug)]
pub struct ValidationContext {
    pub(crate) id: String,
    pub(crate) mail_from: Option<MailAddrList>,
    pub(crate) rcpt_to: Option<MailAddrList>,
    pub(crate) data: Option<Vec<u8>>,
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
#[no_mangle]
pub extern "C" fn validation_context_get_id(vctx: &ValidationContext) -> *const libc::c_char {
    let id = CString::new(vctx.id()).unwrap();
    let data = id.as_ptr();

    std::mem::forget(id);

    data.cast()
}

#[no_mangle]
pub extern "C" fn validation_context_get_recipients(vctx: &ValidationContext) -> Buffer {
    let rcpts = vctx.recipients();
    let data = rcpts.as_ptr().cast();
    let len = rcpts.len();

    std::mem::forget(rcpts);

    Buffer { len, data }
}
