//! SMTP client implementation with support for TLS and STARTTLS.

use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, LazyLock};

use empath_common::message::END_OF_BODY;
use empath_common::{outgoing, tracing};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpSocket, TcpStream};
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::rustls::{ClientConfig, ProtocolVersion, RootCertStore};

use super::error::{ClientError, Result};
use super::response::Response;

/// Initial size of the read buffer for SMTP responses.
const BUFFER_SIZE: usize = 8192;

/// Maximum size of the read buffer to prevent unbounded growth (1MB).
const MAX_BUFFER_SIZE: usize = 1024 * 1024;
const STREAM_BUFFER_SIZE: usize = 16 * 1024;
const CRLF: &[u8] = b"\r\n";

/// An SMTP client connection that can be either plain TCP or TLS-wrapped.
enum ClientConnection {
    Plain(TcpStream),
    Tls(Box<TlsStream<TcpStream>>),
}

impl ClientConnection {
    /// Sends data over the connection.
    async fn send(&mut self, data: &[u8]) -> Result<()> {
        outgoing!("{:?}", std::str::from_utf8(data));

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
    async fn upgrade_to_tls(self, domain: &str, accept_invalid_certs: bool) -> Result<Self> {
        match self {
            Self::Plain(stream) => {
                // SECURITY WARNING: Allow invalid certificates if explicitly configured
                // This should only be used for testing or development with self-signed certificates
                if accept_invalid_certs {
                    tracing::warn!(
                        domain = %domain,
                        "SECURITY WARNING: TLS certificate validation is disabled for this connection. \
                         This should only be used for testing or development environments."
                    );
                }

                let config = tls_config(accept_invalid_certs)?;
                let connector = TlsConnector::from(Arc::clone(&config));
                let server_name = ServerName::try_from(domain.to_string())
                    .map_err(|e| ClientError::TlsError(format!("Invalid domain: {e}")))?;

                let tls_stream = connector
                    .connect(server_name, stream)
                    .await
                    .map_err(|e| ClientError::TlsError(e.to_string()))?;

                Ok(Self::Tls(Box::new(tls_stream)))
            }
            Self::Tls(_) => Err(ClientError::TlsError(
                "Connection is already TLS".to_string(),
            )),
        }
    }
}

/// A certificate verifier that accepts all certificates without validation.
///
/// # Security Warning
///
/// This verifier **completely disables TLS certificate validation**, accepting any certificate
/// including expired, self-signed, or certificates with mismatched hostnames. This creates a
/// significant security risk as it allows Man-in-the-Middle (`MitM`) attacks.
///
/// # When to Use
///
/// Only use this in scenarios where you explicitly accept the security risk:
/// - **Development/Testing**: Local testing with self-signed certificates
/// - **Internal Networks**: Isolated environments with custom Certificate Authorities
/// - **Staging**: Non-production staging environments
///
/// # Never Use For
///
/// - ❌ Production email delivery
/// - ❌ Connections to public email providers (Gmail, Outlook, etc.)
/// - ❌ Any environment where security matters
///
/// # Configuration
///
/// This verifier is controlled via the two-tier `accept_invalid_certs` configuration:
/// - Global setting in `delivery.accept_invalid_certs` (default: `false`)
/// - Per-domain override in `delivery.domains.<domain>.accept_invalid_certs`
///
/// When enabled, a security warning is logged for each connection.
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

/// Connection configuration for outbound SMTP clients.
#[derive(Debug, Clone, Default)]
pub struct ConnectConfig {
    /// Optional source IP to bind before connecting.
    pub local_ip: Option<IpAddr>,
    /// Optional source interface name to bind to (Linux only).
    pub local_interface: Option<String>,
}

impl ConnectConfig {
    const fn is_empty(&self) -> bool {
        self.local_ip.is_none() && self.local_interface.is_none()
    }
}

// Cache TLS client configs so we avoid rebuilding root stores and configs per connection.
// We keep a strict config and an "accept invalid certs" config for test-only scenarios.
static STRICT_TLS_CONFIG: LazyLock<std::result::Result<Arc<ClientConfig>, ClientError>> =
    LazyLock::new(|| build_tls_config(false));
static INSECURE_TLS_CONFIG: LazyLock<std::result::Result<Arc<ClientConfig>, ClientError>> =
    LazyLock::new(|| build_tls_config(true));

/// Build a rustls `ClientConfig`, optionally allowing invalid certificates.
fn build_tls_config(
    accept_invalid_certs: bool,
) -> std::result::Result<Arc<ClientConfig>, ClientError> {
    let mut root_store = RootCertStore::empty();

    // Add system certificates
    let certs = rustls_native_certs::load_native_certs();
    for cert in certs.certs {
        root_store
            .add(cert)
            .map_err(|e| ClientError::TlsError(format!("Failed to add certificate: {e}")))?;
    }
    if !certs.errors.is_empty() {
        tracing::warn!(?certs.errors, "Some certificates could not be loaded");
    }

    let mut config = ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();

    if accept_invalid_certs {
        config
            .dangerous()
            .set_certificate_verifier(Arc::new(NoVerifier));
    }

    Ok(Arc::new(config))
}

/// Get a cached TLS config (strict or insecure) for client connections.
fn tls_config(accept_invalid_certs: bool) -> Result<Arc<ClientConfig>> {
    let cached = if accept_invalid_certs {
        &*INSECURE_TLS_CONFIG
    } else {
        &*STRICT_TLS_CONFIG
    };

    cached
        .as_ref()
        .map(Arc::clone)
        .map_err(|e| ClientError::TlsError(e.to_string()))
}

impl SmtpClient {
    /// Create a new SMTP client with a connection to the server.
    ///
    /// # Errors
    ///
    /// Returns `ClientError::Io` if the connection fails.
    pub async fn connect(server_address: &str, server_domain: String) -> Result<Self> {
        let addr = server_address.parse::<SocketAddr>().map_err(|_| {
            ClientError::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid address",
            ))
        })?;
        let stream = TcpStream::connect(addr).await.map_err(ClientError::Io)?;

        Ok(Self {
            connection: Some(ClientConnection::Plain(stream)),
            buffer: vec![0; BUFFER_SIZE],
            buffer_pos: 0,
            responses: Vec::new(),
            server_domain,
            accept_invalid_certs: false,
        })
    }

    /// Create a new SMTP client with a configured socket.
    ///
    /// # Errors
    ///
    /// Returns `ClientError` if the connection fails.
    pub async fn connect_with_config(
        server_address: &str,
        server_domain: String,
        config: &ConnectConfig,
    ) -> Result<Self> {
        if config.is_empty() {
            return Self::connect(server_address, server_domain).await;
        }

        let addr = server_address.parse::<SocketAddr>().map_err(|_| {
            ClientError::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid address",
            ))
        })?;

        let socket = create_socket(addr, config)?;
        let stream = connect_socket(socket, addr).await?;

        Ok(Self {
            connection: Some(ClientConnection::Plain(stream)),
            buffer: vec![0; BUFFER_SIZE],
            buffer_pos: 0,
            responses: Vec::new(),
            server_domain,
            accept_invalid_certs: false,
        })
    }

    /// Enable or disable acceptance of invalid TLS certificates.
    #[must_use]
    pub fn accept_invalid_certs(mut self, accept: bool) -> Self {
        self.accept_invalid_certs = accept;
        self
    }

    /// Enable or disable acceptance of invalid TLS certificates in-place.
    pub fn set_accept_invalid_certs(&mut self, accept: bool) {
        self.accept_invalid_certs = accept;
    }

    /// Returns true if the connection is currently TLS.
    #[must_use]
    pub fn is_tls(&self) -> bool {
        matches!(self.connection, Some(ClientConnection::Tls(_)))
    }

    /// Returns the negotiated TLS protocol version, if available.
    #[must_use]
    pub fn tls_protocol_version(&self) -> Option<ProtocolVersion> {
        match &self.connection {
            Some(ClientConnection::Tls(stream)) => stream.get_ref().1.protocol_version(),
            _ => None,
        }
    }

    /// Read the initial server greeting.
    ///
    /// # Errors
    ///
    /// Returns `ClientError` if the greeting cannot be read or is invalid.
    pub async fn read_greeting(&mut self) -> Result<Response> {
        let response = self.read_response().await?;
        self.responses.push(response.clone());
        Ok(response)
    }

    /// Send a raw SMTP command.
    pub async fn command(&mut self, command: &str) -> Result<Response> {
        let cmd = format!("{command}\r\n");
        self.connection
            .as_mut()
            .ok_or(ClientError::ConnectionClosed)?
            .send(cmd.as_bytes())
            .await?;

        let response = self.read_response().await?;
        self.responses.push(response.clone());
        Ok(response)
    }

    /// Send EHLO command.
    pub async fn ehlo(&mut self, domain: &str) -> Result<Response> {
        self.command(&format!("EHLO {domain}")).await
    }

    /// Send HELO command.
    pub async fn helo(&mut self, domain: &str) -> Result<Response> {
        self.command(&format!("HELO {domain}")).await
    }

    /// Send MAIL FROM command.
    pub async fn mail_from(&mut self, from: &str, size: Option<usize>) -> Result<Response> {
        let command = if let Some(size) = size {
            format!("MAIL FROM:<{from}> SIZE={size}")
        } else {
            format!("MAIL FROM:<{from}>")
        };
        self.command(&command).await
    }

    /// Send MAIL FROM command with optional parameters.
    pub async fn mail_from_with_params(
        &mut self,
        from: &str,
        params: Option<&str>,
    ) -> Result<Response> {
        let command = if let Some(params) = params {
            format!("MAIL FROM:<{from}> {params}")
        } else {
            format!("MAIL FROM:<{from}>")
        };
        self.command(&command).await
    }

    /// Send RCPT TO command.
    pub async fn rcpt_to(&mut self, to: &str) -> Result<Response> {
        self.command(&format!("RCPT TO:<{to}>")).await
    }

    /// Send DATA command.
    pub async fn data(&mut self) -> Result<Response> {
        self.command("DATA").await
    }

    /// Send email data (string).
    pub async fn send_data(&mut self, data: &str) -> Result<Response> {
        self.send_data_bytes(data.as_bytes()).await
    }

    /// Send email data (bytes), handling dot-stuffing and termination.
    pub async fn send_data_bytes(&mut self, data: &[u8]) -> Result<Response> {
        let mut buffer = Vec::with_capacity(data.len() + END_OF_BODY.len());
        buffer.extend_from_slice(data);
        if !buffer.ends_with(CRLF) {
            buffer.extend_from_slice(CRLF);
        }
        buffer.extend_from_slice(END_OF_BODY);

        let connection = self
            .connection
            .as_mut()
            .ok_or(ClientError::ConnectionClosed)?;
        connection.send(&buffer).await?;

        let response = self.read_response().await?;
        self.responses.push(response.clone());
        Ok(response)
    }

    /// Send email data from a stream.
    pub async fn send_data_stream<R>(&mut self, mut reader: R) -> Result<Response>
    where
        R: AsyncRead + Unpin,
    {
        let mut connection = self
            .connection
            .take()
            .ok_or(ClientError::ConnectionClosed)?;

        let mut buffer = vec![0; STREAM_BUFFER_SIZE];
        loop {
            let n = reader.read(&mut buffer).await?;
            if n == 0 {
                break;
            }
            connection.send(&buffer[..n]).await?;
        }

        connection.send(END_OF_BODY).await?;
        self.connection = Some(connection);

        let response = self.read_response().await?;
        self.responses.push(response.clone());
        Ok(response)
    }

    /// Send QUIT command.
    pub async fn quit(&mut self) -> Result<Response> {
        let response = self.command("QUIT").await?;
        self.connection = None;
        Ok(response)
    }

    /// Send NOOP command.
    pub async fn noop(&mut self) -> Result<Response> {
        self.command("NOOP").await
    }

    /// Send STARTTLS command and upgrade connection to TLS.
    pub async fn starttls(&mut self) -> Result<Response> {
        let response = self.command("STARTTLS").await?;

        if response.is_success() {
            let connection = self
                .connection
                .take()
                .ok_or(ClientError::ConnectionClosed)?;
            let tls_connection = connection
                .upgrade_to_tls(&self.server_domain, self.accept_invalid_certs)
                .await?;
            self.connection = Some(tls_connection);
        }

        Ok(response)
    }

    /// Send RSET command.
    pub async fn rset(&mut self) -> Result<Response> {
        self.command("RSET").await
    }

    /// Return all responses received so far.
    #[must_use]
    pub fn responses(&self) -> &[Response] {
        &self.responses
    }

    /// Return the last response received.
    #[must_use]
    pub fn last_response(&self) -> Option<&Response> {
        self.responses.last()
    }

    async fn read_response(&mut self) -> Result<Response> {
        loop {
            if let Some((response, bytes_consumed)) =
                Response::parse_response(&self.buffer[..self.buffer_pos])?
            {
                // Remove consumed bytes from buffer
                self.buffer.copy_within(bytes_consumed..self.buffer_pos, 0);
                self.buffer_pos -= bytes_consumed;
                return Ok(response);
            }

            if self.buffer_pos >= self.buffer.len() {
                if self.buffer.len() >= MAX_BUFFER_SIZE {
                    return Err(ClientError::ParseError(format!(
                        "Response too large (exceeds {MAX_BUFFER_SIZE} bytes)"
                    )));
                }
                self.buffer.resize(self.buffer.len() * 2, 0);
            }

            let n = self
                .connection
                .as_mut()
                .ok_or(ClientError::ConnectionClosed)?
                .read(&mut self.buffer[self.buffer_pos..])
                .await?;
            self.buffer_pos += n;
        }
    }
}

fn create_socket(addr: SocketAddr, config: &ConnectConfig) -> Result<TcpSocket> {
    let socket = if addr.is_ipv4() {
        TcpSocket::new_v4().map_err(ClientError::Io)?
    } else {
        TcpSocket::new_v6().map_err(ClientError::Io)?
    };

    if let Some(ip) = config.local_ip {
        socket
            .bind(SocketAddr::new(ip, 0))
            .map_err(ClientError::Io)?;
    }

    if let Some(interface) = &config.local_interface {
        set_socket_interface(&socket, interface)?;
    }

    Ok(socket)
}

async fn connect_socket(socket: TcpSocket, addr: SocketAddr) -> Result<TcpStream> {
    socket.connect(addr).await.map_err(ClientError::Io)
}

#[cfg(target_os = "linux")]
fn set_socket_interface(socket: &TcpSocket, interface: &str) -> Result<()> {
    use std::ffi::CString;
    use std::os::fd::AsRawFd;

    let cstr = CString::new(interface).map_err(|_| {
        ClientError::Io(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Invalid interface name",
        ))
    })?;

    let result = unsafe {
        libc::setsockopt(
            socket.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_BINDTODEVICE,
            cstr.as_ptr().cast(),
            u32::try_from(cstr.as_bytes_with_nul().len()).unwrap_or(0),
        )
    };

    if result != 0 {
        return Err(ClientError::Io(io::Error::new(
            io::ErrorKind::Other,
            "Failed to set socket interface",
        )));
    }

    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn set_socket_interface(_socket: &TcpSocket, _interface: &str) -> Result<()> {
    Err(ClientError::Io(io::Error::other(
        "Binding to interface is only supported on Linux",
    )))
}
