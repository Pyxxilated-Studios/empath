use std::fmt::{Debug, Write};
use std::time::Instant;

use empath_common::tracing;
use empath_metrics::metrics;
use empath_tracing::traced;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio_rustls::TlsAcceptor;
use tokio_rustls::rustls::{ProtocolVersion, ServerConnection, SupportedCipherSuite};
use tokio_rustls::server::TlsStream;

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
        TlsError::ProfileSetNotFound { .. }
        | TlsError::ProfileNotFound { .. }
        | TlsError::ProfileConfig(_) => "profile_config",
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
    pub(crate) async fn upgrade(self, tls_context: &TlsContext) -> TlsResult<(Self, TlsInfo)> {
        tracing::debug!("Upgrading connection ...");

        let config = tls_context.server_config()?;
        let acceptor = TlsAcceptor::from(config);
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

                if let Some(metrics) = metrics() {
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
                if let Some(metrics) = metrics() {
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

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt, duplex};

    use super::*;

    fn make_plain(stream: tokio::io::DuplexStream) -> Connection<tokio::io::DuplexStream> {
        Connection::Plain {
            stream,
            read_buf: Vec::new(),
            read_pos: 0,
            read_len: 0,
        }
    }

    #[tokio::test]
    async fn send_appends_crlf_and_reports_length() -> Result<(), Box<dyn std::error::Error>> {
        let (client, mut server) = duplex(64);
        let mut connection = make_plain(client);
        let response = "250 OK";

        let written = connection.send(&response).await?;

        let mut buf = vec![0u8; response.len() + 2];
        server.read_exact(&mut buf).await?;

        assert_eq!(written, response.len() + 2);
        assert_eq!(buf, b"250 OK\r\n");
        Ok(())
    }

    #[tokio::test]
    async fn receive_uses_buffered_data_before_reading_again()
    -> Result<(), Box<dyn std::error::Error>> {
        let (client, mut server) = duplex(64);
        let mut connection = make_plain(client);

        server.write_all(b"hello").await?;

        let mut buf = [0u8; 2];
        let first_read = connection.receive(&mut buf).await?;
        assert_eq!(first_read, 2);
        assert_eq!(&buf, b"he");

        let mut buf = [0u8; 4];
        let second_read = connection.receive(&mut buf).await?;
        assert_eq!(second_read, 3);
        assert_eq!(&buf[..second_read], b"llo");
        Ok(())
    }
}
