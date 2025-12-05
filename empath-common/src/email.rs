use std::{
    fmt::{self, Display},
    ops::Deref,
    sync::Arc,
};

use serde::{Deserialize, Serialize};

/// Errors produced by validated newtypes.
#[derive(Debug, PartialEq, Eq)]
pub enum EmailValidationError {
    MissingAt,
    InvalidFormat(&'static str),
}

impl Display for EmailValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingAt => write!(f, "email address must contain '@'"),
            Self::InvalidFormat(reason) => write!(f, "invalid email address: {reason}"),
        }
    }
}

/// Email address newtype with basic validation.
///
/// Validates the presence of '@' and rejects leading/trailing '@' or whitespace.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
#[repr(transparent)]
pub struct EmailAddress(Arc<str>);

impl EmailAddress {
    /// Construct a validated `EmailAddress`.
    ///
    /// Validation: contains '@', not starting/ending with '@', and no whitespace.
    ///
    /// # Errors
    ///
    /// Returns `EmailValidationError` when validation fails (missing `@`, whitespace, or leading/trailing `@`).
    pub fn new<S: Into<Arc<str>>>(s: S) -> Result<Self, EmailValidationError> {
        let s: Arc<str> = s.into();
        let bytes = s.as_bytes();

        if !bytes.contains(&b'@') {
            return Err(EmailValidationError::MissingAt);
        }
        if bytes.first() == Some(&b'@') || bytes.last() == Some(&b'@') {
            return Err(EmailValidationError::InvalidFormat(
                "address cannot start or end with '@'",
            ));
        }
        if bytes.iter().any(u8::is_ascii_whitespace) {
            return Err(EmailValidationError::InvalidFormat(
                "address cannot contain whitespace",
            ));
        }

        Ok(Self(s))
    }

    /// Construct an `EmailAddress` without validation.
    ///
    /// Intended for internal use when the input is already validated upstream.
    #[must_use]
    pub fn new_unchecked<S: Into<Arc<str>>>(s: S) -> Self {
        Self(s.into())
    }

    /// Borrow the address as `&str`.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consume into the inner `Arc<str>`.
    #[must_use]
    pub fn into_inner(self) -> Arc<str> {
        self.0
    }
}

impl Display for EmailAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Deref for EmailAddress {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl TryFrom<String> for EmailAddress {
    type Error = EmailValidationError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl TryFrom<&str> for EmailAddress {
    type Error = EmailValidationError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl TryFrom<Arc<str>> for EmailAddress {
    type Error = EmailValidationError;

    fn try_from(value: Arc<str>) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<EmailAddress> for Arc<str> {
    fn from(value: EmailAddress) -> Self {
        value.0
    }
}

impl From<&EmailAddress> for Arc<str> {
    fn from(value: &EmailAddress) -> Self {
        value.0.clone()
    }
}

/// Server identifier (e.g., MX host) newtype.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
#[repr(transparent)]
pub struct ServerId(Arc<str>);

impl ServerId {
    /// Create a `ServerId` ensuring it is not empty and has no whitespace.
    ///
    /// # Errors
    ///
    /// Returns `EmailValidationError` if empty or contains whitespace.
    pub fn new<S: Into<Arc<str>>>(s: S) -> Result<Self, EmailValidationError> {
        let s: Arc<str> = s.into();
        let bytes = s.as_bytes();
        if bytes.is_empty() {
            return Err(EmailValidationError::InvalidFormat(
                "server id cannot be empty",
            ));
        }
        if bytes.iter().any(u8::is_ascii_whitespace) {
            return Err(EmailValidationError::InvalidFormat(
                "server id cannot contain whitespace",
            ));
        }
        Ok(Self(s))
    }

    /// Create a `ServerId` without validation (use only with trusted inputs).
    #[must_use]
    pub fn new_unchecked<S: Into<Arc<str>>>(s: S) -> Self {
        Self(s.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Display for ServerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Deref for ServerId {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl TryFrom<String> for ServerId {
    type Error = EmailValidationError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl TryFrom<&str> for ServerId {
    type Error = EmailValidationError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

/// Banner hostname newtype for greeting messages.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
#[repr(transparent)]
pub struct BannerHostname(Arc<str>);

impl BannerHostname {
    /// Create a `BannerHostname` ensuring it is not empty and has no whitespace.
    ///
    /// # Errors
    ///
    /// Returns `EmailValidationError` if empty or contains whitespace.
    pub fn new<S: Into<Arc<str>>>(s: S) -> Result<Self, EmailValidationError> {
        let s: Arc<str> = s.into();
        let bytes = s.as_bytes();
        if bytes.is_empty() {
            return Err(EmailValidationError::InvalidFormat(
                "banner hostname cannot be empty",
            ));
        }
        if bytes.iter().any(u8::is_ascii_whitespace) {
            return Err(EmailValidationError::InvalidFormat(
                "banner hostname cannot contain whitespace",
            ));
        }
        Ok(Self(s))
    }

    /// Create a `BannerHostname` without validation (internal use for fallbacks).
    pub fn new_unchecked(s: &str) -> Self {
        Self(Arc::from(s))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Display for BannerHostname {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Deref for BannerHostname {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl TryFrom<String> for BannerHostname {
    type Error = EmailValidationError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl TryFrom<&str> for BannerHostname {
    type Error = EmailValidationError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::unwrap_in_result,
        clippy::unwrap_or_default,
        reason = "tests should fail loudly on validation regression"
    )]
    use super::{BannerHostname, EmailAddress, EmailValidationError, ServerId};

    #[test]
    fn email_valid() {
        let addr = EmailAddress::new("user@example.com").unwrap();
        assert_eq!(addr.as_str(), "user@example.com");
    }

    #[test]
    fn email_invalid_missing_at() {
        let err = EmailAddress::new("invalid").unwrap_err();
        assert_eq!(err, EmailValidationError::MissingAt);
    }

    #[test]
    fn email_invalid_whitespace() {
        let err = EmailAddress::new("user @example.com").unwrap_err();
        assert_eq!(
            err,
            EmailValidationError::InvalidFormat("address cannot contain whitespace")
        );
    }

    #[test]
    fn server_id_rejects_empty() {
        let err = ServerId::new("").unwrap_err();
        assert_eq!(
            err,
            EmailValidationError::InvalidFormat("server id cannot be empty")
        );
    }

    #[test]
    fn banner_hostname_rejects_whitespace() {
        let err = BannerHostname::new("mail host").unwrap_err();
        assert_eq!(
            err,
            EmailValidationError::InvalidFormat("banner hostname cannot contain whitespace")
        );
    }
}
