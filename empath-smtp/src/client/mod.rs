//! SMTP client implementation for testing and integration purposes.
//!
//! This module provides a flexible SMTP client with builder pattern support
//! for creating test scenarios. It supports:
//!
//! - Plain TCP and TLS connections
//! - STARTTLS upgrade
//! - Flexible command sequencing via builder pattern
//! - Configurable quit points (like `swaks --quit-after`)
//! - Response inspection for assertions
//!
//! # Examples
//!
//! ## Basic Usage
//!
//! ```no_run
//! use empath_smtp::client::SmtpClientBuilder;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let responses = SmtpClientBuilder::new("localhost:2525", "mail.example.com")
//!     .ehlo("client.example.com")
//!     .mail_from("sender@example.com")
//!     .rcpt_to("recipient@example.com")
//!     .data_with_content("Subject: Test\r\n\r\nHello World")
//!     .execute()
//!     .await?;
//!
//! // Verify the last response
//! assert!(responses.last().unwrap().is_success());
//! # Ok(())
//! # }
//! ```
//!
//! ## Quit After Specific Command (like swaks --quit-after)
//!
//! ```no_run
//! use empath_smtp::client::{SmtpClientBuilder, QuitAfter};
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let responses = SmtpClientBuilder::new("localhost:2525", "mail.example.com")
//!     .ehlo("client.example.com")
//!     .mail_from("sender@example.com")
//!     .quit_after(QuitAfter::MailFrom)
//!     .execute()
//!     .await?;
//!
//! // Session ends after MAIL FROM response
//! # Ok(())
//! # }
//! ```
//!
//! ## STARTTLS Example
//!
//! ```no_run
//! use empath_smtp::client::SmtpClientBuilder;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let responses = SmtpClientBuilder::new("localhost:2525", "mail.example.com")
//!     .ehlo("client.example.com")
//!     .starttls()
//!     .ehlo("client.example.com")  // Re-send EHLO after STARTTLS
//!     .mail_from("sender@example.com")
//!     .rcpt_to("recipient@example.com")
//!     .data_with_content("Subject: Secure\r\n\r\nSecure message")
//!     .execute()
//!     .await?;
//! # Ok(())
//! # }
//! ```
//!
//! ## Raw Commands for Testing
//!
//! ```no_run
//! use empath_smtp::client::SmtpClientBuilder;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let responses = SmtpClientBuilder::new("localhost:2525", "mail.example.com")
//!     .ehlo("client.example.com")
//!     .raw_command("VRFY user@example.com")
//!     .raw_command("HELP")
//!     .execute()
//!     .await?;
//! # Ok(())
//! # }
//! ```

mod builder;
mod error;
mod message;
mod quit_after;
mod response;
mod smtp_client;

pub use builder::SmtpClientBuilder;
pub use error::{ClientError, Result};
pub use message::{Attachment, MessageBuilder};
pub use quit_after::QuitAfter;
pub use response::{Response, ResponseLine};
pub use smtp_client::SmtpClient;
