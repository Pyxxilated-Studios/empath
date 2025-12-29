//! SMTP transaction execution module for outbound delivery.
//!
//! This module handles the execution of SMTP transactions for message delivery.

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use empath_common::address::Address;
use empath_common::context::Context;
use empath_common::tracing;
use empath_metrics::DeliveryMetrics;
use empath_smtp::client::{ClientError, ConnectConfig, Response as ClientResponse, SmtpClient};
use empath_spool::{BackingStore, SpooledMessageId};
use empath_tracing::traced;

use crate::error::{DeliveryError, PermanentError, SystemError, TemporaryError};
use crate::timeouts::SmtpTimeouts;
use crate::tls::{CertValidation, TlsMode};

#[derive(Debug, Clone, Copy)]
enum TlsRequirement {
    Optional,
    Required { failure_is_temporary: bool },
}

#[cfg(test)]
mod tests {
    use empath_common::context::Context;

    use super::*;

    #[derive(Default)]
    struct MockStarttlsClient {
        ehlo_responses: Vec<ClientResponse>,
        starttls_result: Option<Result<ClientResponse, ClientError>>,
        ehlo_calls: usize,
        starttls_calls: usize,
    }

    #[async_trait]
    impl StarttlsClient for MockStarttlsClient {
        async fn ehlo(&mut self, _domain: &str) -> Result<ClientResponse, ClientError> {
            let response = self
                .ehlo_responses
                .get(self.ehlo_calls)
                .cloned()
                .unwrap_or_else(|| ClientResponse::new(500, vec!["EHLO failure".to_string()]));
            self.ehlo_calls += 1;
            Ok(response)
        }

        async fn starttls(&mut self) -> Result<ClientResponse, ClientError> {
            self.starttls_calls += 1;
            self.starttls_result
                .take()
                .unwrap_or_else(|| Ok(ClientResponse::new(220, vec!["Ready for TLS".to_string()])))
        }

        fn is_tls(&self) -> bool {
            false
        }
    }

    fn make_context() -> Context {
        Context {
            id: "example.com".to_string(),
            ..Context::default()
        }
    }

    fn default_timeouts() -> SmtpTimeouts {
        SmtpTimeouts {
            ehlo_secs: 1,
            starttls_secs: 1,
            ..SmtpTimeouts::default()
        }
    }

    fn starttls_advertised_response() -> ClientResponse {
        ClientResponse::new(
            250,
            vec!["example.com", "STARTTLS"]
                .into_iter()
                .map(str::to_string)
                .collect(),
        )
    }

    fn starttls_not_supported_response() -> ClientResponse {
        ClientResponse::new(
            250,
            vec!["example.com"]
                .into_iter()
                .map(str::to_string)
                .collect(),
        )
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)]
    async fn disabled_policy_skips_even_when_advertised() -> Result<(), DeliveryError> {
        let context = make_context();
        let timeouts = default_timeouts();
        let tls_config = SmtpTlsConfig::new(TlsMode::Disabled, CertValidation::Strict);
        let transaction =
            SmtpTransaction::new(&context, "server:25".to_string(), tls_config, &timeouts);

        let mut client = MockStarttlsClient {
            ehlo_responses: vec![starttls_advertised_response()],
            starttls_result: None,
            ..Default::default()
        };

        let outcome = transaction.negotiate_tls(&mut client).await?;

        assert_eq!(outcome, TlsNegotiationOutcome::Skipped);
        assert_eq!(client.starttls_calls, 0, "STARTTLS should not be attempted");
        Ok(())
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)]
    async fn opportunistic_skips_when_server_lacks_starttls() -> Result<(), DeliveryError> {
        let context = make_context();
        let timeouts = default_timeouts();
        let tls_config = SmtpTlsConfig::new(TlsMode::Opportunistic, CertValidation::Strict);
        let transaction =
            SmtpTransaction::new(&context, "server:25".to_string(), tls_config, &timeouts);

        let mut client = MockStarttlsClient {
            ehlo_responses: vec![starttls_not_supported_response()],
            starttls_result: None,
            ..Default::default()
        };

        let outcome = transaction.negotiate_tls(&mut client).await?;

        assert_eq!(outcome, TlsNegotiationOutcome::Skipped);
        assert_eq!(client.starttls_calls, 0, "STARTTLS should not be attempted");
        assert_eq!(client.ehlo_calls, 1);
        Ok(())
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)]
    async fn opportunistic_retries_without_tls_on_failure() -> Result<(), DeliveryError> {
        let context = make_context();
        let timeouts = default_timeouts();
        let tls_config = SmtpTlsConfig::new(TlsMode::Opportunistic, CertValidation::Strict);
        let transaction =
            SmtpTransaction::new(&context, "server:25".to_string(), tls_config, &timeouts);

        let mut client = MockStarttlsClient {
            ehlo_responses: vec![starttls_advertised_response()],
            starttls_result: Some(Err(ClientError::TlsError("handshake failed".to_string()))),
            ..Default::default()
        };

        let outcome = transaction.negotiate_tls(&mut client).await?;

        assert_eq!(outcome, TlsNegotiationOutcome::RetryWithoutTls);
        assert_eq!(client.starttls_calls, 1);
        Ok(())
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)]
    async fn require_tls_propagates_failure_as_permanent() -> Result<(), DeliveryError> {
        let context = make_context();
        let timeouts = default_timeouts();
        let tls_config = SmtpTlsConfig::new(TlsMode::Required, CertValidation::Strict);
        let transaction =
            SmtpTransaction::new(&context, "secure:25".to_string(), tls_config, &timeouts);

        let mut client = MockStarttlsClient {
            ehlo_responses: vec![starttls_advertised_response()],
            starttls_result: Some(Err(ClientError::TlsError("cert mismatch".to_string()))),
            ..Default::default()
        };

        let err: DeliveryError = match transaction.negotiate_tls(&mut client).await {
            Ok(outcome) => {
                return Err(DeliveryError::System(SystemError::Internal(format!(
                    "expected TLS failure, got {outcome:?}"
                ))));
            }
            Err(e) => e,
        };

        match err {
            DeliveryError::Permanent(PermanentError::TlsRequired(msg)) => {
                assert!(
                    msg.contains("cert mismatch"),
                    "expected TLS error message to bubble up"
                );
            }
            other => {
                return Err(DeliveryError::System(SystemError::Internal(format!(
                    "expected permanent TLS-required error, got {other:?}"
                ))));
            }
        }
        assert_eq!(client.starttls_calls, 1);
        Ok(())
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)]
    async fn require_tls_succeeds_when_starttls_succeeds() -> Result<(), DeliveryError> {
        let context = make_context();
        let timeouts = default_timeouts();
        let tls_config = SmtpTlsConfig::new(TlsMode::Required, CertValidation::Strict);
        let transaction =
            SmtpTransaction::new(&context, "secure:25".to_string(), tls_config, &timeouts);

        let mut client = MockStarttlsClient {
            ehlo_responses: vec![
                starttls_advertised_response(),
                starttls_not_supported_response(),
            ],
            starttls_result: Some(Ok(ClientResponse::new(
                220,
                vec!["Ready to start TLS".to_string()],
            ))),
            ..Default::default()
        };

        let outcome = transaction.negotiate_tls(&mut client).await?;

        assert_eq!(outcome, TlsNegotiationOutcome::Success);
        assert_eq!(client.starttls_calls, 1);
        Ok(())
    }
}

impl TlsRequirement {
    const fn is_required(self) -> bool {
        matches!(self, Self::Required { .. })
    }

    const fn failure_is_temporary(self) -> bool {
        matches!(
            self,
            Self::Required {
                failure_is_temporary: true
            }
        )
    }
}

/// TLS failure policy when TLS is required.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TlsFailurePolicy {
    Permanent,
    Temporary,
}

impl TlsFailurePolicy {
    const fn is_temporary(self) -> bool {
        matches!(self, Self::Temporary)
    }
}

/// TLS configuration for outbound SMTP transactions.
#[derive(Debug, Clone, Copy)]
pub struct SmtpTlsConfig {
    pub mode: TlsMode,
    pub validation: CertValidation,
    pub failure_policy: TlsFailurePolicy,
}

impl SmtpTlsConfig {
    #[must_use]
    pub const fn new(mode: TlsMode, validation: CertValidation) -> Self {
        Self {
            mode,
            validation,
            failure_policy: TlsFailurePolicy::Permanent,
        }
    }

    #[must_use]
    pub const fn with_failure_policy(mut self, failure_policy: TlsFailurePolicy) -> Self {
        self.failure_policy = failure_policy;
        self
    }
}

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

#[derive(Debug, Clone, Copy)]
enum EhloStage {
    Initial,
    AfterStarttls,
}

/// Extracts the email address string from an Address (for SMTP commands)
fn extract_email_address(address: &Address) -> String {
    address.to_string()
}

/// Represents a single SMTP transaction for delivering a message.
pub struct SmtpTransaction<'a> {
    /// The message context containing envelope and data
    context: &'a Context,
    /// The body source used for DATA transmission
    body_source: MessageBodySource<'a>,
    /// The SMTP server address (host:port)
    server_address: String,
    /// Whether TLS should be attempted at all (disabled policy skips STARTTLS)
    attempt_tls: bool,
    /// TLS requirement policy
    tls_requirement: TlsRequirement,
    /// TLS certificate validation policy
    tls_validation: CertValidation,
    /// Timeout configuration for SMTP operations
    smtp_timeouts: &'a SmtpTimeouts,
    /// Optional HELO/EHLO override
    helo_domain: Option<String>,
    /// Optional local bind configuration
    connect_config: Option<ConnectConfig>,
    /// Optional metrics collector for binding observability
    metrics: Option<&'a DeliveryMetrics>,
    /// Optional binding name for metrics labeling
    binding_name: Option<&'a str>,
}

enum MessageBodySource<'a> {
    Context,
    Spool {
        spool: &'a Arc<dyn BackingStore>,
        message_id: &'a SpooledMessageId,
    },
}

#[async_trait]
trait StarttlsClient {
    async fn ehlo(&mut self, domain: &str) -> Result<ClientResponse, ClientError>;
    async fn starttls(&mut self) -> Result<ClientResponse, ClientError>;
    fn is_tls(&self) -> bool;
}

#[async_trait]
impl StarttlsClient for SmtpClient {
    async fn ehlo(&mut self, domain: &str) -> Result<ClientResponse, ClientError> {
        self.ehlo(domain).await
    }

    async fn starttls(&mut self) -> Result<ClientResponse, ClientError> {
        self.starttls().await
    }

    fn is_tls(&self) -> bool {
        self.is_tls()
    }
}

impl<'a> SmtpTransaction<'a> {
    /// Create a new SMTP transaction.
    #[must_use]
    pub const fn new(
        context: &'a Context,
        server_address: String,
        tls_config: SmtpTlsConfig,
        smtp_timeouts: &'a SmtpTimeouts,
    ) -> Self {
        let tls_requirement = if tls_config.mode.require_tls() {
            TlsRequirement::Required {
                failure_is_temporary: tls_config.failure_policy.is_temporary(),
            }
        } else {
            TlsRequirement::Optional
        };

        Self {
            context,
            body_source: MessageBodySource::Context,
            server_address,
            attempt_tls: tls_config.mode.attempt_tls(),
            tls_requirement,
            tls_validation: tls_config.validation,
            smtp_timeouts,
            helo_domain: None,
            connect_config: None,
            metrics: None,
            binding_name: None,
        }
    }

    #[must_use]
    pub fn with_spool_body(
        mut self,
        spool: &'a Arc<dyn BackingStore>,
        message_id: &'a SpooledMessageId,
    ) -> Self {
        self.body_source = MessageBodySource::Spool { spool, message_id };
        self
    }

    #[must_use]
    pub fn with_helo_domain(mut self, helo_domain: Option<String>) -> Self {
        self.helo_domain = helo_domain;
        self
    }

    #[must_use]
    pub fn with_connect_config(mut self, connect_config: Option<ConnectConfig>) -> Self {
        self.connect_config = connect_config;
        self
    }

    #[must_use]
    pub fn with_metrics(mut self, metrics: Option<&'a DeliveryMetrics>) -> Self {
        self.metrics = metrics;
        self
    }

    #[must_use]
    pub fn with_binding_name(mut self, binding_name: Option<&'a str>) -> Self {
        self.binding_name = binding_name;
        self
    }

    fn helo_domain(&self) -> &str {
        self.helo_domain.as_deref().unwrap_or(&self.context.id)
    }

    fn require_tls_error(&self, message: String) -> DeliveryError {
        if self.tls_requirement.failure_is_temporary() {
            TemporaryError::TlsHandshakeFailed(message).into()
        } else {
            PermanentError::TlsRequired(message).into()
        }
    }

    fn record_connect_duration(&self, start: Instant) {
        if let Some(metrics) = self.metrics {
            metrics.record_binding_connect_duration(self.binding_name, start.elapsed());
        }
    }

    fn record_ehlo_duration(&self, start: Instant) {
        if let Some(metrics) = self.metrics {
            metrics.record_binding_ehlo_duration(self.binding_name, start.elapsed());
        }
    }

    async fn connect_and_greet(&self) -> Result<SmtpClient, DeliveryError> {
        let start = Instant::now();
        let mut client = match &self.connect_config {
            Some(config) => {
                SmtpClient::connect_with_config(
                    &self.server_address,
                    self.server_address.clone(),
                    config,
                )
                .await
            }
            None => SmtpClient::connect(&self.server_address, self.server_address.clone()).await,
        }
        .map_err(|e| {
            TemporaryError::ConnectionFailed(format!(
                "Failed to connect to {}: {e}",
                self.server_address
            ))
        })?
        .accept_invalid_certs(self.tls_validation.accepts_invalid());

        let greeting = client.read_greeting().await?;

        if !greeting.is_success() {
            return Err(TemporaryError::ServerBusy(format!(
                "Server rejected connection: {}",
                greeting.message()
            ))
            .into());
        }

        self.record_connect_duration(start);

        Ok(client)
    }

    /// Execute the complete SMTP transaction.
    #[traced(instrument(level = tracing::Level::INFO, skip(self), fields(server = %self.server_address, message_id = %self.context.id)), timing(precision = "ms"))]
    pub async fn execute(self) -> Result<(), DeliveryError> {
        let client = self.connect_and_greet().await?;
        self.execute_with_client_internal(client, false)
            .await
            .map(|_| ())
    }

    /// Execute the SMTP transaction using a pre-connected client (from a pool).
    pub async fn execute_with_client(
        self,
        client: SmtpClient,
    ) -> Result<Option<SmtpClient>, DeliveryError> {
        self.execute_with_client_internal(client, true).await
    }

    async fn execute_with_client_internal(
        self,
        mut client: SmtpClient,
        pooled: bool,
    ) -> Result<Option<SmtpClient>, DeliveryError> {
        if self.tls_validation.accepts_invalid() {
            tracing::warn!(
                server = %self.server_address,
                "SECURITY WARNING: TLS certificate validation is disabled for this connection"
            );
        }

        let tls_outcome = self.negotiate_tls(&mut client).await?;

        if tls_outcome == TlsNegotiationOutcome::RetryWithoutTls {
            tracing::info!(
                server = %self.server_address,
                "Reconnecting without STARTTLS per RFC 3207 Section 4.1"
            );

            drop(client);
            client = self.connect_and_greet().await?;

            let helo_domain = self.helo_domain();
            let ehlo_timeout = Duration::from_secs(self.smtp_timeouts.ehlo_secs);
            let ehlo_start = Instant::now();
            let ehlo_response = tokio::time::timeout(ehlo_timeout, client.ehlo(helo_domain))
                .await
                .map_err(|_| {
                    TemporaryError::Timeout(format!("EHLO timed out after {ehlo_timeout:?}"))
                })??;
            self.record_ehlo_duration(ehlo_start);

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

        self.send_mail_from(&mut client).await?;
        self.send_rcpt_to(&mut client).await?;
        Box::pin(self.send_message_data(&mut client)).await?;

        if pooled {
            let rset_timeout = Duration::from_secs(self.smtp_timeouts.quit_secs);
            match tokio::time::timeout(rset_timeout, client.rset()).await {
                Ok(Ok(response)) if response.is_success() => Ok(Some(client)),
                Ok(Ok(response)) => {
                    tracing::warn!(
                        server = %self.server_address,
                        response = %response.message(),
                        "RSET failed after delivery; dropping pooled connection"
                    );
                    Ok(None)
                }
                Ok(Err(err)) => {
                    tracing::warn!(
                        server = %self.server_address,
                        error = %err,
                        "RSET failed after delivery; dropping pooled connection"
                    );
                    Ok(None)
                }
                Err(err) => {
                    tracing::warn!(
                        server = %self.server_address,
                        timeout = ?rset_timeout,
                        "RSET timed out after delivery; dropping pooled connection: {err}"
                    );
                    Ok(None)
                }
            }
        } else {
            let quit_timeout = Duration::from_secs(self.smtp_timeouts.quit_secs);
            if let Err(e) = tokio::time::timeout(quit_timeout, client.quit()).await {
                tracing::warn!(
                    server = %self.server_address,
                    timeout = ?quit_timeout,
                    "QUIT command timed out after successful delivery: {e}"
                );
            }

            Ok(None)
        }
    }

    async fn negotiate_tls(
        &self,
        client: &mut (impl StarttlsClient + Send),
    ) -> Result<TlsNegotiationOutcome, DeliveryError> {
        let helo_domain = self.helo_domain();

        if client.is_tls() {
            return self
                .negotiate_tls_for_existing_session(client, helo_domain)
                .await;
        }

        if !self.attempt_tls {
            tracing::debug!(
                domain = %self.context.id,
                "TLS negotiation skipped due to disabled policy"
            );
            return Ok(TlsNegotiationOutcome::Skipped);
        }

        let ehlo_response = self
            .send_ehlo(client, helo_domain, EhloStage::Initial)
            .await?;
        if !ehlo_response.is_success() {
            return Err(TemporaryError::SmtpTemporary(format!(
                "Server rejected EHLO: {}",
                ehlo_response.message()
            ))
            .into());
        }

        let supports_starttls = Self::supports_starttls(&ehlo_response);
        if !supports_starttls && !self.tls_requirement.is_required() {
            return Ok(TlsNegotiationOutcome::Skipped);
        }

        if self.tls_requirement.is_required() || supports_starttls {
            return self.negotiate_starttls(client, helo_domain).await;
        }

        Ok(TlsNegotiationOutcome::Skipped)
    }

    async fn negotiate_tls_for_existing_session(
        &self,
        client: &mut (impl StarttlsClient + Send),
        helo_domain: &str,
    ) -> Result<TlsNegotiationOutcome, DeliveryError> {
        let ehlo_response = self
            .send_ehlo(client, helo_domain, EhloStage::Initial)
            .await?;
        if !ehlo_response.is_success() {
            return Err(TemporaryError::SmtpTemporary(format!(
                "Server rejected EHLO: {}",
                ehlo_response.message()
            ))
            .into());
        }
        Ok(TlsNegotiationOutcome::Success)
    }

    async fn send_ehlo(
        &self,
        client: &mut (impl StarttlsClient + Send),
        helo_domain: &str,
        stage: EhloStage,
    ) -> Result<ClientResponse, DeliveryError> {
        let ehlo_timeout = Duration::from_secs(self.smtp_timeouts.ehlo_secs);
        let ehlo_start = Instant::now();
        let ehlo_response: ClientResponse =
            tokio::time::timeout(ehlo_timeout, client.ehlo(helo_domain))
                .await
                .map_err(|_| {
                    let message = match stage {
                        EhloStage::Initial => {
                            format!("EHLO timed out after {ehlo_timeout:?}")
                        }
                        EhloStage::AfterStarttls => {
                            format!("EHLO after STARTTLS timed out after {ehlo_timeout:?}")
                        }
                    };
                    TemporaryError::Timeout(message)
                })??;
        self.record_ehlo_duration(ehlo_start);
        Ok(ehlo_response)
    }

    fn supports_starttls(response: &ClientResponse) -> bool {
        response
            .message()
            .lines()
            .any(|line: &str| line.to_uppercase().contains("STARTTLS"))
    }

    async fn negotiate_starttls(
        &self,
        client: &mut (impl StarttlsClient + Send),
        helo_domain: &str,
    ) -> Result<TlsNegotiationOutcome, DeliveryError> {
        let starttls_timeout = Duration::from_secs(self.smtp_timeouts.starttls_secs);
        let starttls_result: Result<ClientResponse, String> =
            tokio::time::timeout(starttls_timeout, client.starttls())
                .await
                .map_err(|_| format!("STARTTLS timed out after {starttls_timeout:?}"))
                .and_then(|r| r.map_err(|e| format!("STARTTLS failed: {e}")));

        let starttls_response: ClientResponse = match starttls_result {
            Ok(response) => response,
            Err(error_msg) => {
                if self.tls_requirement.is_required() {
                    return Err(self.require_tls_error(error_msg));
                }
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
            if self.tls_requirement.is_required() {
                return Err(self.require_tls_error(message));
            }
            tracing::info!(
                domain = helo_domain,
                response = %starttls_response.message(),
                "Server rejected STARTTLS for opportunistic TLS, will retry without TLS per RFC 3207"
            );
            return Ok(TlsNegotiationOutcome::RetryWithoutTls);
        }

        let ehlo_response = self
            .send_ehlo(client, helo_domain, EhloStage::AfterStarttls)
            .await?;
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
    }

    async fn send_mail_from(&self, client: &mut SmtpClient) -> Result<(), DeliveryError> {
        let sender = self
            .context
            .envelope
            .sender()
            .map_or_else(|| "<>".to_string(), extract_email_address);

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

    async fn send_message_data(&self, client: &mut SmtpClient) -> Result<(), DeliveryError> {
        let data_timeout = Duration::from_secs(self.smtp_timeouts.data_secs);

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

        let send_response = match self.body_source {
            MessageBodySource::Context => {
                let message_data =
                    self.context.data.as_ref().ok_or_else(|| {
                        SystemError::Internal("No message data to send".to_string())
                    })?;

                tokio::time::timeout(data_timeout, client.send_data_bytes(message_data.as_ref()))
                    .await
                    .map_err(|_| {
                        TemporaryError::Timeout(format!(
                            "Sending message data timed out after {data_timeout:?}"
                        ))
                    })??
            }
            MessageBodySource::Spool { spool, message_id } => {
                let body_stream = spool
                    .open_message_stream(message_id)
                    .await
                    .map_err(|e| SystemError::SpoolRead(e.to_string()))?;

                Box::pin(tokio::time::timeout(
                    data_timeout,
                    client.send_data_stream(body_stream),
                ))
                .await
                .map_err(|_| {
                    TemporaryError::Timeout(format!(
                        "Sending message data timed out after {data_timeout:?}"
                    ))
                })??
            }
        };

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
