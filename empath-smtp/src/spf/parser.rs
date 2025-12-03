use ipnetwork::{Ipv4Network, Ipv6Network};
use serde::{Deserialize, Serialize};

use super::{error::SpfError, qualifiers::Qualifier};

/// Domain and CIDR specification for SPF mechanisms
#[derive(Debug, Default)]
struct DomainCidrSpec {
    domain: Option<String>,
    cidr4: Option<u8>,
    cidr6: Option<u8>,
}

/// Parsed SPF record (RFC 7208)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpfRecord {
    /// SPF version (always "spf1" for RFC 7208)
    pub version: String,
    /// List of mechanisms
    pub mechanisms: Vec<Mechanism>,
    /// List of modifiers
    pub modifiers: Vec<Modifier>,
}

/// SPF mechanism (RFC 7208 Section 5)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mechanism {
    /// `all` - always matches
    All(Qualifier),
    /// `ip4:<network>` - IPv4 address or CIDR
    Ip4 {
        qualifier: Qualifier,
        network: Ipv4Network,
    },
    /// `ip6:<network>` - IPv6 address or CIDR
    Ip6 {
        qualifier: Qualifier,
        network: Ipv6Network,
    },
    /// `a[:<domain>][/<cidr>]` - A/AAAA record lookup
    A {
        qualifier: Qualifier,
        domain: Option<String>,
        cidr4: Option<u8>,
        cidr6: Option<u8>,
    },
    /// `mx[:<domain>][/<cidr>]` - MX record lookup
    Mx {
        qualifier: Qualifier,
        domain: Option<String>,
        cidr4: Option<u8>,
        cidr6: Option<u8>,
    },
    /// `ptr[:<domain>]` - PTR record validation (deprecated but still supported)
    Ptr {
        qualifier: Qualifier,
        domain: Option<String>,
    },
    /// `exists:<domain>` - A record existence check
    Exists {
        qualifier: Qualifier,
        domain: String,
    },
    /// `include:<domain>` - Recursive SPF evaluation
    Include {
        qualifier: Qualifier,
        domain: String,
    },
}

/// SPF modifier (RFC 7208 Section 6)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Modifier {
    /// `redirect=<domain>` - Redirect to another domain's SPF record
    Redirect(String),
    /// `exp=<domain>` - Explanation for failures
    Exp(String),
    /// Unknown modifier (name=value)
    Unknown(String, String),
}

impl SpfRecord {
    /// Parse an SPF record from a TXT record string
    ///
    /// # Errors
    ///
    /// Returns `SpfError::InvalidRecord` if the record is malformed
    pub fn parse(txt_record: &str) -> Result<Self, SpfError> {
        let txt_record = txt_record.trim();

        // Must start with "v=spf1"
        if !txt_record.starts_with("v=spf1") {
            return Err(SpfError::InvalidRecord(
                "SPF record must start with 'v=spf1'".to_string(),
            ));
        }

        let mut mechanisms = Vec::new();
        let mut modifiers = Vec::new();

        // Split into terms (space-separated)
        for term in txt_record.split_whitespace().skip(1) {
            // Skip "v=spf1"
            if term.is_empty() {
                continue;
            }

            // Check if it's a modifier (contains '=')
            if term.contains('=') && !term.starts_with(|c: char| "+-~?".contains(c)) {
                modifiers.push(Self::parse_modifier(term)?);
            } else {
                mechanisms.push(Self::parse_mechanism(term)?);
            }
        }

        Ok(Self {
            version: "spf1".to_string(),
            mechanisms,
            modifiers,
        })
    }

    fn parse_mechanism(term: &str) -> Result<Mechanism, SpfError> {
        // Extract qualifier (first character if it's a qualifier symbol)
        let (qualifier, term) = if let Some(first_char) = term.chars().next() {
            if "+-~?".contains(first_char) {
                (Qualifier::from_char(first_char), &term[1..])
            } else {
                (Qualifier::Pass, term)
            }
        } else {
            return Err(SpfError::InvalidRecord("Empty mechanism".to_string()));
        };

        // Split mechanism name and value (if present)
        // For mechanisms like "a/24" or "mx/24", the slash indicates CIDR without domain
        #[allow(clippy::option_if_let_else)]
        let (mech_name, mech_value) = if let Some(colon_pos) = term.find(':') {
            (&term[..colon_pos], Some(&term[colon_pos + 1..]))
        } else if let Some(slash_pos) = term.find('/') {
            // Handle "a/24" format - mechanism name before slash, CIDR after
            let name = &term[..slash_pos];
            let cidr = &term[slash_pos..]; // Include the slash
            (name, Some(cidr))
        } else {
            (term, None)
        };

        // Parse based on mechanism type
        match mech_name {
            "all" => Ok(Mechanism::All(qualifier)),

            "ip4" => {
                let value = mech_value
                    .ok_or_else(|| SpfError::InvalidRecord("ip4 requires a value".to_string()))?;
                let network: Ipv4Network = value
                    .parse()
                    .map_err(|e| SpfError::InvalidRecord(format!("Invalid IPv4 network: {e}")))?;
                Ok(Mechanism::Ip4 { qualifier, network })
            }

            "ip6" => {
                let value = mech_value
                    .ok_or_else(|| SpfError::InvalidRecord("ip6 requires a value".to_string()))?;
                let network: Ipv6Network = value
                    .parse()
                    .map_err(|e| SpfError::InvalidRecord(format!("Invalid IPv6 network: {e}")))?;
                Ok(Mechanism::Ip6 { qualifier, network })
            }

            "a" => {
                let spec = Self::parse_domain_cidr(mech_value)?;
                Ok(Mechanism::A {
                    qualifier,
                    domain: spec.domain,
                    cidr4: spec.cidr4,
                    cidr6: spec.cidr6,
                })
            }

            "mx" => {
                let spec = Self::parse_domain_cidr(mech_value)?;
                Ok(Mechanism::Mx {
                    qualifier,
                    domain: spec.domain,
                    cidr4: spec.cidr4,
                    cidr6: spec.cidr6,
                })
            }

            "ptr" => {
                let domain = mech_value.map(std::string::ToString::to_string);
                Ok(Mechanism::Ptr { qualifier, domain })
            }

            "exists" => {
                let domain = mech_value
                    .ok_or_else(|| SpfError::InvalidRecord("exists requires a domain".to_string()))?
                    .to_string();
                Ok(Mechanism::Exists { qualifier, domain })
            }

            "include" => {
                let domain = mech_value
                    .ok_or_else(|| {
                        SpfError::InvalidRecord("include requires a domain".to_string())
                    })?
                    .to_string();
                Ok(Mechanism::Include { qualifier, domain })
            }

            _ => Err(SpfError::InvalidRecord(format!(
                "Unknown mechanism: {mech_name}"
            ))),
        }
    }

    fn parse_domain_cidr(value: Option<&str>) -> Result<DomainCidrSpec, SpfError> {
        let Some(value) = value else {
            return Ok(DomainCidrSpec::default());
        };

        // Split domain and CIDR (if present)
        let (domain, cidr_part) = value.find('/').map_or_else(
            || (Some(value), None),
            |slash_pos| (Some(&value[..slash_pos]), Some(&value[slash_pos + 1..])),
        );

        let domain = if domain == Some("") {
            None
        } else {
            domain.map(std::string::ToString::to_string)
        };

        // Parse CIDR - can be single value or dual (IPv4/IPv6)
        let (cidr4, cidr6) = if let Some(cidr_str) = cidr_part {
            if let Some(slash_pos) = cidr_str.find('/') {
                // Dual CIDR: /24/64
                let cidr4_str = &cidr_str[..slash_pos];
                let cidr6_str = &cidr_str[slash_pos + 1..];
                let cidr4 = cidr4_str
                    .parse()
                    .map_err(|e| SpfError::InvalidRecord(format!("Invalid IPv4 CIDR: {e}")))?;
                let cidr6 = cidr6_str
                    .parse()
                    .map_err(|e| SpfError::InvalidRecord(format!("Invalid IPv6 CIDR: {e}")))?;
                (Some(cidr4), Some(cidr6))
            } else {
                // Single CIDR applies to both IPv4 and IPv6
                let cidr: u8 = cidr_str
                    .parse()
                    .map_err(|e| SpfError::InvalidRecord(format!("Invalid CIDR: {e}")))?;
                (Some(cidr), Some(cidr))
            }
        } else {
            (None, None)
        };

        Ok(DomainCidrSpec {
            domain,
            cidr4,
            cidr6,
        })
    }

    fn parse_modifier(term: &str) -> Result<Modifier, SpfError> {
        let Some(eq_pos) = term.find('=') else {
            return Err(SpfError::InvalidRecord(format!(
                "Modifier must contain '=': {term}"
            )));
        };

        let name = &term[..eq_pos];
        let value = &term[eq_pos + 1..];

        if value.is_empty() {
            return Err(SpfError::InvalidRecord(format!(
                "Modifier value cannot be empty: {name}"
            )));
        }

        match name {
            "redirect" => Ok(Modifier::Redirect(value.to_string())),
            "exp" => Ok(Modifier::Exp(value.to_string())),
            _ => Ok(Modifier::Unknown(name.to_string(), value.to_string())),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_spf() {
        let record = SpfRecord::parse("v=spf1 mx -all").unwrap();
        assert_eq!(record.version, "spf1");
        assert_eq!(record.mechanisms.len(), 2);
        assert!(matches!(record.mechanisms[0], Mechanism::Mx { .. }));
        assert_eq!(record.mechanisms[1], Mechanism::All(Qualifier::Fail));
    }

    #[test]
    fn test_parse_ip4_mechanism() {
        let record = SpfRecord::parse("v=spf1 ip4:192.0.2.0/24 -all").unwrap();
        assert_eq!(record.mechanisms.len(), 2);
        if let Mechanism::Ip4 { qualifier, network } = &record.mechanisms[0] {
            assert_eq!(*qualifier, Qualifier::Pass);
            assert_eq!(network.to_string(), "192.0.2.0/24");
        } else {
            panic!("Expected Ip4 mechanism");
        }
    }

    #[test]
    fn test_parse_ip6_mechanism() {
        let record = SpfRecord::parse("v=spf1 ip6:2001:db8::/32 -all").unwrap();
        assert_eq!(record.mechanisms.len(), 2);
        if let Mechanism::Ip6 { qualifier, network } = &record.mechanisms[0] {
            assert_eq!(*qualifier, Qualifier::Pass);
            assert_eq!(network.to_string(), "2001:db8::/32");
        } else {
            panic!("Expected Ip6 mechanism");
        }
    }

    #[test]
    fn test_parse_qualifiers() {
        let record = SpfRecord::parse("v=spf1 +mx -ip4:192.0.2.1 ~all").unwrap();
        assert_eq!(record.mechanisms.len(), 3);

        if let Mechanism::Mx { qualifier, .. } = record.mechanisms[0] {
            assert_eq!(qualifier, Qualifier::Pass);
        }

        if let Mechanism::Ip4 { qualifier, .. } = record.mechanisms[1] {
            assert_eq!(qualifier, Qualifier::Fail);
        }

        if let Mechanism::All(qualifier) = record.mechanisms[2] {
            assert_eq!(qualifier, Qualifier::SoftFail);
        }
    }

    #[test]
    fn test_parse_a_mechanism_with_domain() {
        let record = SpfRecord::parse("v=spf1 a:example.com -all").unwrap();
        if let Mechanism::A {
            domain,
            cidr4,
            cidr6,
            ..
        } = &record.mechanisms[0]
        {
            assert_eq!(domain.as_deref(), Some("example.com"));
            assert_eq!(*cidr4, None);
            assert_eq!(*cidr6, None);
        } else {
            panic!("Expected A mechanism");
        }
    }

    #[test]
    fn test_parse_a_mechanism_with_cidr() {
        let record = SpfRecord::parse("v=spf1 a/24 -all").unwrap();
        if let Mechanism::A {
            domain,
            cidr4,
            cidr6,
            ..
        } = &record.mechanisms[0]
        {
            assert_eq!(*domain, None);
            assert_eq!(*cidr4, Some(24));
            assert_eq!(*cidr6, Some(24));
        } else {
            panic!("Expected A mechanism");
        }
    }

    #[test]
    fn test_parse_a_mechanism_with_dual_cidr() {
        let record = SpfRecord::parse("v=spf1 a:example.com/24/64 -all").unwrap();
        if let Mechanism::A {
            domain,
            cidr4,
            cidr6,
            ..
        } = &record.mechanisms[0]
        {
            assert_eq!(domain.as_deref(), Some("example.com"));
            assert_eq!(*cidr4, Some(24));
            assert_eq!(*cidr6, Some(64));
        } else {
            panic!("Expected A mechanism");
        }
    }

    #[test]
    fn test_parse_include_mechanism() {
        let record = SpfRecord::parse("v=spf1 include:_spf.example.com -all").unwrap();
        if let Mechanism::Include { domain, .. } = &record.mechanisms[0] {
            assert_eq!(domain, "_spf.example.com");
        } else {
            panic!("Expected Include mechanism");
        }
    }

    #[test]
    fn test_parse_exists_mechanism() {
        let record = SpfRecord::parse("v=spf1 exists:%{i}._spf.example.com -all").unwrap();
        if let Mechanism::Exists { domain, .. } = &record.mechanisms[0] {
            assert_eq!(domain, "%{i}._spf.example.com");
        } else {
            panic!("Expected Exists mechanism");
        }
    }

    #[test]
    fn test_parse_redirect_modifier() {
        let record = SpfRecord::parse("v=spf1 redirect=_spf.example.com").unwrap();
        assert_eq!(record.modifiers.len(), 1);
        if let Modifier::Redirect(domain) = &record.modifiers[0] {
            assert_eq!(domain, "_spf.example.com");
        } else {
            panic!("Expected Redirect modifier");
        }
    }

    #[test]
    fn test_parse_exp_modifier() {
        let record = SpfRecord::parse("v=spf1 mx -all exp=explain.example.com").unwrap();
        assert_eq!(record.modifiers.len(), 1);
        if let Modifier::Exp(domain) = &record.modifiers[0] {
            assert_eq!(domain, "explain.example.com");
        } else {
            panic!("Expected Exp modifier");
        }
    }

    #[test]
    fn test_parse_unknown_modifier() {
        let record = SpfRecord::parse("v=spf1 mx custom=value -all").unwrap();
        assert_eq!(record.modifiers.len(), 1);
        if let Modifier::Unknown(name, value) = &record.modifiers[0] {
            assert_eq!(name, "custom");
            assert_eq!(value, "value");
        } else {
            panic!("Expected Unknown modifier");
        }
    }

    #[test]
    fn test_parse_invalid_version() {
        let result = SpfRecord::parse("v=spf2 mx -all");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_complex_record() {
        let record = SpfRecord::parse(
            "v=spf1 ip4:192.0.2.0/24 ip6:2001:db8::/32 a mx include:_spf.example.com -all",
        )
        .unwrap();
        assert_eq!(record.mechanisms.len(), 6);
        assert!(matches!(record.mechanisms[0], Mechanism::Ip4 { .. }));
        assert!(matches!(record.mechanisms[1], Mechanism::Ip6 { .. }));
        assert!(matches!(record.mechanisms[2], Mechanism::A { .. }));
        assert!(matches!(record.mechanisms[3], Mechanism::Mx { .. }));
        assert!(matches!(record.mechanisms[4], Mechanism::Include { .. }));
        assert!(matches!(
            record.mechanisms[5],
            Mechanism::All(Qualifier::Fail)
        ));
    }
}
