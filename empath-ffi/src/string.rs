use std::{bstr::ByteStr, ffi::CString, ptr::null, rc::Rc, str::Utf8Error, sync::Arc};

/// Sanitize bytes by filtering out null bytes (\0).
/// This prevents `CString` creation from panicking on malicious FFI module input.
fn sanitize_null_bytes(bytes: &[u8]) -> Vec<u8> {
    bytes.iter().copied().filter(|&b| b != 0).collect()
}

#[repr(C)]
#[derive(Default)]
pub struct String {
    pub len: usize,
    pub data: *const i8,
}

impl Drop for String {
    fn drop(&mut self) {
        if !self.data.is_null() {
            let _ =
                unsafe { CString::from_raw((self.data.cast::<core::ffi::c_char>()).cast_mut()) };
            self.data = null();
        }
    }
}

#[repr(C)]
#[allow(clippy::module_name_repetitions)]
#[derive(Default)]
pub struct StringVector {
    pub len: usize,
    pub data: *const String,
}

impl Drop for StringVector {
    fn drop(&mut self) {
        if !self.data.is_null() {
            let _ = unsafe { Vec::from_raw_parts(self.data.cast_mut(), self.len, self.len) };
            self.data = null();
        }
    }
}

impl TryFrom<&[u8]> for String {
    type Error = Utf8Error;

    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        Ok(Self::from(ByteStr::new(value)))
    }
}

impl From<&str> for String {
    fn from(value: &str) -> Self {
        // SAFETY: Sanitize null bytes to prevent panics from malicious FFI module input.
        let sanitized = sanitize_null_bytes(value.as_bytes());
        let len = sanitized.len();
        let id = CString::new(sanitized).unwrap_or_default();
        let data = id.into_raw().cast::<i8>();

        Self { len, data }
    }
}

impl From<&ByteStr> for String {
    fn from(value: &ByteStr) -> Self {
        Self::from(value.to_string())
    }
}

impl From<&Arc<str>> for String {
    fn from(value: &Arc<str>) -> Self {
        // SAFETY: Sanitize null bytes to prevent panics from malicious FFI module input.
        let sanitized = sanitize_null_bytes(value.as_bytes());
        let len = sanitized.len();
        let id = CString::new(sanitized).unwrap_or_default();
        let data = id.into_raw().cast::<i8>();

        Self { len, data }
    }
}

impl From<&Rc<str>> for String {
    fn from(value: &Rc<str>) -> Self {
        // SAFETY: Sanitize null bytes to prevent panics from malicious FFI module input.
        let sanitized = sanitize_null_bytes(value.as_bytes());
        let len = sanitized.len();
        let id = CString::new(sanitized).unwrap_or_default();
        let data = id.into_raw().cast::<i8>();

        Self { len, data }
    }
}

impl From<&std::string::String> for String {
    fn from(value: &std::string::String) -> Self {
        Self::from(value.as_str())
    }
}

impl From<std::string::String> for String {
    fn from(value: std::string::String) -> Self {
        Self::from(value.as_str())
    }
}

impl From<&[std::string::String]> for StringVector {
    fn from(value: &[std::string::String]) -> Self {
        let recipients = value
            .iter()
            .map(std::convert::Into::into)
            .collect::<Vec<_>>();

        let (data, len, _) = recipients.into_raw_parts();

        Self { len, data }
    }
}

impl From<Vec<std::string::String>> for StringVector {
    fn from(value: Vec<std::string::String>) -> Self {
        Self::from(value.as_slice())
    }
}

impl From<&[Arc<str>]> for StringVector {
    fn from(value: &[Arc<str>]) -> Self {
        let recipients = value
            .iter()
            .map(std::convert::Into::into)
            .collect::<Vec<_>>();

        let (data, len, _) = recipients.into_raw_parts();

        Self { len, data }
    }
}

impl From<&[Rc<str>]> for StringVector {
    fn from(value: &[Rc<str>]) -> Self {
        let recipients = value
            .iter()
            .map(std::convert::Into::into)
            .collect::<Vec<_>>();

        let (data, len, _) = recipients.into_raw_parts();

        Self { len, data }
    }
}

#[unsafe(no_mangle)]
#[allow(clippy::module_name_repetitions)]
pub extern "C" fn em_free_string(ffi_string: String) {
    drop(ffi_string);
}

#[unsafe(no_mangle)]
#[allow(clippy::module_name_repetitions)]
pub extern "C" fn em_free_string_vector(ffi_vector: StringVector) {
    drop(ffi_vector);
}

#[cfg(test)]
mod test {
    use std::{ptr::null, sync::Arc};

    use crate::string::{String, StringVector, em_free_string, em_free_string_vector};

    const TEST: &str = "test";

    #[test]
    fn string_vector() {
        let sv = StringVector::from(vec![]);
        assert_eq!(sv.len, 0);
        assert_ne!(sv.data, null());

        em_free_string_vector(sv);

        let sv = StringVector::from(&[TEST.to_string()][..]);
        assert_eq!(sv.len, 1);
        assert_ne!(sv.data, null());

        em_free_string_vector(sv);

        let sv = StringVector::from(&[Arc::from(TEST)][..]);
        assert_eq!(sv.len, 1);
        assert_ne!(sv.data, null());

        em_free_string_vector(sv);

        let sv = StringVector::from(vec![TEST.to_string()]);
        assert_eq!(sv.len, 1);
        assert_ne!(sv.data, null());

        em_free_string_vector(sv);
    }

    #[test]
    fn string() {
        let s = String::from("");
        assert_eq!(s.len, 0);
        assert_ne!(s.data, null());

        em_free_string(s);

        let s = String::from(TEST);
        assert_eq!(s.len, TEST.len());
        assert_ne!(s.data, null());

        em_free_string(s);

        let s = String::from(&Arc::from(TEST));
        assert_eq!(s.len, TEST.len());
        assert_ne!(s.data, null());

        em_free_string(s);
    }

    #[test]
    fn string_null_byte_sanitization_str() {
        // Test that null bytes are removed from &str input
        let s = String::from("test\0with\0nulls");
        assert_eq!(s.len, "testwithnulls".len());
        assert_ne!(s.data, null());
        em_free_string(s);

        // Test string that is only null bytes becomes empty
        let s = String::from("\0\0\0");
        assert_eq!(s.len, 0);
        assert_ne!(s.data, null());
        em_free_string(s);

        // Test null byte at start
        let s = String::from("\0test");
        assert_eq!(s.len, "test".len());
        assert_ne!(s.data, null());
        em_free_string(s);

        // Test null byte at end
        let s = String::from("test\0");
        assert_eq!(s.len, "test".len());
        assert_ne!(s.data, null());
        em_free_string(s);
    }

    #[test]
    fn string_null_byte_sanitization_arc() {
        // Test that null bytes are removed from Arc<str> input
        let s = String::from(&Arc::from("test\0with\0nulls"));
        assert_eq!(s.len, "testwithnulls".len());
        assert_ne!(s.data, null());
        em_free_string(s);

        // Test Arc<str> with only null bytes
        let s = String::from(&Arc::from("\0\0"));
        assert_eq!(s.len, 0);
        assert_ne!(s.data, null());
        em_free_string(s);
    }

    #[test]
    fn string_null_byte_sanitization_rc() {
        use std::rc::Rc;

        // Test that null bytes are removed from Rc<str> input
        let s = String::from(&Rc::from("test\0with\0nulls"));
        assert_eq!(s.len, "testwithnulls".len());
        assert_ne!(s.data, null());
        em_free_string(s);

        // Test Rc<str> with only null bytes
        let s = String::from(&Rc::from("\0\0"));
        assert_eq!(s.len, 0);
        assert_ne!(s.data, null());
        em_free_string(s);
    }

    #[test]
    fn string_null_byte_no_panic() {
        // This test verifies that we don't panic on null bytes (the original bug)
        // If this test completes without panicking, the fix is working
        let malicious_inputs = vec![
            "normal",
            "\0",
            "start\0end",
            "\0\0\0",
            "multiple\0null\0bytes\0here",
        ];

        for input in malicious_inputs {
            let s = String::from(input);
            assert_ne!(s.data, null());
            em_free_string(s);

            let s = String::from(&Arc::from(input));
            assert_ne!(s.data, null());
            em_free_string(s);

            let s = String::from(&std::rc::Rc::from(input));
            assert_ne!(s.data, null());
            em_free_string(s);
        }
    }
}
