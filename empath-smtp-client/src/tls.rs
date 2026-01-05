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

/// Minimum TLS version required for outbound SMTP connections.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum TlsMinimumVersion {
    Tls12,
}

impl TlsMinimumVersion {
    #[must_use]
    pub const fn is_satisfied_by(self, version: tokio_rustls::rustls::ProtocolVersion) -> bool {
        match self {
            Self::Tls12 => matches!(
                version,
                tokio_rustls::rustls::ProtocolVersion::TLSv1_2
                    | tokio_rustls::rustls::ProtocolVersion::TLSv1_3
            ),
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Tls12 => "tls1.2+",
        }
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

#[cfg(test)]
mod tests {
    use tokio_rustls::rustls::ProtocolVersion;

    use super::{CertValidation, TlsMinimumVersion, TlsMode};

    #[test]
    fn tls_mode_construction_and_flags() {
        let disabled = TlsMode::new(false, false);
        assert_eq!(disabled, TlsMode::Disabled);
        assert!(!disabled.attempt_tls());
        assert!(!disabled.require_tls());

        let opportunistic = TlsMode::new(true, false);
        assert_eq!(opportunistic, TlsMode::Opportunistic);
        assert!(opportunistic.attempt_tls());
        assert!(!opportunistic.require_tls());

        let required = TlsMode::new(true, true);
        assert_eq!(required, TlsMode::Required);
        assert!(required.attempt_tls());
        assert!(required.require_tls());
    }

    #[test]
    fn tls_minimum_version_accepts_tls12_and_tls13() {
        let min = TlsMinimumVersion::Tls12;
        assert!(min.is_satisfied_by(ProtocolVersion::TLSv1_2));
        assert!(min.is_satisfied_by(ProtocolVersion::TLSv1_3));
        assert_eq!(min.as_str(), "tls1.2+");
    }

    #[test]
    fn cert_validation_accepts_invalid() {
        assert!(!CertValidation::Strict.accepts_invalid());
        assert!(CertValidation::AllowInvalid.accepts_invalid());
    }
}
