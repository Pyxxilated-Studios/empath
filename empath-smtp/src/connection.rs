use std::{
    fmt::{Debug, Write},
    sync::Arc,
    time::Instant,
};

use empath_common::tracing;
use empath_tracing::traced;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio_rustls::{
    TlsAcceptor,
    rustls::{
        ProtocolVersion, ServerConfig, ServerConnection, SupportedCipherSuite,
        pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject},
    },
    server::TlsStream,
};

use super::session::TlsContext;
use crate::error::{ConnectionResult, TlsError, TlsResult};

#[repr(C)]
#[derive(Debug)]
pub struct TlsInfo {
    version: ProtocolVersion,
    ciphers: SupportedCipherSuite,
}

impl TlsInfo {
    fn of(conn: &ServerConnection) -> TlsResult<Self> {
        Ok(Self {
            version: conn
                .protocol_version()
                .ok_or_else(|| TlsError::ProtocolInfoMissing("protocol version".to_string()))?,
            ciphers: conn
                .negotiated_cipher_suite()
                .ok_or_else(|| TlsError::ProtocolInfoMissing("cipher suite".to_string()))?,
        })
    }

    pub fn proto(&self) -> String {
        self.version
            .as_str()
            .map_or_else(String::new, str::to_string)
    }

    pub fn cipher(&self) -> String {
        self.ciphers
            .suite()
            .as_str()
            .map_or_else(String::new, str::to_string)
    }
}

const fn tls_error_reason(err: &TlsError) -> &'static str {
    match err {
        TlsError::Io(_) => "io",
        TlsError::CertificateLoad { .. } => "certificate_load",
        TlsError::KeyLoad { .. } => "key_load",
        TlsError::UpgradeFailed(_) => "handshake",
        TlsError::ProtocolInfoMissing(_) => "protocol_info_missing",
        TlsError::Rustls(_) => "rustls",
    }
}

pub enum Connection<Stream: AsyncRead + AsyncWrite + Unpin + Send + Sync> {
    Plain {
        stream: Stream,
        /// Internal read buffer to reduce syscalls (8KB)
        read_buf: Vec<u8>,
        /// Current position in read buffer
        read_pos: usize,
        /// Amount of valid data in read buffer
        read_len: usize,
    },
    Tls {
        stream: Box<TlsStream<Stream>>,
        /// Internal read buffer to reduce syscalls (8KB)
        read_buf: Vec<u8>,
        /// Current position in read buffer
        read_pos: usize,
        /// Amount of valid data in read buffer
        read_len: usize,
    },
}

impl<Stream: AsyncRead + AsyncWrite + Unpin + Send + Sync> Connection<Stream> {
    #[traced(instrument(level = tracing::Level::TRACE, skip_all), timing)]
    pub(crate) async fn send<S: core::fmt::Display + Send + Sync>(
        &mut self,
        response: &S,
    ) -> ConnectionResult<usize> {
        // Format response to stack-allocated buffer to avoid heap allocation
        let mut buffer = arrayvec::ArrayString::<512>::new();
        write!(&mut buffer, "{response}\r\n")?;

        Ok(match self {
            Self::Plain { stream, .. } => stream
                .write_all(buffer.as_bytes())
                .await
                .map(|()| buffer.len())?,
            Self::Tls { stream, .. } => stream
                .write_all(buffer.as_bytes())
                .await
                .map(|()| buffer.len())?,
        })
    }

    #[traced(instrument(level = tracing::Level::TRACE, skip_all), timing)]
    fn load_certs<P: AsRef<std::path::Path> + Debug>(
        path: &P,
    ) -> TlsResult<CertificateDer<'static>> {
        CertificateDer::from_pem_file(path).map_err(|e| TlsError::KeyLoad {
            path: format!("{path:?}"),
            reason: e.to_string(),
        })
    }

    #[traced(instrument(level = tracing::Level::TRACE, skip_all), timing)]
    fn load_keys<P: AsRef<std::path::Path> + Debug>(path: &P) -> TlsResult<PrivateKeyDer<'static>> {
        PrivateKeyDer::from_pem_file(path).map_err(|e| TlsError::KeyLoad {
            path: format!("{path:?}"),
            reason: e.to_string(),
        })
    }

    #[traced(instrument(level = tracing::Level::TRACE, skip_all), timing)]
    pub(crate) async fn upgrade(self, tls_context: &TlsContext) -> TlsResult<(Self, TlsInfo)> {
        tracing::debug!("Upgrading connection ...");

        let certs = Self::load_certs(&tls_context.certificate)?;
        let keys = Self::load_keys(&tls_context.key)?;

        let config = ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![certs], keys)?;

        let acceptor = TlsAcceptor::from(Arc::new(config));
        let start = Instant::now();

        let handshake: TlsResult<(Self, TlsInfo)> = match self {
            Self::Plain {
                stream,
                read_buf,
                read_pos,
                read_len,
            } => {
                let stream = acceptor
                    .accept(stream)
                    .await
                    .map_err(|e| TlsError::UpgradeFailed(e.to_string()))?;
                let info = TlsInfo::of(stream.get_ref().1)?;

                Ok((
                    Self::Tls {
                        stream: Box::new(stream),
                        read_buf,
                        read_pos,
                        read_len,
                    },
                    info,
                ))
            }
            Self::Tls {
                stream,
                read_buf,
                read_pos,
                read_len,
            } => {
                let (stream, connection) = acceptor
                    .accept(stream)
                    .await
                    .map_err(|e| TlsError::UpgradeFailed(e.to_string()))?
                    .into_inner();

                Ok((
                    Self::Tls {
                        stream,
                        read_buf,
                        read_pos,
                        read_len,
                    },
                    TlsInfo::of(&connection)?,
                ))
            }
        };

        let duration_secs = start.elapsed().as_secs_f64();

        match handshake {
            Ok((conn, info)) => {
                let proto = info.proto();
                let cipher = info.cipher();

                if let Some(metrics) = empath_metrics::try_metrics() {
                    metrics
                        .smtp
                        .record_tls_handshake(duration_secs, &proto, &cipher);
                }

                tracing::info!(
                    duration_seconds = duration_secs,
                    protocol = proto,
                    cipher,
                    "TLS handshake succeeded"
                );

                Ok((conn, info))
            }
            Err(err) => {
                if let Some(metrics) = empath_metrics::try_metrics() {
                    metrics
                        .smtp
                        .record_tls_handshake_failure(tls_error_reason(&err));
                }

                tracing::warn!(
                    duration_seconds = duration_secs,
                    reason = tls_error_reason(&err),
                    error = %err,
                    "TLS handshake failed"
                );

                Err(err)
            }
        }
    }

    #[traced(instrument(level = tracing::Level::TRACE, skip_all), timing)]
    pub(crate) async fn receive(&mut self, buf: &mut [u8]) -> ConnectionResult<usize> {
        const BUFFER_SIZE: usize = 8192;

        match self {
            Self::Plain {
                stream,
                read_buf,
                read_pos,
                read_len,
            } => {
                // If we have buffered data, use it first
                if *read_pos < *read_len {
                    let available = *read_len - *read_pos;
                    let to_copy = available.min(buf.len());
                    buf[..to_copy].copy_from_slice(&read_buf[*read_pos..*read_pos + to_copy]);
                    *read_pos += to_copy;
                    return Ok(to_copy);
                }

                // Buffer is empty, read more data
                if read_buf.is_empty() {
                    read_buf.resize(BUFFER_SIZE, 0);
                }

                let bytes_read = stream.read(read_buf).await?;
                *read_pos = 0;
                *read_len = bytes_read;

                // Copy from buffer to output
                let to_copy = bytes_read.min(buf.len());
                buf[..to_copy].copy_from_slice(&read_buf[..to_copy]);
                *read_pos = to_copy;
                Ok(to_copy)
            }
            Self::Tls {
                stream,
                read_buf,
                read_pos,
                read_len,
            } => {
                // If we have buffered data, use it first
                if *read_pos < *read_len {
                    let available = *read_len - *read_pos;
                    let to_copy = available.min(buf.len());
                    buf[..to_copy].copy_from_slice(&read_buf[*read_pos..*read_pos + to_copy]);
                    *read_pos += to_copy;
                    return Ok(to_copy);
                }

                // Buffer is empty, read more data
                if read_buf.is_empty() {
                    read_buf.resize(BUFFER_SIZE, 0);
                }

                let bytes_read = stream.read(read_buf).await?;
                *read_pos = 0;
                *read_len = bytes_read;

                // Copy from buffer to output
                let to_copy = bytes_read.min(buf.len());
                buf[..to_copy].copy_from_slice(&read_buf[..to_copy]);
                *read_pos = to_copy;
                Ok(to_copy)
            }
        }
    }
}
