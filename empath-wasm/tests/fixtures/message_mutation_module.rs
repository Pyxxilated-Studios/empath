//! Minimal WASM guest module for testing message helper host APIs.
//! Compiled as a binary to wasm32-wasip1 for use in tests.

use std::ffi::CString;

// Host imports generated from wit (handwritten here to keep small).
// These function signatures must match the host-side interface in empath.wit.
extern "C" {
    // headers
    fn headers_set(name: *const u8, name_len: usize, value: *const u8, value_len: usize) -> i32;
    fn body_append_text(
        text: *const u8,
        text_len: usize,
        charset: *const u8,
        charset_len: usize,
    ) -> i32;
}

#[no_mangle]
pub extern "C" fn validate_data(_ctx: u32) -> i32 {
    // Set a header and append to body; ignore errors for brevity.
    let name = b"X-Wasm-Test";
    let value = b"ok";
    unsafe {
        let _ = headers_set(name.as_ptr(), name.len(), value.as_ptr(), value.len());
        let charset = b"utf-8";
        let body = b" appended";
        let _ = body_append_text(body.as_ptr(), body.len(), charset.as_ptr(), charset.len());
    }
    0 // accept
}

#[no_mangle]
pub extern "C" fn validate_connect(_ctx: u32) -> i32 {
    0
}
#[no_mangle]
pub extern "C" fn validate_ehlo(_ctx: u32) -> i32 {
    0
}
#[no_mangle]
pub extern "C" fn validate_mail_from(_ctx: u32) -> i32 {
    0
}
#[no_mangle]
pub extern "C" fn validate_rcpt_to(_ctx: u32) -> i32 {
    0
}
#[no_mangle]
pub extern "C" fn validate_starttls(_ctx: u32) -> i32 {
    0
}
