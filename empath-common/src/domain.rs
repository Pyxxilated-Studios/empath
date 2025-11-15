//! Domain newtype for type safety
//!
//! Wraps domain strings to prevent accidentally passing email addresses
//! or other strings where domains are expected. Provides a zero-cost
//! abstraction with compile-time type safety.

use std::{
    fmt::{self, Display},
    ops::Deref,
    sync::Arc,
};

use serde::{Deserialize, Serialize};

/// A domain name string wrapper for type safety
///
/// This newtype prevents accidentally passing email addresses or other
/// strings where domain names are expected. The `#[repr(transparent)]`
/// attribute ensures this is a zero-cost abstraction at runtime.
///
/// # Examples
///
/// ```
/// use empath_common::Domain;
///
/// let domain = Domain::new("example.com");
/// assert_eq!(domain.as_str(), "example.com");
///
/// // Zero-cost conversion from String
/// let domain: Domain = "mail.example.com".into();
/// assert_eq!(domain.as_str(), "mail.example.com");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
#[repr(transparent)] // Zero-cost abstraction guarantee
pub struct Domain(Arc<str>);

impl Domain {
    /// Create a new `Domain` from any type that can be converted to `Arc<str>`
    ///
    /// # Examples
    ///
    /// ```
    /// use empath_common::Domain;
    ///
    /// let domain = Domain::new("example.com");
    /// let domain = Domain::new(String::from("example.com"));
    /// let domain = Domain::new(Arc::from("example.com"));
    /// ```
    #[must_use]
    pub fn new(s: impl Into<Arc<str>>) -> Self {
        Self(s.into())
    }

    /// Get the domain as a string slice
    ///
    /// # Examples
    ///
    /// ```
    /// use empath_common::Domain;
    ///
    /// let domain = Domain::new("example.com");
    /// assert_eq!(domain.as_str(), "example.com");
    /// ```
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Convert the domain into the inner `Arc<str>`
    ///
    /// # Examples
    ///
    /// ```
    /// use empath_common::Domain;
    ///
    /// let domain = Domain::new("example.com");
    /// let arc_str = domain.into_inner();
    /// ```
    #[must_use]
    pub fn into_inner(self) -> Arc<str> {
        self.0
    }
}

impl Display for Domain {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl AsRef<str> for Domain {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl Deref for Domain {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl From<String> for Domain {
    fn from(s: String) -> Self {
        Self(Arc::from(s))
    }
}

impl From<&str> for Domain {
    fn from(s: &str) -> Self {
        Self(Arc::from(s))
    }
}

impl From<Arc<str>> for Domain {
    fn from(s: Arc<str>) -> Self {
        Self(s)
    }
}

impl From<Domain> for Arc<str> {
    fn from(domain: Domain) -> Self {
        domain.0
    }
}

impl From<&Domain> for Arc<str> {
    fn from(domain: &Domain) -> Self {
        domain.0.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_domain_creation() {
        let domain = Domain::new("example.com");
        assert_eq!(domain.as_str(), "example.com");
    }

    #[test]
    fn test_domain_from_string() {
        let s = String::from("mail.example.com");
        let domain: Domain = s.into();
        assert_eq!(domain.as_str(), "mail.example.com");
    }

    #[test]
    fn test_domain_from_str() {
        let domain: Domain = "test.example.com".into();
        assert_eq!(domain.as_str(), "test.example.com");
    }

    #[test]
    fn test_domain_from_arc_str() {
        let arc_str: Arc<str> = Arc::from("arc.example.com");
        let domain: Domain = arc_str.into();
        assert_eq!(domain.as_str(), "arc.example.com");
    }

    #[test]
    fn test_domain_display() {
        let domain = Domain::new("display.example.com");
        assert_eq!(format!("{domain}"), "display.example.com");
    }

    #[test]
    fn test_domain_as_ref() {
        let domain = Domain::new("ref.example.com");
        let s: &str = domain.as_ref();
        assert_eq!(s, "ref.example.com");
    }

    #[test]
    fn test_domain_deref() {
        let domain = Domain::new("deref.example.com");
        assert_eq!(domain.len(), "deref.example.com".len());
        assert!(!domain.is_empty());
    }

    #[test]
    fn test_domain_equality() {
        let domain1 = Domain::new("example.com");
        let domain2 = Domain::new("example.com");
        let domain3 = Domain::new("different.com");

        assert_eq!(domain1, domain2);
        assert_ne!(domain1, domain3);
    }

    #[test]
    fn test_domain_clone() {
        let domain1 = Domain::new("clone.example.com");
        let domain2 = domain1.clone();
        assert_eq!(domain1, domain2);
    }

    #[test]
    fn test_domain_serde() {
        use serde_json;

        let domain = Domain::new("serde.example.com");
        let serialized = serde_json::to_string(&domain).unwrap();
        assert_eq!(serialized, "\"serde.example.com\"");

        let deserialized: Domain = serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized, domain);
    }

    #[test]
    fn test_domain_into_inner() {
        let domain = Domain::new("inner.example.com");
        let arc_str: Arc<str> = domain.into_inner();
        assert_eq!(arc_str.as_ref(), "inner.example.com");
    }

    #[test]
    fn test_domain_hash() {
        use std::collections::HashMap;

        let mut map = HashMap::new();
        let domain = Domain::new("hash.example.com");
        map.insert(domain.clone(), 42);

        assert_eq!(map.get(&domain), Some(&42));
    }
}
