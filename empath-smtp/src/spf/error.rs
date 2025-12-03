use thiserror::Error;

/// SPF evaluation errors
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SpfError {
    /// DNS lookup failure (temporary)
    #[error("DNS lookup failed: {0}")]
    DnsLookupFailed(String),

    /// DNS timeout
    #[error("DNS timeout")]
    DnsTimeout,

    /// Too many DNS lookups (RFC 7208 limit: 10)
    #[error("Too many DNS lookups (limit: {limit})")]
    TooManyLookups { limit: usize },

    /// Too many void lookups (RFC 7208 limit: 2)
    #[error("Too many void lookups (limit: {limit})")]
    TooManyVoidLookups { limit: usize },

    /// Invalid SPF record syntax
    #[error("Invalid SPF record: {0}")]
    InvalidRecord(String),

    /// Invalid macro syntax
    #[error("Invalid macro syntax: {0}")]
    InvalidMacro(String),

    /// Infinite recursion detected
    #[error("Infinite recursion detected (depth: {0})")]
    InfiniteRecursion(usize),

    /// No SPF record found
    #[error("No SPF record found")]
    NoRecord,

    /// Invalid sender email format
    #[error("Invalid sender email: {0}")]
    InvalidSender(String),

    /// Invalid domain format
    #[error("Invalid domain: {0}")]
    InvalidDomain(String),
}

impl SpfError {
    /// Returns true if this is a temporary error (temperror in SPF terminology)
    #[must_use]
    pub const fn is_temporary(&self) -> bool {
        matches!(
            self,
            Self::DnsLookupFailed(_) | Self::DnsTimeout | Self::InfiniteRecursion(_)
        )
    }

    /// Returns true if this is a permanent error (permerror in SPF terminology)
    #[must_use]
    pub const fn is_permanent(&self) -> bool {
        matches!(
            self,
            Self::TooManyLookups { .. }
                | Self::TooManyVoidLookups { .. }
                | Self::InvalidRecord(_)
                | Self::InvalidMacro(_)
                | Self::InvalidSender(_)
                | Self::InvalidDomain(_)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_temporary_errors() {
        assert!(SpfError::DnsLookupFailed("test".to_string()).is_temporary());
        assert!(SpfError::DnsTimeout.is_temporary());
        assert!(SpfError::InfiniteRecursion(10).is_temporary());
    }

    #[test]
    fn test_permanent_errors() {
        assert!(SpfError::TooManyLookups { limit: 10 }.is_permanent());
        assert!(SpfError::TooManyVoidLookups { limit: 2 }.is_permanent());
        assert!(SpfError::InvalidRecord("test".to_string()).is_permanent());
        assert!(SpfError::InvalidMacro("test".to_string()).is_permanent());
    }

    #[test]
    fn test_no_record_neither_temp_nor_perm() {
        assert!(!SpfError::NoRecord.is_temporary());
        assert!(!SpfError::NoRecord.is_permanent());
    }
}
