use std::{fs::File, io::BufReader, sync::Arc};

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio_rustls::{
    rustls::{
        pki_types::{CertificateDer, PrivateKeyDer},
        ProtocolVersion, ServerConfig, ServerConnection, SupportedCipherSuite,
    },
    server::TlsStream,
    TlsAcceptor,
};

use super::session::TlsContext;

#[repr(C)]
#[derive(Debug)]
pub struct TlsInfo {
    version: ProtocolVersion,
    ciphers: SupportedCipherSuite,
}

impl TlsInfo {
    fn of(conn: &ServerConnection) -> Self {
        Self {
            version: conn.protocol_version().unwrap(),
            ciphers: conn.negotiated_cipher_suite().unwrap(),
        }
    }

    pub fn proto(&self) -> String {
        self.version
            .as_str()
            .map(str::to_string)
            .unwrap_or_default()
    }

    pub fn cipher(&self) -> String {
        self.ciphers
            .suite()
            .as_str()
            .map(str::to_string)
            .unwrap_or_default()
    }
}

pub enum Connection<Stream: AsyncRead + AsyncWrite + Unpin + Send + Sync> {
    Plain { stream: Stream },
    Tls { stream: Box<TlsStream<Stream>> },
}

impl<Stream: AsyncRead + AsyncWrite + Unpin + Send + Sync> Connection<Stream> {
    pub(crate) async fn send<S: core::fmt::Display + Send + Sync>(
        &mut self,
        response: &S,
    ) -> anyhow::Result<usize> {
        Ok(match self {
            Self::Plain { stream } => stream.write(format!("{response}\r\n").as_bytes()).await?,
            Self::Tls { stream, .. } => stream.write(format!("{response}\r\n").as_bytes()).await?,
        })
    }

    fn load_certs<P: AsRef<std::path::Path>>(
        path: &P,
    ) -> std::io::Result<Vec<CertificateDer<'static>>> {
        rustls_pemfile::certs(&mut BufReader::new(File::open(path)?)).collect()
    }

    fn load_keys<P: AsRef<std::path::Path>>(path: &P) -> anyhow::Result<PrivateKeyDer<'static>> {
        let mut reader = BufReader::new(File::open(path)?);

        match rustls_pemfile::read_one(&mut reader)?.map(Into::into) {
            Some(rustls_pemfile::Item::Pkcs1Key(key)) => Ok(PrivateKeyDer::Pkcs1(key)),
            Some(rustls_pemfile::Item::Pkcs8Key(key)) => Ok(PrivateKeyDer::Pkcs8(key)),
            Some(rustls_pemfile::Item::Sec1Key(key)) => Ok(PrivateKeyDer::Sec1(key)),
            _ => Err(anyhow::Error::new(std::io::Error::new(
                std::io::ErrorKind::Other,
                "Unable to determine key file",
            ))),
        }
    }

    pub(crate) async fn upgrade(self, tls_context: &TlsContext) -> anyhow::Result<(Self, TlsInfo)> {
        tracing::debug!("Upgrading connection ...");
        if !tls_context.is_available() {
            return Err(anyhow::Error::new(std::io::Error::new(
                std::io::ErrorKind::Other,
                "No tls certificate or key provided",
            )));
        }

        let certs = Self::load_certs(&tls_context.certificate)?;
        let keys = Self::load_keys(&tls_context.key)?;

        let config = ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(certs, keys)?;

        let acceptor = TlsAcceptor::from(Arc::new(config));

        Ok(match self {
            Self::Plain { stream } => {
                let stream = acceptor.accept(stream).await?;
                let info = TlsInfo::of(stream.get_ref().1);

                (
                    Self::Tls {
                        stream: Box::new(stream),
                    },
                    info,
                )
            }
            Self::Tls { stream, .. } => {
                let (stream, connection) = acceptor.accept(stream).await?.into_inner();

                (Self::Tls { stream }, TlsInfo::of(&connection))
            }
        })
    }

    pub(crate) async fn receive(&mut self, buf: &mut [u8]) -> anyhow::Result<usize> {
        Ok(match self {
            Self::Plain { stream } => stream.read(buf).await?,
            Self::Tls { stream, .. } => stream.read(buf).await?,
        })
    }
}
