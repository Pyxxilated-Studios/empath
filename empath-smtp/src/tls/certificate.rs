//! Certificate inspection utilities.

use hex::encode as hex_encode;
use x509_parser::prelude::X509Certificate;

use super::TlsContextInfo;

pub(super) fn fill_certificate_details(info: &mut TlsContextInfo, cert: &X509Certificate<'_>) {
    info.subject = Some(cert.subject().to_string());
    info.issuer = Some(cert.issuer().to_string());
    info.not_before = as_unix_timestamp(cert.validity().not_before);
    info.not_after = as_unix_timestamp(cert.validity().not_after);
    info.subject_alt_names = collect_subject_alt_names(cert);
}

fn as_unix_timestamp(time: x509_parser::prelude::ASN1Time) -> Option<u64> {
    let timestamp = time.timestamp();
    if timestamp <= 0 {
        None
    } else {
        u64::try_from(timestamp).ok()
    }
}

fn collect_subject_alt_names(cert: &X509Certificate<'_>) -> Vec<String> {
    let mut names = Vec::new();

    let Ok(Some(san)) = cert.subject_alternative_name() else {
        return names;
    };

    for name in &san.value.general_names {
        let value = match name {
            x509_parser::extensions::GeneralName::DNSName(dns) => dns.to_string(),
            x509_parser::extensions::GeneralName::RFC822Name(email) => email.to_string(),
            x509_parser::extensions::GeneralName::URI(uri) => uri.to_string(),
            x509_parser::extensions::GeneralName::IPAddress(addr) => {
                format_ip_address(addr).unwrap_or_else(|| hex_encode(addr))
            }
            other => format!("{other:?}"),
        };
        names.push(value);
    }

    names
}

fn format_ip_address(bytes: &[u8]) -> Option<String> {
    match bytes.len() {
        4 => {
            let octets = <[u8; 4]>::try_from(bytes).ok()?;
            Some(std::net::Ipv4Addr::from(octets).to_string())
        }
        16 => {
            let octets = <[u8; 16]>::try_from(bytes).ok()?;
            Some(std::net::Ipv6Addr::from(octets).to_string())
        }
        _ => None,
    }
}
