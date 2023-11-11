use std::{ffi::CString, ptr::null, rc::Rc, str::Utf8Error, sync::Arc};

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

impl From<&Arc<str>> for String {
    fn from(value: &Arc<str>) -> Self {
        let len = value.len();
        let id = CString::new(value.as_bytes()).expect("Invalid CString");
        let data = id.into_raw();

        Self { len, data }
    }
}

impl From<&Rc<str>> for String {
    fn from(value: &Rc<str>) -> Self {
        let len = value.len();
        let id = CString::new(value.as_bytes()).expect("Invalid CString");
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

#[no_mangle]
#[allow(clippy::module_name_repetitions)]
pub extern "C" fn free_string(ffi_string: String) {
    drop(ffi_string);
}

#[no_mangle]
#[allow(clippy::module_name_repetitions)]
pub extern "C" fn free_string_vector(ffi_vector: StringVector) {
    drop(ffi_vector);
}

#[cfg(test)]
mod test {
    use std::{ptr::null, sync::Arc};

    use crate::ffi::string::{free_string, free_string_vector, String, StringVector};

    const TEST: &str = "test";

    #[test]
    fn string_vector() {
        let sv = StringVector::from(vec![]);
        assert_eq!(sv.len, 0);
        assert_ne!(sv.data, null());

        free_string_vector(sv);

        let sv = StringVector::from(&[TEST.to_string()][..]);
        assert_eq!(sv.len, 1);
        assert_ne!(sv.data, null());

        free_string_vector(sv);

        let sv = StringVector::from(&[Arc::from(TEST)][..]);
        assert_eq!(sv.len, 1);
        assert_ne!(sv.data, null());

        free_string_vector(sv);

        let sv = StringVector::from(vec![TEST.to_string()]);
        assert_eq!(sv.len, 1);
        assert_ne!(sv.data, null());

        free_string_vector(sv);
    }

    #[test]
    fn string() {
        let s = String::from("");
        assert_eq!(s.len, 0);
        assert_ne!(s.data, null());

        free_string(s);

        let s = String::from(TEST);
        assert_eq!(s.len, TEST.len());
        assert_ne!(s.data, null());

        free_string(s);

        let s = String::from(&Arc::from(TEST));
        assert_eq!(s.len, TEST.len());
        assert_ne!(s.data, null());

        free_string(s);
    }
}
