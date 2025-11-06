//! SMTP client implementation with support for TLS and STARTTLS.

use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::rustls::{ClientConfig, RootCertStore};
use tokio_rustls::TlsConnector;

use empath_common::tracing;

use super::error::{ClientError, Result};
use super::response::Response;

/// Initial size of the read buffer for SMTP responses.
const BUFFER_SIZE: usize = 8192;

/// Maximum size of the read buffer to prevent unbounded growth (1MB).
const MAX_BUFFER_SIZE: usize = 1024 * 1024;

/// An SMTP client connection that can be either plain TCP or TLS-wrapped.
enum ClientConnection {
    Plain(TcpStream),
    Tls(tokio_rustls::client::TlsStream<TcpStream>),
}

impl ClientConnection {
    /// Sends data over the connection.
    async fn send(&mut self, data: &[u8]) -> Result<()> {
        match self {
            Self::Plain(stream) => stream.write_all(data).await?,
            Self::Tls(stream) => stream.write_all(data).await?,
        }
        Ok(())
    }

    /// Reads data from the connection into the provided buffer.
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        let n = match self {
            Self::Plain(stream) => stream.read(buf).await?,
            Self::Tls(stream) => stream.read(buf).await?,
        };
        if n == 0 {
            return Err(ClientError::ConnectionClosed);
        }
        Ok(n)
    }

    /// Upgrades a plain connection to TLS.
    async fn upgrade_to_tls(
        self,
        domain: &str,
        accept_invalid_certs: bool,
    ) -> Result<Self> {
        match self {
            Self::Plain(stream) => {
                let mut root_store = RootCertStore::empty();

                // Add system certificates
                let certs = rustls_native_certs::load_native_certs();
                for cert in certs.certs {
                    root_store.add(cert).map_err(|e| {
                        ClientError::TlsError(format!("Failed to add certificate: {e}"))
                    })?;
                }
                // Log errors but don't fail if some certs couldn't be loaded
                if !certs.errors.is_empty() {
                    tracing::warn!(?certs.errors, "Some certificates could not be loaded");
                }

                let mut config = ClientConfig::builder()
                    .with_root_certificates(root_store)
                    .with_no_client_auth();

                // For testing purposes, allow invalid certificates if requested
                if accept_invalid_certs {
                    config
                        .dangerous()
                        .set_certificate_verifier(Arc::new(NoVerifier));
                }

                let connector = TlsConnector::from(Arc::new(config));
                let server_name = ServerName::try_from(domain.to_string())
                    .map_err(|e| ClientError::TlsError(format!("Invalid domain: {e}")))?;

                let tls_stream = connector
                    .connect(server_name, stream)
                    .await
                    .map_err(|e| ClientError::TlsError(e.to_string()))?;

                Ok(Self::Tls(tls_stream))
            }
            Self::Tls(_) => Err(ClientError::TlsError(
                "Connection is already TLS".to_string(),
            )),
        }
    }
}

/// A certificate verifier that accepts all certificates (for testing only).
#[derive(Debug)]
struct NoVerifier;

impl tokio_rustls::rustls::client::danger::ServerCertVerifier for NoVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &tokio_rustls::rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[tokio_rustls::rustls::pki_types::CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: tokio_rustls::rustls::pki_types::UnixTime,
    ) -> std::result::Result<
        tokio_rustls::rustls::client::danger::ServerCertVerified,
        tokio_rustls::rustls::Error,
    > {
        Ok(tokio_rustls::rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &tokio_rustls::rustls::pki_types::CertificateDer<'_>,
        _dss: &tokio_rustls::rustls::DigitallySignedStruct,
    ) -> std::result::Result<
        tokio_rustls::rustls::client::danger::HandshakeSignatureValid,
        tokio_rustls::rustls::Error,
    > {
        Ok(tokio_rustls::rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &tokio_rustls::rustls::pki_types::CertificateDer<'_>,
        _dss: &tokio_rustls::rustls::DigitallySignedStruct,
    ) -> std::result::Result<
        tokio_rustls::rustls::client::danger::HandshakeSignatureValid,
        tokio_rustls::rustls::Error,
    > {
        Ok(tokio_rustls::rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<tokio_rustls::rustls::SignatureScheme> {
        vec![
            tokio_rustls::rustls::SignatureScheme::RSA_PKCS1_SHA256,
            tokio_rustls::rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
            tokio_rustls::rustls::SignatureScheme::ED25519,
        ]
    }
}

/// An SMTP client for sending commands and receiving responses.
pub struct SmtpClient {
    connection: Option<ClientConnection>,
    buffer: Vec<u8>,
    buffer_pos: usize,
    responses: Vec<Response>,
    server_domain: String,
    accept_invalid_certs: bool,
}

impl SmtpClient {
    /// Creates a new SMTP client by connecting to the specified address.
    ///
    /// # Errors
    ///
    /// Returns an error if the connection fails.
    pub async fn connect(addr: &str, server_domain: String) -> Result<Self> {
        let stream = TcpStream::connect(addr)
            .await
            .map_err(ClientError::Io)?;

        Ok(Self {
            connection: Some(ClientConnection::Plain(stream)),
            buffer: vec![0u8; BUFFER_SIZE],
            buffer_pos: 0,
            responses: Vec::new(),
            server_domain,
            accept_invalid_certs: false, // Default to false for security
        })
    }

    /// Sets whether to accept invalid TLS certificates.
    ///
    /// This is useful for testing with self-signed certificates.
    /// Default is `false` for security. Set to `true` for testing only.
    #[must_use]
    pub const fn accept_invalid_certs(mut self, accept: bool) -> Self {
        self.accept_invalid_certs = accept;
        self
    }

    /// Reads the initial server greeting (220 response).
    ///
    /// # Errors
    ///
    /// Returns an error if reading fails or the greeting is invalid.
    pub async fn read_greeting(&mut self) -> Result<Response> {
        self.read_response().await
    }

    /// Sends a command to the server.
    ///
    /// # Errors
    ///
    /// Returns an error if sending fails.
    pub async fn send_command(&mut self, command: &str) -> Result<()> {
        let data = format!("{command}\r\n");
        self.connection
            .as_mut()
            .ok_or(ClientError::ConnectionClosed)?
            .send(data.as_bytes())
            .await?;
        Ok(())
    }

    /// Sends a raw command and reads the response.
    ///
    /// # Errors
    ///
    /// Returns an error if sending or reading fails.
    pub async fn command(&mut self, command: &str) -> Result<Response> {
        self.send_command(command).await?;
        self.read_response().await
    }

    /// Sends EHLO with the specified domain.
    ///
    /// # Errors
    ///
    /// Returns an error if the command fails.
    pub async fn ehlo(&mut self, domain: &str) -> Result<Response> {
        self.command(&format!("EHLO {domain}")).await
    }

    /// Sends HELO with the specified domain.
    ///
    /// # Errors
    ///
    /// Returns an error if the command fails.
    pub async fn helo(&mut self, domain: &str) -> Result<Response> {
        self.command(&format!("HELO {domain}")).await
    }

    /// Sends MAIL FROM command.
    ///
    /// # Errors
    ///
    /// Returns an error if the command fails.
    pub async fn mail_from(&mut self, from: &str, size: Option<usize>) -> Result<Response> {
        let cmd = if let Some(sz) = size {
            format!("MAIL FROM:<{from}> SIZE={sz}")
        } else {
            format!("MAIL FROM:<{from}>")
        };
        self.command(&cmd).await
    }

    /// Sends MAIL FROM command with generic ESMTP parameters (RFC 5321 Section 3.3).
    ///
    /// # Errors
    ///
    /// Returns an error if the command fails.
    pub async fn mail_from_with_params(
        &mut self,
        from: &str,
        params: &crate::command::MailParameters,
    ) -> Result<Response> {
        let cmd = if params.is_empty() {
            format!("MAIL FROM:<{from}>")
        } else {
            format!("MAIL FROM:<{from}> {params}")
        };
        self.command(&cmd).await
    }

    /// Sends RCPT TO command.
    ///
    /// # Errors
    ///
    /// Returns an error if the command fails.
    pub async fn rcpt_to(&mut self, to: &str) -> Result<Response> {
        self.command(&format!("RCPT TO:<{to}>")).await
    }

    /// Sends DATA command.
    ///
    /// # Errors
    ///
    /// Returns an error if the command fails.
    pub async fn data(&mut self) -> Result<Response> {
        self.command("DATA").await
    }

    /// Sends the message data followed by a dot on its own line.
    ///
    /// # Errors
    ///
    /// Returns an error if sending fails.
    pub async fn send_data(&mut self, data: &str) -> Result<Response> {
        let connection = self
            .connection
            .as_mut()
            .ok_or(ClientError::ConnectionClosed)?;

        // Send the data
        connection.send(data.as_bytes()).await?;

        // Ensure data ends with CRLF (handle both \n and \r\n cases)
        if data.ends_with("\r\n") {
            // Already has proper CRLF, do nothing
        } else if data.ends_with('\n') {
            // Has \n but not \r\n, send just \r
            connection.send(b"\r").await?;
        } else {
            // No line ending, send full CRLF
            connection.send(b"\r\n").await?;
        }

        // Send end-of-data marker
        connection.send(b".\r\n").await?;

        // Read the response
        self.read_response().await
    }

    /// Sends QUIT command.
    ///
    /// # Errors
    ///
    /// Returns an error if the command fails.
    pub async fn quit(&mut self) -> Result<Response> {
        self.command("QUIT").await
    }

    /// Sends STARTTLS command and upgrades the connection to TLS.
    ///
    /// # Errors
    ///
    /// Returns an error if STARTTLS fails or TLS upgrade fails.
    pub async fn starttls(&mut self) -> Result<Response> {
        let response = self.command("STARTTLS").await?;

        if response.is_success() {
            // Upgrade the connection to TLS
            let domain = self.server_domain.clone();
            let accept_invalid = self.accept_invalid_certs;

            // Take ownership of the connection and upgrade it
            if let Some(old_connection) = self.connection.take() {
                self.connection = Some(old_connection.upgrade_to_tls(&domain, accept_invalid).await?);
            } else {
                return Err(ClientError::ConnectionClosed);
            }
        }

        Ok(response)
    }

    /// Sends RSET command to reset the transaction.
    ///
    /// # Errors
    ///
    /// Returns an error if the command fails.
    pub async fn rset(&mut self) -> Result<Response> {
        self.command("RSET").await
    }

    /// Returns all responses received so far.
    #[must_use]
    pub fn responses(&self) -> &[Response] {
        &self.responses
    }

    /// Returns the last response received, if any.
    #[must_use]
    pub fn last_response(&self) -> Option<&Response> {
        self.responses.last()
    }

    /// Reads a complete SMTP response from the server.
    ///
    /// # Errors
    ///
    /// Returns an error if reading fails or the response is malformed.
    async fn read_response(&mut self) -> Result<Response> {
        loop {
            // Try to parse a complete response from the buffer
            if let Some((response, consumed)) = Response::parse_response(&self.buffer[..self.buffer_pos])? {
                // Remove consumed bytes from buffer
                self.buffer.copy_within(consumed..self.buffer_pos, 0);
                self.buffer_pos -= consumed;

                // Store the response
                self.responses.push(response.clone());

                return Ok(response);
            }

            // Need more data - read from connection
            if self.buffer_pos >= self.buffer.len() {
                // Buffer is full but no complete response - expand buffer
                let new_size = self.buffer.len() * 2;
                if new_size > MAX_BUFFER_SIZE {
                    return Err(ClientError::ParseError(format!(
                        "Response too large (exceeds {} bytes)",
                        MAX_BUFFER_SIZE
                    )));
                }
                self.buffer.resize(new_size, 0);
            }

            let connection = self
                .connection
                .as_mut()
                .ok_or(ClientError::ConnectionClosed)?;
            let n = connection.read(&mut self.buffer[self.buffer_pos..]).await?;
            self.buffer_pos += n;
        }
    }
}
