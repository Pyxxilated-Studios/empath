use std::{ffi::CString, ptr::null, str::Utf8Error};

#[repr(C)]
pub struct String {
    pub len: usize,
    pub data: *const i8,
}

impl Drop for String {
    fn drop(&mut self) {
        if !self.data.is_null() {
            let _ = unsafe { CString::from_raw(self.data.cast_mut()) };
        }
    }
}

impl Default for String {
    fn default() -> Self {
        Self {
            len: 0,
            data: null(),
        }
    }
}

#[repr(C)]
#[allow(clippy::module_name_repetitions)]
pub struct StringVector {
    pub len: usize,
    pub data: *const String,
}

impl Drop for StringVector {
    fn drop(&mut self) {
        if !self.data.is_null() {
            let _ = unsafe { Vec::from_raw_parts(self.data.cast_mut(), self.len, self.len) };
        }
    }
}

impl TryFrom<&[u8]> for String {
    type Error = Utf8Error;

    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        Ok(Self::from(std::str::from_utf8(value)?))
    }
}

impl From<&str> for String {
    fn from(value: &str) -> Self {
        let len = value.len();
        let id = CString::new(value).expect("Invalid CString");
        let data = id.into_raw();

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

impl From<&Vec<std::string::String>> for StringVector {
    fn from(value: &Vec<std::string::String>) -> Self {
        let rcpts = value
            .iter()
            .map(std::convert::Into::into)
            .collect::<Vec<_>>();

        let (data, len, _) = rcpts.into_raw_parts();

        Self { len, data }
    }
}

impl From<Vec<std::string::String>> for StringVector {
    fn from(value: Vec<std::string::String>) -> Self {
        Self::from(&value)
    }
}

#[no_mangle]
#[allow(clippy::module_name_repetitions)]
pub extern "C" fn free_string(ffi_string: String) {
    std::mem::drop(ffi_string);
}

#[no_mangle]
#[allow(clippy::module_name_repetitions)]
pub extern "C" fn free_string_vector(ffi_vector: StringVector) {
    std::mem::drop(ffi_vector);
}
