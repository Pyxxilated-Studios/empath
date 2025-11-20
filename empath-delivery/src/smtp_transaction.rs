//! SMTP transaction execution module
//!
//! This module handles the execution of SMTP transactions for message delivery.
//! It encapsulates all SMTP protocol operations including:
//! - TLS negotiation via STARTTLS
//! - MAIL FROM command
//! - RCPT TO commands for all recipients
//! - DATA command and message content transmission
//! - QUIT command for connection cleanup

use std::time::Duration;

use empath_common::{context::Context, tracing};
use empath_smtp::client::SmtpClient;
use empath_tracing::traced;

use crate::{
    SmtpTimeouts,
    error::{DeliveryError, PermanentError, SystemError, TemporaryError},
};

/// Outcome of TLS negotiation attempt
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TlsNegotiationOutcome {
    /// TLS was successfully negotiated via STARTTLS
    Success,
    /// TLS was not attempted (server doesn't advertise STARTTLS and not required)
    Skipped,
    /// TLS negotiation failed but was opportunistic - should retry without TLS per RFC 3207
    RetryWithoutTls,
}

/// Extracts the email address string from an Address (for SMTP commands)
///
/// Since Address now wraps Mailbox which is always a simple email address,
/// this function uses the Display impl to format the address.
///
/// Returns a formatted email address (`local_part@domain`).
fn extract_email_address(address: &empath_common::address::Address) -> String {
    address.to_string()
}

/// Represents a single SMTP transaction for delivering a message
///
/// This struct encapsulates all the logic for performing a complete SMTP
/// transaction, from connection establishment to message delivery.
pub struct SmtpTransaction<'a> {
    /// The message context containing envelope and data
    context: &'a Context,
    /// The SMTP server address (host:port)
    server_address: String,
    /// Whether TLS is required for this delivery
    require_tls: bool,
    /// Whether to accept invalid TLS certificates
    accept_invalid_certs: bool,
    /// Timeout configuration for SMTP operations
    smtp_timeouts: &'a SmtpTimeouts,
}

impl<'a> SmtpTransaction<'a> {
    /// Create a new SMTP transaction
    ///
    /// # Arguments
    /// * `context` - The message context containing envelope and data
    /// * `server_address` - The SMTP server address (host:port)
    /// * `require_tls` - Whether TLS is required for this delivery
    /// * `accept_invalid_certs` - Whether to accept invalid TLS certificates
    /// * `smtp_timeouts` - Timeout configuration for SMTP operations
    #[must_use]
    pub const fn new(
        context: &'a Context,
        server_address: String,
        require_tls: bool,
        accept_invalid_certs: bool,
        smtp_timeouts: &'a SmtpTimeouts,
    ) -> Self {
        Self {
            context,
            server_address,
            require_tls,
            accept_invalid_certs,
            smtp_timeouts,
        }
    }

    /// Connect to SMTP server and read greeting
    ///
    /// # Errors
    /// Returns an error if connection or greeting fails
    async fn connect_and_greet(&self) -> Result<SmtpClient, DeliveryError> {
        // Connect to the SMTP server
        // Automatic From<ClientError> conversion handles connection errors
        let mut client = SmtpClient::connect(&self.server_address, self.server_address.clone())
            .await
            .map_err(|e| {
                // Add context about which server we tried to connect to
                TemporaryError::ConnectionFailed(format!(
                    "Failed to connect to {}: {e}",
                    self.server_address
                ))
            })?
            .accept_invalid_certs(self.accept_invalid_certs);

        // Read greeting
        let greeting = client.read_greeting().await?;

        if !greeting.is_success() {
            return Err(TemporaryError::ServerBusy(format!(
                "Server rejected connection: {}",
                greeting.message()
            ))
            .into());
        }

        Ok(client)
    }

    /// Execute the complete SMTP transaction
    ///
    /// This method performs the full SMTP transaction:
    /// 1. Connects to the SMTP server
    /// 2. Reads the server greeting
    /// 3. Optionally upgrades to TLS via STARTTLS if required or available
    /// 4. If STARTTLS fails for opportunistic TLS, reconnects without TLS (RFC 3207)
    /// 5. Performs EHLO/HELO handshake
    /// 6. Sends MAIL FROM and RCPT TO commands
    /// 7. Sends DATA command and the actual message content
    /// 8. Sends QUIT to cleanly close the connection
    ///
    /// # Errors
    /// Returns an error if any part of the SMTP transaction fails
    #[traced(instrument(level = tracing::Level::INFO, skip(self), fields(server = %self.server_address, message_id = %self.context.id)), timing(precision = "ms"))]
    pub async fn execute(self) -> Result<(), DeliveryError> {
        // Log security warning if certificate validation is disabled
        if self.accept_invalid_certs {
            tracing::warn!(
                server = %self.server_address,
                "SECURITY WARNING: TLS certificate validation is disabled for this connection"
            );
        }

        // Connect to the SMTP server
        let mut client = self.connect_and_greet().await?;

        // Perform TLS negotiation if required or available
        let tls_outcome = self.negotiate_tls(&mut client).await?;

        // RFC 3207: If STARTTLS fails for opportunistic TLS, reconnect without attempting TLS
        if tls_outcome == TlsNegotiationOutcome::RetryWithoutTls {
            tracing::info!(
                server = %self.server_address,
                "Reconnecting without STARTTLS per RFC 3207 Section 4.1"
            );

            // Drop the old connection and reconnect
            drop(client);
            client = self.connect_and_greet().await?;

            // Perform EHLO without attempting TLS
            let helo_domain = &self.context.id;
            let ehlo_timeout = Duration::from_secs(self.smtp_timeouts.ehlo_secs);
            let ehlo_response = tokio::time::timeout(ehlo_timeout, client.ehlo(helo_domain))
                .await
                .map_err(|_| {
                    TemporaryError::Timeout(format!("EHLO timed out after {ehlo_timeout:?}"))
                })??;

            if !ehlo_response.is_success() {
                return Err(TemporaryError::SmtpTemporary(format!(
                    "Server rejected EHLO: {}",
                    ehlo_response.message()
                ))
                .into());
            }

            tracing::info!(
                server = %self.server_address,
                "Successfully reconnected without TLS, proceeding with plaintext delivery"
            );
        }

        // Send MAIL FROM
        self.send_mail_from(&mut client).await?;

        // Send RCPT TO for all recipients
        self.send_rcpt_to(&mut client).await?;

        // Send DATA command and message content
        self.send_message_data(&mut client).await?;

        // Send QUIT to cleanly close the connection
        // Note: We don't fail the delivery if QUIT fails since the message was already delivered
        let quit_timeout = Duration::from_secs(self.smtp_timeouts.quit_secs);
        if let Err(e) = tokio::time::timeout(quit_timeout, client.quit()).await {
            tracing::warn!(
                server = %self.server_address,
                timeout = ?quit_timeout,
                "QUIT command timed out after successful delivery: {e}"
            );
        }

        Ok(())
    }

    /// Negotiate TLS upgrade via STARTTLS if required or available
    ///
    /// Returns an outcome indicating whether TLS was successfully negotiated, skipped,
    /// or failed in a way that requires reconnection without TLS (per RFC 3207).
    ///
    /// # Errors
    /// Returns an error if TLS is required but fails
    async fn negotiate_tls(
        &self,
        client: &mut SmtpClient,
    ) -> Result<TlsNegotiationOutcome, DeliveryError> {
        let helo_domain = &self.context.id;

        // Send initial EHLO
        let ehlo_timeout = Duration::from_secs(self.smtp_timeouts.ehlo_secs);
        let ehlo_response = tokio::time::timeout(ehlo_timeout, client.ehlo(helo_domain))
            .await
            .map_err(|_| {
                TemporaryError::Timeout(format!("EHLO timed out after {ehlo_timeout:?}"))
            })??;

        if !ehlo_response.is_success() {
            return Err(TemporaryError::SmtpTemporary(format!(
                "Server rejected EHLO: {}",
                ehlo_response.message()
            ))
            .into());
        }

        // Check if server advertises STARTTLS
        let supports_starttls = ehlo_response
            .message()
            .lines()
            .any(|line| line.to_uppercase().contains("STARTTLS"));

        // If server doesn't support STARTTLS and it's not required, skip TLS
        if !supports_starttls && !self.require_tls {
            return Ok(TlsNegotiationOutcome::Skipped);
        }

        if self.require_tls || supports_starttls {
            let starttls_timeout = Duration::from_secs(self.smtp_timeouts.starttls_secs);

            // Attempt STARTTLS command
            let starttls_result = tokio::time::timeout(starttls_timeout, client.starttls())
                .await
                .map_err(|_| format!("STARTTLS timed out after {starttls_timeout:?}"))
                .and_then(|r| r.map_err(|e| format!("STARTTLS failed: {e}")));

            let starttls_response = match starttls_result {
                Ok(response) => response,
                Err(error_msg) => {
                    if self.require_tls {
                        // TLS required but failed - permanent error
                        return Err(PermanentError::TlsRequired(error_msg).into());
                    }
                    // Opportunistic TLS failed - should retry without TLS per RFC 3207
                    tracing::info!(
                        domain = helo_domain,
                        error = %error_msg,
                        "STARTTLS failed for opportunistic TLS, will retry without TLS per RFC 3207"
                    );
                    return Ok(TlsNegotiationOutcome::RetryWithoutTls);
                }
            };

            if !starttls_response.is_success() {
                let message = format!("Server rejected STARTTLS: {}", starttls_response.message());
                if self.require_tls {
                    // TLS required but server rejected - permanent error
                    return Err(PermanentError::TlsRequired(message).into());
                }
                // Opportunistic TLS rejected - should retry without TLS per RFC 3207
                tracing::info!(
                    domain = helo_domain,
                    response = %starttls_response.message(),
                    "Server rejected STARTTLS for opportunistic TLS, will retry without TLS per RFC 3207"
                );
                return Ok(TlsNegotiationOutcome::RetryWithoutTls);
            }

            // Re-send EHLO after STARTTLS (RFC 3207)
            let ehlo_response = tokio::time::timeout(ehlo_timeout, client.ehlo(helo_domain))
                .await
                .map_err(|_| {
                    TemporaryError::Timeout(format!(
                        "EHLO after STARTTLS timed out after {ehlo_timeout:?}"
                    ))
                })??;

            if !ehlo_response.is_success() {
                return Err(TemporaryError::SmtpTemporary(format!(
                    "Server rejected EHLO after STARTTLS: {}",
                    ehlo_response.message()
                ))
                .into());
            }

            tracing::debug!(
                domain = helo_domain,
                "TLS successfully negotiated via STARTTLS"
            );
            Ok(TlsNegotiationOutcome::Success)
        } else {
            // This branch shouldn't be reached given the earlier check, but handle it for completeness
            Ok(TlsNegotiationOutcome::Skipped)
        }
    }

    /// Send MAIL FROM command
    ///
    /// # Errors
    /// Returns an error if the MAIL FROM command fails
    async fn send_mail_from(&self, client: &mut SmtpClient) -> Result<(), DeliveryError> {
        let sender = self
            .context
            .envelope
            .sender()
            .map(extract_email_address)
            .unwrap_or_default();

        let mail_from_timeout = Duration::from_secs(self.smtp_timeouts.mail_from_secs);
        let mail_response =
            tokio::time::timeout(mail_from_timeout, client.mail_from(&sender, None))
                .await
                .map_err(|_| {
                    TemporaryError::Timeout(format!(
                        "MAIL FROM timed out after {mail_from_timeout:?}"
                    ))
                })??;

        if !mail_response.is_success() {
            let code = mail_response.code;
            let message = format!("Server rejected MAIL FROM: {}", mail_response.message());
            return if (500..600).contains(&code) {
                Err(PermanentError::MessageRejected(message).into())
            } else {
                Err(TemporaryError::SmtpTemporary(message).into())
            };
        }

        Ok(())
    }

    /// Send RCPT TO commands for all recipients
    ///
    /// # Errors
    /// Returns an error if any RCPT TO command fails
    async fn send_rcpt_to(&self, client: &mut SmtpClient) -> Result<(), DeliveryError> {
        let Some(recipients) = self.context.envelope.recipients() else {
            return Err(SystemError::Internal("No recipients in message".to_string()).into());
        };

        let rcpt_to_timeout = Duration::from_secs(self.smtp_timeouts.rcpt_to_secs);
        for recipient in recipients.iter() {
            let recipient_addr = extract_email_address(recipient);

            let rcpt_response =
                tokio::time::timeout(rcpt_to_timeout, client.rcpt_to(&recipient_addr))
                    .await
                    .map_err(|_| {
                        TemporaryError::Timeout(format!(
                            "RCPT TO timed out after {rcpt_to_timeout:?}"
                        ))
                    })??;

            if !rcpt_response.is_success() {
                let code = rcpt_response.code;
                let message = format!(
                    "Server rejected RCPT TO {recipient_addr}: {}",
                    rcpt_response.message()
                );
                return if (500..600).contains(&code) {
                    Err(PermanentError::InvalidRecipient(message).into())
                } else {
                    Err(TemporaryError::SmtpTemporary(message).into())
                };
            }
        }

        Ok(())
    }

    /// Send DATA command and message content
    ///
    /// # Errors
    /// Returns an error if the DATA command or message sending fails
    async fn send_message_data(&self, client: &mut SmtpClient) -> Result<(), DeliveryError> {
        let data_timeout = Duration::from_secs(self.smtp_timeouts.data_secs);

        // Send DATA command
        let data_response = tokio::time::timeout(data_timeout, client.data())
            .await
            .map_err(|_| {
                TemporaryError::Timeout(format!("DATA command timed out after {data_timeout:?}"))
            })??;

        if !(300..400).contains(&data_response.code) {
            let code = data_response.code;
            let message = format!("Server rejected DATA: {}", data_response.message());
            return if (500..600).contains(&code) {
                Err(PermanentError::MessageRejected(message).into())
            } else {
                Err(TemporaryError::SmtpTemporary(message).into())
            };
        }

        // Send the actual message data
        let message_data = self
            .context
            .data
            .as_ref()
            .ok_or_else(|| SystemError::Internal("No message data to send".to_string()))?;

        let data_str = std::str::from_utf8(message_data.as_ref())
            .map_err(|e| SystemError::Internal(format!("Message data is not valid UTF-8: {e}")))?;

        let send_response = tokio::time::timeout(data_timeout, client.send_data(data_str))
            .await
            .map_err(|_| {
                TemporaryError::Timeout(format!(
                    "Sending message data timed out after {data_timeout:?}"
                ))
            })??;

        if !send_response.is_success() {
            let code = send_response.code;
            let message = format!("Server rejected message data: {}", send_response.message());
            return if (500..600).contains(&code) {
                Err(PermanentError::MessageRejected(message).into())
            } else {
                Err(TemporaryError::SmtpTemporary(message).into())
            };
        }

        Ok(())
    }
}
