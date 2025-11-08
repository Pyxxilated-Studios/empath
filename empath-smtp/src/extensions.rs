use core::fmt::{self, Display};

use empath_common::context::Capability;
use serde::Deserialize;

use crate::session::TlsContext;

/// SMTP protocol extensions advertised in EHLO response.
///
/// Extensions modify SMTP behavior and capabilities as defined in various RFCs.
/// The server advertises supported extensions after receiving EHLO from the client.
#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "lowercase")]
pub enum Extension {
    /// STARTTLS extension (RFC 3207) - Allows upgrading connection to TLS.
    ///
    /// When advertised, clients can use the STARTTLS command to initiate
    /// TLS negotiation before transmitting sensitive data.
    Starttls(TlsContext),

    /// HELP extension - Provides command help information.
    ///
    /// Allows clients to request help about available commands via HELP command.
    Help,

    /// SIZE extension (RFC 1870) - Message size declaration and enforcement.
    ///
    /// # Behavior
    ///
    /// - Server advertises maximum message size in EHLO: `SIZE <max_bytes>`
    /// - Client declares message size in MAIL FROM: `MAIL FROM:<addr> SIZE=<bytes>`
    /// - Server validates at two points:
    ///   1. MAIL FROM: Rejects if declared size exceeds maximum (552 status)
    ///   2. DATA: Rejects if actual received bytes exceed maximum (552 status)
    ///
    /// # Configuration
    ///
    /// Set to 0 for no size limit (unlimited). When set to a positive value,
    /// messages exceeding the limit are rejected with SMTP status code 552.
    ///
    /// # RFC 1870 Compliance
    ///
    /// Per RFC 1870 Section 4, the SIZE parameter value "indicates the size of
    /// the message that the client wishes to transfer. The server may reject
    /// the MAIL command if the value supplied exceeds its implementation
    /// limit or otherwise violates a site policy."
    Size(usize),
}

impl Display for Extension {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        match self {
            Self::Starttls(_) => fmt.write_str("STARTTLS"),
            Self::Help => fmt.write_str("HELP"),
            Self::Size(max) => {
                if *max == 0 {
                    fmt.write_str("SIZE")
                } else {
                    write!(fmt, "SIZE {max}")
                }
            }
        }
    }
}

impl TryInto<Capability> for Extension {
    type Error = ();

    fn try_into(self) -> Result<Capability, Self::Error> {
        Err(())
    }
}

impl TryInto<Capability> for &Extension {
    type Error = ();

    fn try_into(self) -> Result<Capability, Self::Error> {
        Err(())
    }
}

#[cfg(test)]
mod test {
    use super::Extension;
    use crate::session::TlsContext;

    #[test]
    fn extension_display() {
        // SIZE with limit should show the value
        let size_limited = Extension::Size(100_000_000);
        assert_eq!(size_limited.to_string(), "SIZE 100000000");

        // SIZE with 0 (no limit) should show just SIZE
        let size_unlimited = Extension::Size(0);
        assert_eq!(size_unlimited.to_string(), "SIZE");

        // Other extensions
        assert_eq!(
            Extension::Starttls(TlsContext {
                certificate: "..".into(),
                key: "..".into()
            })
            .to_string(),
            "STARTTLS"
        );
        assert_eq!(Extension::Help.to_string(), "HELP");
    }
}
