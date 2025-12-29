use serde::{Deserialize, Serialize};

/// TLS delivery policy for outbound SMTP connections.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum TlsPolicy {
    /// Attempt STARTTLS when advertised; allow plaintext if unavailable.
    #[default]
    Opportunistic,
    /// Require STARTTLS; fail delivery if TLS cannot be negotiated.
    Required,
    /// Never use STARTTLS (testing only).
    Disabled,
}

impl TlsPolicy {
    #[must_use]
    pub const fn attempt_tls(self) -> bool {
        matches!(self, Self::Opportunistic | Self::Required)
    }

    #[must_use]
    pub const fn require_tls(self) -> bool {
        matches!(self, Self::Required)
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Opportunistic => "opportunistic",
            Self::Required => "required",
            Self::Disabled => "disabled",
        }
    }
}

impl std::fmt::Display for TlsPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
