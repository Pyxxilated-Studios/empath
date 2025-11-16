use std::{fmt::Write, fs::File, io::BufReader, sync::Arc};

use empath_common::tracing;
use empath_tracing::traced;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio_rustls::{
    TlsAcceptor,
    rustls::{
        ProtocolVersion, ServerConfig, ServerConnection, SupportedCipherSuite,
        pki_types::{CertificateDer, PrivateKeyDer},
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
        self.version.as_str().map_or_default(str::to_string)
    }

    pub fn cipher(&self) -> String {
        self.ciphers.suite().as_str().map_or_default(str::to_string)
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
    fn load_certs<P: AsRef<std::path::Path>>(
        path: &P,
    ) -> std::io::Result<Vec<CertificateDer<'static>>> {
        rustls_pemfile::certs(&mut BufReader::new(File::open(path)?)).collect()
    }

    #[traced(instrument(level = tracing::Level::TRACE, skip_all), timing)]
    fn load_keys<P: AsRef<std::path::Path>>(path: &P) -> TlsResult<PrivateKeyDer<'static>> {
        let path_str = path.as_ref().display().to_string();
        let mut reader = BufReader::new(File::open(path).map_err(|e| TlsError::KeyLoad {
            path: path_str.clone(),
            reason: e.to_string(),
        })?);

        match rustls_pemfile::read_one(&mut reader).map_err(|e| TlsError::KeyLoad {
            path: path_str.clone(),
            reason: e.to_string(),
        })? {
            Some(rustls_pemfile::Item::Pkcs1Key(key)) => Ok(PrivateKeyDer::Pkcs1(key)),
            Some(rustls_pemfile::Item::Pkcs8Key(key)) => Ok(PrivateKeyDer::Pkcs8(key)),
            Some(rustls_pemfile::Item::Sec1Key(key)) => Ok(PrivateKeyDer::Sec1(key)),
            _ => Err(TlsError::KeyLoad {
                path: path_str,
                reason: "Unable to determine key file format (expected PKCS1, PKCS8, or SEC1)"
                    .to_string(),
            }),
        }
    }

    #[traced(instrument(level = tracing::Level::TRACE, skip_all), timing)]
    pub(crate) async fn upgrade(self, tls_context: &TlsContext) -> TlsResult<(Self, TlsInfo)> {
        tracing::debug!("Upgrading connection ...");

        let certs =
            Self::load_certs(&tls_context.certificate).map_err(|e| TlsError::CertificateLoad {
                path: tls_context.certificate.display().to_string(),
                source: e,
            })?;
        let keys = Self::load_keys(&tls_context.key)?;

        let config = ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(certs, keys)?;

        let acceptor = TlsAcceptor::from(Arc::new(config));

        Ok(match self {
            Self::Plain {
                stream,
                read_buf,
                read_pos,
                read_len,
            } => {
                let stream = acceptor.accept(stream).await?;
                let info = TlsInfo::of(stream.get_ref().1)?;

                (
                    Self::Tls {
                        stream: Box::new(stream),
                        read_buf,
                        read_pos,
                        read_len,
                    },
                    info,
                )
            }
            Self::Tls {
                stream,
                read_buf,
                read_pos,
                read_len,
            } => {
                let (stream, connection) = acceptor.accept(stream).await?.into_inner();

                (
                    Self::Tls {
                        stream,
                        read_buf,
                        read_pos,
                        read_len,
                    },
                    TlsInfo::of(&connection)?,
                )
            }
        })
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
