use std::ffi::CString;

#[repr(C)]
pub struct FFIString {
    pub len: usize,
    pub data: *const i8,
}

impl Drop for FFIString {
    fn drop(&mut self) {
        let _ = unsafe { CString::from_raw(self.data.cast_mut()) };
    }
}

#[repr(C)]
pub struct FFIStringVector {
    pub len: usize,
    pub data: *const FFIString,
}

impl Drop for FFIStringVector {
    fn drop(&mut self) {
        let _ = unsafe { Vec::from_raw_parts(self.data.cast_mut(), self.len, self.len) };
    }
}

impl From<&str> for FFIString {
    fn from(value: &str) -> Self {
        let len = value.len();
        let id = CString::new(value).expect("Invalid CString");
        let data = id.into_raw();

        FFIString { len, data }
    }
}

impl From<&String> for FFIString {
    fn from(value: &String) -> Self {
        FFIString::from(value.as_str())
    }
}

impl From<String> for FFIString {
    fn from(value: String) -> Self {
        FFIString::from(value.as_str())
    }
}

impl From<&Vec<String>> for FFIStringVector {
    fn from(value: &Vec<String>) -> Self {
        let rcpts = value
            .iter()
            .map(std::convert::Into::into)
            .collect::<Vec<_>>();

        let (data, len, _) = rcpts.into_raw_parts();

        FFIStringVector { len, data }
    }
}

impl From<Vec<String>> for FFIStringVector {
    fn from(value: Vec<String>) -> Self {
        FFIStringVector::from(&value)
    }
}
