//! DNS error types

use thiserror::Error;

/// Errors that can occur during DNS resolution.
#[derive(Debug, Clone, Error)]
pub enum DnsError {
    /// No MX, A, or AAAA records found for the domain.
    #[error("No mail servers found for domain: {0}")]
    NoMailServers(String),

    /// DNS query failed due to network or resolver issues.
    #[error("DNS lookup failed: {0}")]
    LookupFailed(#[from] hickory_resolver::ResolveError),

    /// Domain does not exist (NXDOMAIN).
    #[error("Domain does not exist: {0}")]
    DomainNotFound(String),

    /// DNS query timed out.
    #[error("DNS query timed out for domain: {0}")]
    Timeout(String),

    /// DNSSEC validation failed (bogus or indeterminate).
    #[error("DNSSEC validation failed for {target}: {status}")]
    DnssecValidationFailed {
        target: String,
        status: DnssecStatus,
    },
}

impl DnsError {
    /// Returns `true` if this error is temporary and should be retried.
    #[must_use]
    pub const fn is_temporary(&self) -> bool {
        matches!(self, Self::Timeout(_) | Self::LookupFailed(_))
            || matches!(
                self,
                Self::DnssecValidationFailed {
                    status: DnssecStatus::Indeterminate,
                    ..
                }
            )
    }
}

/// DNSSEC validation status for a lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DnssecStatus {
    Secure,
    Insecure,
    Bogus,
    Indeterminate,
}

impl DnssecStatus {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Secure => "secure",
            Self::Insecure => "insecure",
            Self::Bogus => "bogus",
            Self::Indeterminate => "indeterminate",
        }
    }

    #[must_use]
    pub const fn is_failure(self) -> bool {
        matches!(self, Self::Bogus | Self::Indeterminate)
    }
}

impl std::fmt::Display for DnssecStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
