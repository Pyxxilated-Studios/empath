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

    const TEST_KEY: &str = r"-----BEGIN PRIVATE KEY-----
MIIEvgIBADANBgkqhkiG9w0BAQEFAASCBKgwggSkAgEAAoIBAQClhytI0tUN9jCj
lhaMJNlrMoaPwkZTTX27vKgZpYvXBSX/5f92NMtQKY07XoIMqpfuaEv+vlo+TSNs
SeDnf4i/Ilel6ZTCL2hh2aH0bfUX7PeH1AVH/oQcW2bbW+TqIjymE8zSVpFdmSAe
8ZAVU8jerlhC1NytvWfUOxDW4d/mAyucBE8FYkT2xcBGQGPsm11S7/8cjJveIrun
wwzina5I2AsnkqaNABMEPhr1VyM6fIKAVuBhmZ+Pru8itc+98iT/Aguh5wGth3JN
76MLcdbM7wSwdc0W1wY/sVaAADTe46w4nzld8EXirTn9oVkYS2qYRgoxvDYPygmR
5ERA0wO7AgMBAAECggEARSWHBl/UUoXCYcPD3X1TOb2cipc8a3YWjQNNMZF/Li5f
Zlu88LMzL2R8NA3Rw9rLpvL4tAzug9ITdxwRkIMtK7L77Dn8Q3YsY5vXVNVOKlV6
Pcpq/l0q8PGZsQ7dPJVawoHI8SDqjq/1xBdi+hXK9oLcKebj3MwfgH/AFBO9RUbn
QhPMljEislHIMEL5h/aVluyJkpJdtIKUwgMiXD0ygwvpDkMhIoYJ7NIFhQ8pua7w
p99NwY2VxeyOU5y0NN+05PkHXxHPIHqL98vPKXkVQUy/6pQi8RU+9hBaXf8eKRrp
jxBj2nazAE+EESNlqnRomH4L0DDaU69Z2wHjenBuqQKBgQDeg0+79BrHcgF6Jyjz
SwAtmvpe1LkKs7h3HIQVvEvNYdsphgP8w3rteeDsX/5dhmIemirPA30Qio81b4YH
LTf7E8qluB/PmIGlVcGRq5QBuoBOVSrabk7/zWvs1+NBRD3yuerqdrLkuM2ln8m/
ms2GUYyThkNcMepWNqzGgm6IvQKBgQC+cGqaP8PnwAc/8Kmpg3LLbN7VUWSbRA15
4qZkdVk7DVwqjqhH8UTrnOXjoC2XfiRfs8ovmbhvdLb0lMIseXaDFTGA+UyHS1hd
dOLGUkcBLTtuDWFlAjRyQAtgfVGa6JiNYwt4B6xEh+3rmr/sxVWKwslR4Y+WbqXQ
OoTdjzox1wKBgBeIRZ94EnjzWqqjh5YrYWpxUHdFAgjatvCJ6IIv9shSyA3dKYDK
365LzHjTMiCL/MaFwW9BTDG6paieEcuigaAhGLZsAZk/5L4qkp0oSy/fnIAsS6rs
LZiqDfbvYg5d/Ds2ZFzFjufXj5ArlPeZlJDXAQn9EhSnBsR1ZdJcqvn1AoGBALiT
Pk8YJGkWrjAjsl0QgQ93CpaYF7mBAMq0iO7u6bgZX84DjEiGKHVwEURIeEfPQ7Pa
DsmJ5L9p0PVGxNfUNbAgt2U4uxqaRR/3bOAo2JbkXyhwIYiD1Y8yUZSIgbJxY4IN
kA0a6rVVRZzH1OLH8/vUhMkupDt6LI2gWixfaFjrAoGBANSk312/2+nyJnj/MIad
YBmIZkZFdubSgxP+rjeylqFaA5Mp1ODBor/hSP76GG5spz0u+TTPl8O+dsQ1rmnH
lN0wx0T+yUKfUkcJxsozyXtiFEojrmUZ3PtfEwt1Ft0VOzlvDoOC/fVeiE28GEod
lKIQPRVaVA85Za5RNTMsLXtE
-----END PRIVATE KEY-----";

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
