use std::{fmt::Display, fs::File, io::BufReader, sync::Arc};

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio_rustls::{
    rustls::{server::AllowAnyAnonymousOrAuthenticatedClient, RootCertStore, ServerConfig},
    server::TlsStream,
    TlsAcceptor,
};

use super::session::TlsContext;

pub enum Connection<Stream: AsyncRead + AsyncWrite + Unpin + Send + Sync> {
    Plain { stream: Stream },
    Tls { stream: Box<TlsStream<Stream>> },
}

impl<Stream: AsyncRead + AsyncWrite + Unpin + Send + Sync> Connection<Stream> {
    pub(crate) async fn send<S: Display + Send + Sync>(
        &mut self,
        response: &S,
    ) -> std::io::Result<usize> {
        match self {
            Self::Plain { stream } => stream.write(format!("{response}\r\n").as_bytes()).await,
            Self::Tls { stream } => stream.write(format!("{response}\r\n").as_bytes()).await,
        }
    }

    pub(crate) async fn upgrade(self, tls_context: &TlsContext) -> std::io::Result<Self> {
        if !tls_context.is_available() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                "No tls certificate or key provided",
            ));
        }

        let certfile = File::open(&tls_context.certificate)?;
        let mut reader = BufReader::new(certfile);
        let certs = rustls_pemfile::certs(&mut reader)
            .unwrap()
            .iter()
            .map(|v| tokio_rustls::rustls::Certificate(v.clone()))
            .collect::<Vec<_>>();

        let keyfile = File::open(&tls_context.key)?;
        let mut reader = BufReader::new(keyfile);

        let key = match rustls_pemfile::read_one(&mut reader)? {
            Some(
                rustls_pemfile::Item::RSAKey(key)
                | rustls_pemfile::Item::PKCS8Key(key)
                | rustls_pemfile::Item::ECKey(key),
            ) => tokio_rustls::rustls::PrivateKey(key),
            _ => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    "Unable to determine key file",
                ))
            }
        };

        let config = ServerConfig::builder()
            .with_safe_default_cipher_suites()
            .with_safe_default_kx_groups()
            .with_safe_default_protocol_versions()
            .expect("Invalid TLS Configuration")
            .with_client_cert_verifier(Arc::new(AllowAnyAnonymousOrAuthenticatedClient::new({
                let mut cert_store = RootCertStore::empty();
                cert_store.add(certs.first().unwrap()).unwrap();
                cert_store
            })))
            .with_single_cert_with_ocsp_and_sct(certs, key, Vec::new(), Vec::new())
            .expect("Invalid Cert Configuration");

        let acceptor = TlsAcceptor::from(Arc::new(config));

        Ok(Self::Tls {
            stream: match self {
                Self::Plain { stream } => Box::new(acceptor.accept(stream).await?),
                Self::Tls { stream } => Box::new(acceptor.accept(stream.into_inner().0).await?),
            },
        })
    }

    pub(crate) async fn receive(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::Plain { stream } => stream.read(buf).await,
            Self::Tls { stream } => stream.read(buf).await,
        }
    }
}