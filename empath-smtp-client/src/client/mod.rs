//! Outbound SMTP client implementation.

mod error;
mod response;
mod smtp_client;

pub use error::ClientError;
pub use response::{Response, ResponseLine};
pub use smtp_client::{ConnectConfig, SmtpClient};
