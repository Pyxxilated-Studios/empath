//! DMARC evaluation helpers.
//!
//! This provides a minimal DMARC evaluator that relies on existing SPF/DKIM
//! results stored in the transaction context and performs DNS TXT lookup
//! for `_dmarc.<domain>` records.

use std::fmt::{self, Display};

use empath_common::message_api::{HeaderName, HeadersView};

use crate::spf::{DnsLookupError, SpfDnsResolver};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DmarcResult {
    Pass,
    Fail,
    None,
    TempError,
    PermError,
}

impl DmarcResult {
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Fail => "fail",
            Self::None => "none",
            Self::TempError => "temperror",
            Self::PermError => "permerror",
        }
    }
}

impl Display for DmarcResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy)]
enum Alignment {
    Relaxed,
    Strict,
}

#[derive(Debug, Clone)]
struct DmarcRecord {
    policy: String,
    adkim: Alignment,
    aspf: Alignment,
}

#[derive(Debug, Clone)]
pub struct DmarcEvaluation {
    pub result: DmarcResult,
    pub policy: Option<String>,
    pub spf_aligned: bool,
    pub dkim_aligned: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DkimInfo {
    pub domain: String,
    pub selector: Option<String>,
}

pub async fn evaluate(
    resolver: &dyn SpfDnsResolver,
    from_domain: &str,
    mail_from_domain: Option<&str>,
    spf_result: Option<&str>,
    dkim_result: Option<&str>,
    dkim_domain: Option<&str>,
) -> DmarcEvaluation {
    let lookup_domain = format!("_dmarc.{from_domain}");
    let txt_lookup = match resolver.txt_lookup(&lookup_domain).await {
        Ok(records) => records,
        Err(err) => {
            let result = match err {
                DnsLookupError::Timeout(_) => DmarcResult::TempError,
                DnsLookupError::LookupFailed(_) => DmarcResult::PermError,
            };
            return DmarcEvaluation {
                result,
                policy: None,
                spf_aligned: false,
                dkim_aligned: false,
                error: Some(err.to_string()),
            };
        }
    };

    let dmarc_records: Vec<&str> = txt_lookup
        .iter()
        .map(String::as_str)
        .filter(|record| {
            record
                .trim_start()
                .to_ascii_lowercase()
                .starts_with("v=dmarc1")
        })
        .collect();

    let Some(record_value) = (match dmarc_records.len() {
        0 => None,
        1 => Some(dmarc_records[0]),
        _ => {
            return DmarcEvaluation {
                result: DmarcResult::PermError,
                policy: None,
                spf_aligned: false,
                dkim_aligned: false,
                error: Some("Multiple DMARC records found".to_string()),
            };
        }
    }) else {
        return DmarcEvaluation {
            result: DmarcResult::None,
            policy: None,
            spf_aligned: false,
            dkim_aligned: false,
            error: None,
        };
    };

    let record = match parse_record(record_value) {
        Ok(record) => record,
        Err(err) => {
            return DmarcEvaluation {
                result: DmarcResult::PermError,
                policy: None,
                spf_aligned: false,
                dkim_aligned: false,
                error: Some(err),
            };
        }
    };

    let spf_pass = spf_result.is_some_and(|value| value.eq_ignore_ascii_case("pass"));
    let dkim_pass = dkim_result.is_some_and(|value| value.eq_ignore_ascii_case("pass"));

    let spf_aligned = spf_pass && is_aligned(mail_from_domain, from_domain, record.aspf);
    let dkim_aligned = dkim_pass && is_aligned(dkim_domain, from_domain, record.adkim);

    let result = if spf_aligned || dkim_aligned {
        DmarcResult::Pass
    } else {
        DmarcResult::Fail
    };

    DmarcEvaluation {
        result,
        policy: Some(record.policy),
        spf_aligned,
        dkim_aligned,
        error: None,
    }
}

pub fn extract_from_domain(headers: &HeadersView<'_>) -> Option<String> {
    let from_header = HeaderName::new("From").ok()?;
    let header_value = headers.get(&from_header)?;
    let header_str = std::str::from_utf8(header_value.as_bytes()).ok()?;
    let address = extract_first_address(header_str)?;
    domain_from_address(address)
}

pub fn extract_dkim_info(headers: &HeadersView<'_>) -> Option<DkimInfo> {
    let dkim_header = HeaderName::new("DKIM-Signature").ok()?;
    for header_value in headers.get_all(&dkim_header) {
        let header_str = std::str::from_utf8(header_value.as_bytes()).ok()?;
        if let Some(info) = parse_dkim_signature(header_str) {
            return Some(info);
        }
    }
    None
}

fn extract_first_address(value: &str) -> Option<&str> {
    if let Some(start) = value.find('<')
        && let Some(end) = value[start + 1..].find('>')
    {
        let offset_end = start + 1 + end;
        let candidate = value.get(start + 1..offset_end)?;
        if candidate.contains('@') {
            return Some(candidate.trim());
        }
    }

    for token in value.split(|c: char| c == ',' || c.is_whitespace()) {
        let trimmed =
            token.trim_matches(|c: char| matches!(c, '<' | '>' | '"' | '\'' | '(' | ')' | ';'));
        if trimmed.contains('@') {
            return Some(trimmed);
        }
    }

    None
}

pub fn domain_from_address(address: &str) -> Option<String> {
    let domain = address.rsplit_once('@')?.1.trim();
    if domain.is_empty() {
        return None;
    }
    Some(domain.trim_end_matches('.').to_ascii_lowercase())
}

fn parse_record(record: &str) -> Result<DmarcRecord, String> {
    let mut policy = None;
    let mut adkim = Alignment::Relaxed;
    let mut aspf = Alignment::Relaxed;
    let mut saw_version = false;

    for part in record.split(';') {
        let trimmed = part.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Some((key, value)) = trimmed.split_once('=') else {
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        let value = value.trim();
        match key.as_str() {
            "v" => {
                saw_version = value.eq_ignore_ascii_case("DMARC1");
            }
            "p" => {
                if value.is_empty() {
                    return Err("DMARC record missing policy value".to_string());
                }
                policy = Some(value.to_ascii_lowercase());
            }
            "adkim" => {
                adkim = if value.eq_ignore_ascii_case("s") {
                    Alignment::Strict
                } else {
                    Alignment::Relaxed
                };
            }
            "aspf" => {
                aspf = if value.eq_ignore_ascii_case("s") {
                    Alignment::Strict
                } else {
                    Alignment::Relaxed
                };
            }
            _ => {}
        }
    }

    if !saw_version {
        return Err("DMARC record missing v=DMARC1 tag".to_string());
    }

    let Some(policy) = policy else {
        return Err("DMARC record missing p= tag".to_string());
    };

    Ok(DmarcRecord {
        policy,
        adkim,
        aspf,
    })
}

fn parse_dkim_signature(header_value: &str) -> Option<DkimInfo> {
    let mut domain = None;
    let mut selector = None;

    for part in header_value.split(';') {
        let trimmed = part.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Some((key, value)) = trimmed.split_once('=') else {
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        let value = value.trim();
        match key.as_str() {
            "d" => {
                if !value.is_empty() {
                    domain = Some(value.trim_end_matches('.').to_ascii_lowercase());
                }
            }
            "s" => {
                if !value.is_empty() {
                    selector = Some(value.to_ascii_lowercase());
                }
            }
            _ => {}
        }
    }

    domain.map(|domain| DkimInfo { domain, selector })
}

fn is_aligned(candidate: Option<&str>, from_domain: &str, mode: Alignment) -> bool {
    let Some(candidate) = candidate else {
        return false;
    };

    let candidate = candidate.trim_end_matches('.').to_ascii_lowercase();
    let from_domain = from_domain.trim_end_matches('.').to_ascii_lowercase();

    match mode {
        Alignment::Strict => candidate == from_domain,
        Alignment::Relaxed => relaxed_match(&candidate, &from_domain),
    }
}

fn relaxed_match(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    if a.ends_with(&format!(".{b}")) {
        return true;
    }
    b.ends_with(&format!(".{a}"))
}

#[cfg(test)]
mod tests {
    use empath_common::message_api::{HeaderName, HeaderValue};

    use super::*;

    type TestResult = std::result::Result<(), String>;

    #[test]
    fn extract_from_domain_prefers_first_address() -> TestResult {
        let name = HeaderName::new("From").map_err(|err| format!("{err:?}"))?;
        let value = HeaderValue::new(b"Alice <Alice@Example.COM>, Bob <bob@test.com>")
            .map_err(|err| format!("{err:?}"))?;
        let headers = vec![(name, value)];
        let view = HeadersView::new(&headers);

        let domain = extract_from_domain(&view);

        assert_eq!(domain.as_deref(), Some("example.com"));
        Ok(())
    }

    #[test]
    fn extract_dkim_info_parses_domain_and_selector() -> TestResult {
        let name = HeaderName::new("DKIM-Signature").map_err(|err| format!("{err:?}"))?;
        let value = HeaderValue::new(b"v=1; d=Example.COM.; s=Selector1;")
            .map_err(|err| format!("{err:?}"))?;
        let headers = vec![(name, value)];
        let view = HeadersView::new(&headers);

        let info = extract_dkim_info(&view).ok_or_else(|| "missing DKIM info".to_string())?;

        assert_eq!(info.domain, "example.com");
        assert_eq!(info.selector.as_deref(), Some("selector1"));
        Ok(())
    }

    #[test]
    fn domain_from_address_strips_trailing_dot_and_lowercases() -> TestResult {
        let domain =
            domain_from_address("user@Example.COM.").ok_or_else(|| "missing domain".to_string())?;

        assert_eq!(domain, "example.com");
        Ok(())
    }

    #[test]
    fn parse_record_applies_alignment_and_policy() -> Result<(), String> {
        let record = parse_record("v=DMARC1; p=reject; adkim=s; aspf=s")?;

        assert_eq!(record.policy, "reject");
        assert!(matches!(record.adkim, Alignment::Strict));
        assert!(matches!(record.aspf, Alignment::Strict));
        Ok(())
    }

    #[test]
    fn parse_record_defaults_to_relaxed_alignment() -> Result<(), String> {
        let record = parse_record("v=DMARC1; p=none")?;

        assert_eq!(record.policy, "none");
        assert!(matches!(record.adkim, Alignment::Relaxed));
        assert!(matches!(record.aspf, Alignment::Relaxed));
        Ok(())
    }

    #[test]
    fn parse_record_rejects_missing_required_tags() {
        assert!(
            parse_record("p=reject").is_err(),
            "missing version should be rejected"
        );
        assert!(
            parse_record("v=DMARC1").is_err(),
            "missing policy should be rejected"
        );
    }

    #[test]
    fn alignment_checks_match_strict_and_relaxed() {
        assert!(is_aligned(
            Some("mail.example.com"),
            "example.com",
            Alignment::Relaxed
        ));
        assert!(!is_aligned(
            Some("mail.example.com"),
            "example.com",
            Alignment::Strict
        ));
    }
}
