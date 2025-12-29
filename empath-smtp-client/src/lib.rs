//! Outbound SMTP client interface for delivery.
//!
//! This crate defines the minimal outbound SMTP surface used by `empath-delivery`,
//! decoupling delivery from the inbound SMTP server implementation.

use std::net::IpAddr;

use async_trait::async_trait;
use thiserror::Error;
use tokio::io::AsyncRead;

/// SMTP client response payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub code: u16,
    pub message: String,
}

impl Response {
    #[must_use]
    pub fn message(&self) -> &str {
        self.message.as_str()
    }

    #[must_use]
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.code)
    }
}

/// Connection configuration for outbound SMTP clients.
#[derive(Debug, Clone, Default)]
pub struct ConnectConfig {
    /// Optional source IP to bind before connecting.
    pub local_ip: Option<IpAddr>,
    /// Optional source interface name to bind to (Linux only).
    pub local_interface: Option<String>,
}

/// Errors returned by outbound SMTP clients.
#[derive(Debug, Error)]
pub enum ClientError {
    #[error("connection closed")]
    ConnectionClosed,
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Message(String),
}

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
