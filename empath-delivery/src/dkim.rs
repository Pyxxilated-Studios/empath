//! DKIM signing support for outbound delivery.

use std::path::PathBuf;

use mail_auth::common::crypto::{RsaKey, Sha256};
use mail_auth::common::headers::HeaderWriter;
use mail_auth::dkim::{DkimSigner, Done};
use rustls_pki_types::pem::PemObject;
use rustls_pki_types::{PrivateKeyDer, PrivatePkcs1KeyDer, PrivatePkcs8KeyDer};
use serde::Deserialize;
use tokio::io::{AsyncRead, AsyncReadExt};

use crate::error::{DeliveryError, SystemError};

fn default_signing_headers() -> Vec<String> {
    vec![
        "From".to_string(),
        "To".to_string(),
        "Subject".to_string(),
        "Date".to_string(),
        "Message-ID".to_string(),
    ]
}

#[derive(Debug, Clone, Deserialize)]
pub struct DkimSigningConfig {
    pub domain: String,
    pub selector: String,
    pub private_key_path: PathBuf,
    #[serde(default = "default_signing_headers")]
    pub headers: Vec<String>,
}

pub struct DkimSigningKey {
    signer: DkimSigner<RsaKey<Sha256>, Done>,
}

impl std::fmt::Debug for DkimSigningKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DkimSigningKey").finish()
    }
}

impl DkimSigningKey {
    pub const fn signer(&self) -> &DkimSigner<RsaKey<Sha256>, Done> {
        &self.signer
    }
}

impl DkimSigningConfig {
    /// # Errors
    /// Returns `DeliveryError` if the signing key cannot be loaded.
    pub fn build_signer(&self) -> Result<DkimSigningKey, DeliveryError> {
        let key_bytes = read_private_key_bytes(&self.private_key_path)?;
        build_signer_from_bytes(&self.domain, &self.selector, &self.headers, &key_bytes)
    }
}

pub async fn sign_message_stream(
    signer: &DkimSigner<RsaKey<Sha256>, Done>,
    reader: &mut (impl AsyncRead + Unpin),
) -> Result<Vec<u8>, DeliveryError> {
    let mut stream = signer.sign_streaming();
    let mut buffer = [0u8; 8192];
    loop {
        let read = reader.read(&mut buffer).await.map_err(SystemError::Io)?;
        if read == 0 {
            break;
        }
        stream.write(&buffer[..read]);
    }

    let signature = stream.finish().map_err(|err| map_sign_error(&err))?;
    Ok(signature.to_header().into_bytes())
}

#[cfg(test)]
fn sign_message_bytes(
    signer: &DkimSigner<RsaKey<Sha256>, Done>,
    message: &[u8],
) -> Result<Vec<u8>, DeliveryError> {
    let signature = signer.sign(message).map_err(|err| map_sign_error(&err))?;
    Ok(signature.to_header().into_bytes())
}

fn load_rsa_key(key_bytes: &[u8]) -> Result<RsaKey<Sha256>, DeliveryError> {
    if let Ok(pkcs1) = PrivatePkcs1KeyDer::from_pem_slice(key_bytes) {
        return RsaKey::from_key_der(PrivateKeyDer::Pkcs1(pkcs1))
            .map_err(|err| SystemError::Configuration(format!("Invalid DKIM key: {err}")).into());
    }

    if let Ok(pkcs8) = PrivatePkcs8KeyDer::from_pem_slice(key_bytes) {
        return RsaKey::from_key_der(PrivateKeyDer::Pkcs8(pkcs8))
            .map_err(|err| SystemError::Configuration(format!("Invalid DKIM key: {err}")).into());
    }

    let pkcs1 = PrivatePkcs1KeyDer::from(key_bytes);
    if let Ok(key) = RsaKey::from_key_der(PrivateKeyDer::Pkcs1(pkcs1)) {
        return Ok(key);
    }

    let pkcs8 = PrivatePkcs8KeyDer::from(key_bytes);
    if let Ok(key) = RsaKey::from_key_der(PrivateKeyDer::Pkcs8(pkcs8)) {
        return Ok(key);
    }

    Err(SystemError::Configuration(
        "Unsupported DKIM key format (expected PKCS1 or PKCS8 PEM/DER)".to_string(),
    )
    .into())
}

pub fn build_signer_from_bytes(
    domain: &str,
    selector: &str,
    headers: &[String],
    key_bytes: &[u8],
) -> Result<DkimSigningKey, DeliveryError> {
    let key = load_rsa_key(key_bytes)?;
    let signer = DkimSigner::from_key(key)
        .domain(domain.to_string())
        .selector(selector.to_string())
        .headers(headers.iter().map(String::as_str));

    Ok(DkimSigningKey { signer })
}

fn read_private_key_bytes(path: &PathBuf) -> Result<Vec<u8>, DeliveryError> {
    Ok(std::fs::read(path).map_err(SystemError::Io)?)
}

fn map_sign_error(err: &mail_auth::Error) -> DeliveryError {
    SystemError::Configuration(format!("DKIM signing failed: {err}")).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_KEY: &str = r"-----BEGIN RSA PRIVATE KEY-----
MIICXwIBAAKBgQDwIRP/UC3SBsEmGqZ9ZJW3/DkMoGeLnQg1fWn7/zYtIxN2SnFC
jxOCKG9v3b4jYfcTNh5ijSsq631uBItLa7od+v/RtdC2UzJ1lWT947qR+Rcac2gb
to/NMqJ0fzfVjH4OuKhitdY9tf6mcwGjaNBcWToIMmPSPDdQPNUYckcQ2QIDAQAB
AoGBALmn+XwWk7akvkUlqb+dOxyLB9i5VBVfje89Teolwc9YJT36BGN/l4e0l6QX
/1//6DWUTB3KI6wFcm7TWJcxbS0tcKZX7FsJvUz1SbQnkS54DJck1EZO/BLa5ckJ
gAYIaqlA9C0ZwM6i58lLlPadX/rtHb7pWzeNcZHjKrjM461ZAkEA+itss2nRlmyO
n1/5yDyCluST4dQfO8kAB3toSEVc7DeFeDhnC1mZdjASZNvdHS4gbLIA1hUGEF9m
3hKsGUMMPwJBAPW5v/U+AWTADFCS22t72NUurgzeAbzb1HWMqO4y4+9Hpjk5wvL/
eVYizyuce3/fGke7aRYw/ADKygMJdW8H/OcCQQDz5OQb4j2QDpPZc0Nc4QlbvMsj
7p7otWRO5xRa6SzXqqV3+F0VpqvDmshEBkoCydaYwc2o6WQ5EBmExeV8124XAkEA
qZzGsIxVP+sEVRWZmW6KNFSdVUpk3qzK0Tz/WjQMe5z0UunY9Ax9/4PVhp/j61bf
eAYXunajbBSOLlx4D+TunwJBANkPI5S9iylsbLs6NkaMHV6k5ioHBBmgCak95JGX
GMot/L2x0IYyMLAz6oLWh2hm7zwtb0CgOrPo1ke44hFYnfc=
-----END RSA PRIVATE KEY-----";

    const TEST_MESSAGE: &str = "From: bill@example.com\r\n\
To: jdoe@example.com\r\n\
Subject: TPS Report\r\n\
Date: Thu, 01 Jan 1970 00:00:00 +0000\r\n\
Message-ID: <test@example.com>\r\n\
\r\n\
TPS reports ASAP.\r\n";

    #[test]
    fn signs_message_bytes_with_header() -> Result<(), DeliveryError> {
        let config = DkimSigningConfig {
            domain: "example.com".to_string(),
            selector: "default".to_string(),
            private_key_path: PathBuf::from("unused"),
            headers: default_signing_headers(),
        };
        let signer = build_signer_from_bytes(
            &config.domain,
            &config.selector,
            &config.headers,
            TEST_KEY.as_bytes(),
        )?;
        let header = sign_message_bytes(signer.signer(), TEST_MESSAGE.as_bytes())?;
        let header_str = String::from_utf8_lossy(&header);
        assert!(header_str.starts_with("DKIM-Signature: "));
        assert!(header_str.contains(" d=example.com;"));
        assert!(header_str.contains(" s=default;"));
        Ok(())
    }
}
