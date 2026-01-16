//! TLS context management for SMTP sessions.

use std::io as std_io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use hex::encode as hex_encode;
use parking_lot::RwLock;
use serde::de::Error as _;
use serde::{Deserialize, Deserializer};
use sha2::{Digest, Sha256};
use tokio_rustls::rustls::ServerConfig;
use tokio_rustls::rustls::pki_types::pem::{Error as PemError, PemObject};
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer};
use x509_parser::prelude::parse_x509_certificate;

use super::certificate::fill_certificate_details;
use crate::error::{TlsError, TlsResult};

/// TLS context for SMTP STARTTLS.
pub struct TlsContext {
    inner: Arc<TlsContextInner>,
}

#[derive(Debug)]
struct TlsContextInner {
    certificate: PathBuf,
    key: PathBuf,
    server_config: RwLock<Option<Arc<ServerConfig>>>,
}

/// TLS certificate metadata for control plane inspection.
#[derive(Debug, Clone)]
pub struct TlsContextInfo {
    /// Path to the configured certificate file.
    pub certificate_path: PathBuf,
    /// Path to the configured key file.
    pub key_path: PathBuf,
    /// SHA-256 fingerprint of the leaf certificate (hex).
    pub fingerprint_sha256: Option<String>,
    /// Subject DN string.
    pub subject: Option<String>,
    /// Issuer DN string.
    pub issuer: Option<String>,
    /// Not before timestamp (Unix seconds).
    pub not_before: Option<u64>,
    /// Not after timestamp (Unix seconds).
    pub not_after: Option<u64>,
    /// Subject alternative names (DNS/IP/URI/email).
    pub subject_alt_names: Vec<String>,
    /// Parse error if certificate inspection failed.
    pub parse_error: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct RawTlsContext {
    pub certificate: PathBuf,
    pub key: PathBuf,
}

impl Clone for TlsContext {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl std::fmt::Debug for TlsContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TlsContext")
            .field("certificate", &self.inner.certificate)
            .field("key", &self.inner.key)
            .finish()
    }
}

impl<'de> Deserialize<'de> for TlsContext {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = RawTlsContext::deserialize(deserializer)?;
        Self::new(raw.certificate, raw.key).map_err(D::Error::custom)
    }
}

impl TlsContext {
    /// Create a TLS context from certificate and key paths.
    ///
    /// # Errors
    ///
    /// Fails if the certificate or private key file cannot be read or parsed.
    pub fn new(certificate: PathBuf, key: PathBuf) -> TlsResult<Self> {
        Ok(Self {
            inner: Arc::new(TlsContextInner {
                certificate,
                key,
                server_config: RwLock::new(None),
            }),
        })
    }

    fn build_server_config(cert_path: &Path, key_path: &Path) -> TlsResult<ServerConfig> {
        let certificate = CertificateDer::from_pem_file(cert_path).map_err(|source| {
            TlsError::CertificateLoad {
                path: cert_path.display().to_string(),
                source: std_io::Error::new(std_io::ErrorKind::InvalidData, source),
            }
        })?;

        let key =
            PrivateKeyDer::from_pem_file(key_path).map_err(|err: PemError| TlsError::KeyLoad {
                path: key_path.display().to_string(),
                reason: err.to_string(),
            })?;

        ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![certificate], key)
            .map_err(|e| TlsError::Rustls(e.to_string()))
    }

    /// Returns the path to the certificate file.
    pub fn certificate_path(&self) -> &Path {
        &self.inner.certificate
    }

    /// Returns the path to the private key file.
    pub fn key_path(&self) -> &Path {
        &self.inner.key
    }

    /// Returns the server configuration, building it lazily if needed.
    ///
    /// # Errors
    ///
    /// Returns an error if the certificate or key cannot be loaded.
    pub fn server_config(&self) -> TlsResult<Arc<ServerConfig>> {
        {
            let guard = self.inner.server_config.read();
            if let Some(config) = guard.clone() {
                return Ok(config);
            }
        }

        let updated = Self::build_server_config(self.certificate_path(), self.key_path())?;
        let config = Arc::new(updated);

        {
            let mut guard = self.inner.server_config.write();
            *guard = Some(Arc::clone(&config));
        }

        Ok(config)
    }

    /// Reload the TLS configuration from disk.
    ///
    /// # Errors
    ///
    /// Returns an error if the certificate or key cannot be reloaded.
    pub fn reload(&self) -> TlsResult<()> {
        let updated = Self::build_server_config(self.certificate_path(), self.key_path())?;
        let config = Arc::new(updated);

        {
            let mut guard = self.inner.server_config.write();
            *guard = Some(Arc::clone(&config));
        }

        Ok(())
    }

    /// Inspect certificate metadata for this TLS context.
    #[must_use]
    pub fn inspect(&self) -> TlsContextInfo {
        let certificate_path = self.inner.certificate.clone();
        let key_path = self.inner.key.clone();

        let mut info = TlsContextInfo {
            certificate_path,
            key_path,
            fingerprint_sha256: None,
            subject: None,
            issuer: None,
            not_before: None,
            not_after: None,
            subject_alt_names: Vec::new(),
            parse_error: None,
        };

        let der = match CertificateDer::from_pem_file(&info.certificate_path) {
            Ok(cert) => cert,
            Err(err) => {
                info.parse_error = Some(format!("Failed to read certificate: {err}"));
                return info;
            }
        };

        let fingerprint = Sha256::digest(der.as_ref());
        info.fingerprint_sha256 = Some(hex_encode(fingerprint));

        let (_, cert) = match parse_x509_certificate(der.as_ref()) {
            Ok(parsed) => parsed,
            Err(err) => {
                info.parse_error = Some(format!("Failed to parse certificate: {err}"));
                return info;
            }
        };

        fill_certificate_details(&mut info, &cert);
        info
    }
}
