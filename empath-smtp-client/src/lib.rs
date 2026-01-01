//! Outbound SMTP client interface and transaction helpers for delivery.
//!
//! This crate defines the minimal outbound SMTP surface used by `empath-delivery`,
//! plus the shared transaction logic for outbound deliveries.

mod client;
mod error;
mod timeouts;
mod tls;
mod transaction;

use async_trait::async_trait;
pub use client::{ClientError, ConnectConfig, Response, ResponseLine, SmtpClient};
pub use error::{DeliveryError, PermanentError, SystemError, TemporaryError};
pub use timeouts::SmtpTimeouts;
pub use tls::{CertValidation, TlsMode};
use tokio::io::AsyncRead;
pub use transaction::{SmtpTlsConfig, SmtpTransaction, TlsFailurePolicy};

/// Outbound SMTP client interface used by delivery.
#[async_trait]
pub trait OutboundSmtpClient: Send {
    async fn read_greeting(&mut self) -> Result<Response, ClientError>;
    async fn ehlo(&mut self, domain: &str) -> Result<Response, ClientError>;
    async fn starttls(&mut self) -> Result<Response, ClientError>;
    async fn mail_from(&mut self, from: &str, size: Option<usize>)
    -> Result<Response, ClientError>;
    async fn rcpt_to(&mut self, to: &str) -> Result<Response, ClientError>;
    async fn data(&mut self) -> Result<Response, ClientError>;
    async fn send_data_bytes(&mut self, data: &[u8]) -> Result<Response, ClientError>;
    async fn send_data_stream(
        &mut self,
        reader: Box<dyn AsyncRead + Unpin + Send>,
    ) -> Result<Response, ClientError>;
    async fn rset(&mut self) -> Result<Response, ClientError>;
    async fn quit(&mut self) -> Result<Response, ClientError>;
    fn is_tls(&self) -> bool;
    fn set_accept_invalid_certs(&mut self, accept: bool);
}

#[async_trait]
impl OutboundSmtpClient for SmtpClient {
    async fn read_greeting(&mut self) -> Result<Response, ClientError> {
        SmtpClient::read_greeting(self).await
    }

    async fn ehlo(&mut self, domain: &str) -> Result<Response, ClientError> {
        SmtpClient::ehlo(self, domain).await
    }

    async fn starttls(&mut self) -> Result<Response, ClientError> {
        SmtpClient::starttls(self).await
    }

    async fn mail_from(
        &mut self,
        from: &str,
        size: Option<usize>,
    ) -> Result<Response, ClientError> {
        SmtpClient::mail_from(self, from, size).await
    }

    async fn rcpt_to(&mut self, to: &str) -> Result<Response, ClientError> {
        SmtpClient::rcpt_to(self, to).await
    }

    async fn data(&mut self) -> Result<Response, ClientError> {
        SmtpClient::data(self).await
    }

    async fn send_data_bytes(&mut self, data: &[u8]) -> Result<Response, ClientError> {
        SmtpClient::send_data_bytes(self, data).await
    }

    async fn send_data_stream(
        &mut self,
        reader: Box<dyn AsyncRead + Unpin + Send>,
    ) -> Result<Response, ClientError> {
        SmtpClient::send_data_stream(self, reader).await
    }

    async fn rset(&mut self) -> Result<Response, ClientError> {
        SmtpClient::rset(self).await
    }

    async fn quit(&mut self) -> Result<Response, ClientError> {
        SmtpClient::quit(self).await
    }

    fn is_tls(&self) -> bool {
        SmtpClient::is_tls(self)
    }

    fn set_accept_invalid_certs(&mut self, accept: bool) {
        SmtpClient::set_accept_invalid_certs(self, accept);
    }
}

/// Factory interface for creating outbound SMTP clients.
#[async_trait]
pub trait OutboundSmtpConnector: Send + Sync {
    type Client: OutboundSmtpClient;

    async fn connect(
        &self,
        server_address: &str,
        server_domain: String,
    ) -> Result<Self::Client, ClientError>;

    async fn connect_with_config(
        &self,
        server_address: &str,
        server_domain: String,
        config: &ConnectConfig,
    ) -> Result<Self::Client, ClientError>;
}
