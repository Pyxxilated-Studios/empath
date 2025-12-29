//! TLS configuration helpers for outbound SMTP.

/// Outbound TLS mode for SMTP delivery.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum TlsMode {
    Disabled,
    Opportunistic,
    Required,
}

impl TlsMode {
    #[must_use]
    pub const fn new(attempt_tls: bool, require_tls: bool) -> Self {
        if !attempt_tls {
            Self::Disabled
        } else if require_tls {
            Self::Required
        } else {
            Self::Opportunistic
        }
    }

    #[must_use]
    pub const fn attempt_tls(self) -> bool {
        matches!(self, Self::Opportunistic | Self::Required)
    }

    #[must_use]
    pub const fn require_tls(self) -> bool {
        matches!(self, Self::Required)
    }
}

/// Certificate validation mode for outbound SMTP.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum CertValidation {
    Strict,
    AllowInvalid,
}

impl CertValidation {
    #[must_use]
    pub const fn accepts_invalid(self) -> bool {
        matches!(self, Self::AllowInvalid)
    }
}
