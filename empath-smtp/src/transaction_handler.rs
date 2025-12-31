//! Business logic handler for SMTP sessions.
//!
//! This module provides the `BusinessHandler` trait that separates business
//! logic (validation, spooling, module dispatch) from protocol state management
//! (FSM) and I/O orchestration.

use std::borrow::Cow;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use empath_common::audit::AuditLogger;
#[cfg(test)]
use empath_common::audit::AuditLoggerBuilder;
use empath_common::context::{Context, SPAN_ID_CONTEXT_KEY, TRACE_ID_CONTEXT_KEY};
use empath_common::status::Status;
use empath_ffi::modules::{self, ModuleRegistry};
use empath_metrics::metrics;
use empath_spool::BackingStore;
use opentelemetry::trace::TraceContextExt as _;
use sha2::{Digest, Sha256};
use tracing_opentelemetry::OpenTelemetrySpanExt as _;
use ulid::Ulid;

use crate::{State, arc, dkim, dmarc};

/// SMTP transaction handler for business logic.
///
/// This trait separates business concerns (validation, spooling, auditing)
/// from protocol concerns (state transitions) and I/O concerns (send/receive).
///
/// # Design Rationale
///
/// By separating business logic into a trait, we achieve:
/// - **Testability**: Business logic can be tested without I/O or networking
/// - **Flexibility**: Different implementations for production vs testing
/// - **Single Responsibility**: Each layer has a clear, focused purpose
/// - **Dependency Injection**: Easily swap implementations
///
/// # Responsibilities
///
/// The transaction handler is responsible for:
/// - Module-based validation dispatch
/// - Message spooling
/// - Response generation (success/failure messages)
/// - Audit logging
/// - Event notification
///
/// # Lifecycle
///
/// The handler is called after FSM state transitions:
/// 1. FSM transitions to new state (pure protocol logic)
/// 2. `SmtpTransactionHandler` validates the transition (business rules)
/// 3. `SmtpTransactionHandler` performs work (spooling, auditing, etc.)
/// 4. `Response` is generated and sent to client
#[async_trait]
pub trait SmtpTransactionHandler: Send + Sync {
    /// Validate a Connect event (new connection established)
    ///
    /// Called when a new client connects, before sending the greeting.
    ///
    /// # Arguments
    ///
    /// * `ctx` - Business context for validation and response
    ///
    /// # Returns
    ///
    /// `true` if the connection should be accepted, `false` to reject
    async fn validate_connect(&mut self, ctx: &mut Context) -> bool;

    /// Validate an EHLO/HELO command
    ///
    /// Called after the client sends EHLO or HELO.
    ///
    /// # Arguments
    ///
    /// * `ctx` - Business context for validation and response
    ///
    /// # Returns
    ///
    /// `true` if the EHLO/HELO should be accepted, `false` to reject
    async fn validate_ehlo(&mut self, ctx: &mut Context) -> bool;

    /// Validate a MAIL FROM command
    ///
    /// Called after the client sends MAIL FROM.
    ///
    /// # Arguments
    ///
    /// * `ctx` - Business context for validation and response
    ///
    /// # Returns
    ///
    /// `true` if the MAIL FROM should be accepted, `false` to reject
    async fn validate_mail_from(&mut self, ctx: &mut Context) -> bool;

    /// Validate an RCPT TO command
    ///
    /// Called after the client sends RCPT TO.
    ///
    /// # Arguments
    ///
    /// * `ctx` - Business context for validation and response
    ///
    /// # Returns
    ///
    /// `true` if the RCPT TO should be accepted, `false` to reject
    async fn validate_rcpt_to(&mut self, ctx: &mut Context) -> bool;

    /// Validate an AUTH command
    ///
    /// Called after the client submits authentication credentials.
    ///
    /// # Arguments
    ///
    /// * `ctx` - Business context for validation and response
    ///
    /// # Returns
    ///
    /// `true` if the AUTH should be accepted, `false` to reject
    async fn validate_auth(&mut self, ctx: &mut Context) -> bool;

    /// Validate and process a complete message (after DATA)
    ///
    /// Called after the client sends the complete message (after ".").
    /// This method both validates the message and performs the spooling
    /// work if validation passes.
    ///
    /// # Arguments
    ///
    /// * `ctx` - Business context for validation, spooling, and response
    ///
    /// # Returns
    ///
    /// `true` if the message was accepted and spooled, `false` if rejected
    async fn handle_message(&mut self, ctx: &mut Context) -> bool;
}

/// Default SMTP transaction handler that uses the module system for validation.
///
/// This implementation delegates all validation to the FFI module system,
/// which allows external plugins to implement business rules.
///
/// # Example
///
/// ```rust
/// use std::sync::Arc;
/// use empath_common::audit::AuditLoggerBuilder;
/// use empath_smtp::{transaction_handler::DefaultSmtpTransactionHandler, spf::SpfConfig};
/// use empath_spool::BackingStore;
///
/// # fn example(spool: Arc<dyn BackingStore>, peer: std::net::SocketAddr) {
/// let handler = DefaultSmtpTransactionHandler::new(
///     Some(spool),
///     peer,
///     None,
///     SpfConfig::default(),
///     false,
///     false,
///     AuditLoggerBuilder::default().build(),
/// );
/// // Use handler with session orchestrator
/// # }
/// ```
pub struct DefaultSmtpTransactionHandler {
    /// Optional spool for message persistence
    spool: Option<Arc<dyn BackingStore>>,
    /// Client peer address for audit logging
    peer: SocketAddr,
    /// Optional SPF DNS resolver for sender validation
    spf_dns_resolver: Option<Arc<dyn crate::spf::SpfDnsResolver>>,
    /// SPF configuration
    spf_config: crate::spf::SpfConfig,
    /// Whether to reject messages without a passing DKIM verification
    require_dkim: bool,
    /// Whether to enforce DMARC policy on failure
    dmarc_enforce: bool,
    /// Audit logger for message lifecycle events
    audit_logger: Arc<dyn AuditLogger>,
}

impl DefaultSmtpTransactionHandler {
    /// Creates a new default SMTP transaction handler.
    ///
    /// # Arguments
    ///
    /// * `spool` - Optional message spool for persistence
    /// * `peer` - Client peer address for audit logging
    /// * `spf_dns_resolver` - Optional SPF DNS resolver for sender validation
    /// * `spf_config` - SPF configuration
    /// * `require_dkim` - Whether to reject messages without DKIM pass
    /// * `dmarc_enforce` - Whether to enforce DMARC policy on failure
    /// * `audit_logger` - Audit logger for message lifecycle events
    #[must_use]
    pub const fn new(
        spool: Option<Arc<dyn BackingStore>>,
        peer: SocketAddr,
        spf_dns_resolver: Option<Arc<dyn crate::spf::SpfDnsResolver>>,
        spf_config: crate::spf::SpfConfig,
        require_dkim: bool,
        dmarc_enforce: bool,
        audit_logger: Arc<dyn AuditLogger>,
    ) -> Self {
        Self {
            spool,
            peer,
            spf_dns_resolver,
            spf_config,
            require_dkim,
            dmarc_enforce,
            audit_logger,
        }
    }

    fn client_ip(&self, ctx: &Context) -> IpAddr {
        ctx.session
            .get("client_ip")
            .and_then(|value| value.parse().ok())
            .unwrap_or_else(|| self.peer.ip())
    }

    fn client_socket(&self, ctx: &Context) -> SocketAddr {
        let port = ctx
            .session
            .get("client_port")
            .and_then(|value| value.parse::<u16>().ok())
            .unwrap_or_else(|| self.peer.port());
        SocketAddr::new(self.client_ip(ctx), port)
    }

    /// Spool a message after validation passes.
    ///
    /// This is an internal helper that handles:
    /// - Writing the message to the spool
    /// - Setting success/failure responses
    /// - Clearing transaction metadata
    /// - Audit logging
    /// - Event dispatching
    ///
    /// # Arguments
    ///
    /// * `ctx` - Business context containing the message data
    async fn spool_message(&self, ctx: &mut Context) {
        Self::record_trace_context(ctx);
        self.insert_received_header(ctx);
        let tracking_id = if let Some(spool) = &self.spool
            && ctx.data.is_some()
        {
            match spool.write(ctx).await {
                Ok(id) => Some(id),
                Err(e) => {
                    tracing::error!("Failed to spool message: {e}");
                    ctx.response = Some((
                        Status::ActionUnavailable,
                        Cow::Borrowed("Please try again later"),
                    ));
                    return;
                }
            }
        } else {
            None
        };

        // Clear transaction state after successful acceptance
        // declared_size is transaction context
        ctx.transaction.remove("declared_size");

        // Set success response with tracking ID
        ctx.response = Some((
            Status::Ok,
            tracking_id.as_ref().map_or_else(
                || Cow::Borrowed("Ok: queued"),
                |id| Cow::Owned(format!("Ok: queued as {id}")),
            ),
        ));

        // Audit log: Message received and spooled
        if let Some(id) = &tracking_id {
            let sender = ctx.sender_email();
            let recipients = ctx.recipient_emails();
            let size = ctx.data.as_ref().map_or(0, |d| d.len());
            let from_ip = self.client_socket(ctx).to_string();

            self.audit_logger.log_message_received(
                &id.to_string(),
                sender,
                &recipients,
                size,
                &from_ip,
            );
        }

        // Dispatch message received event (fail open).
        let _ = ModuleRegistry::dispatch_with_registry(
            ModuleRegistry::global(),
            modules::Event::Event(modules::Ev::SmtpMessageReceived),
            ctx,
        )
        .await;
    }

    fn record_trace_context(ctx: &mut Context) {
        if ctx.transaction.contains_key(TRACE_ID_CONTEXT_KEY) {
            return;
        }

        let span = tracing::Span::current();
        let otel_ctx = span.context();
        let span_ref = otel_ctx.span();
        let span_context = span_ref.span_context();

        if span_context.is_valid() {
            ctx.transaction.insert(
                TRACE_ID_CONTEXT_KEY.into(),
                span_context.trace_id().to_string(),
            );
            ctx.transaction.insert(
                SPAN_ID_CONTEXT_KEY.into(),
                span_context.span_id().to_string(),
            );
        }
    }

    fn insert_received_header(&self, ctx: &mut Context) {
        let Some(existing_data) = ctx.data.as_ref() else {
            return;
        };

        let header = self.build_received_header(ctx, Utc::now());
        let mut buffer = Vec::with_capacity(header.len() + existing_data.len());
        buffer.extend_from_slice(header.as_bytes());
        buffer.extend_from_slice(existing_data);

        ctx.data = Some(Arc::from(buffer));
    }

    fn build_received_header(&self, ctx: &Context, timestamp: DateTime<Utc>) -> String {
        let client_ip = self.client_ip(ctx);
        let ip_repr = format!("[{client_ip}]");
        let helo = ctx.id().trim();
        let from_value = if helo.is_empty() {
            format!("from {ip_repr}")
        } else {
            format!("from {helo} ({helo} {ip_repr})")
        };

        let server = if ctx.banner.is_empty() {
            "localhost"
        } else {
            ctx.banner.as_ref()
        };

        let is_tls = ctx
            .session
            .get("tls")
            .is_some_and(|value| value.eq_ignore_ascii_case("true"));

        let mut proto = if ctx.extended {
            "ESMTP".to_string()
        } else {
            "SMTP".to_string()
        };
        if is_tls {
            proto.push('S');
        }

        let recipients = ctx
            .recipient_emails()
            .into_iter()
            .map(|addr| format!("<{addr}>"))
            .collect::<Vec<_>>();
        let for_segment = if recipients.is_empty() {
            String::new()
        } else {
            format!(" for {}", recipients.join(", "))
        };

        let id = Ulid::new();
        let timestamp_str = timestamp.to_rfc2822();

        format!(
            "Received: {from_value} by {server} (Empath) with {proto} id <{id}>{for_segment}; {timestamp_str}\r\n"
        )
    }
}

#[async_trait]
impl SmtpTransactionHandler for DefaultSmtpTransactionHandler {
    async fn validate_connect(&mut self, ctx: &mut Context) -> bool {
        // Dispatch connection opened event first (fail open).
        let _ = ModuleRegistry::dispatch_with_registry(
            ModuleRegistry::global(),
            modules::Event::Event(modules::Ev::ConnectionOpened),
            ctx,
        )
        .await;

        // Then validate
        matches!(
            ModuleRegistry::dispatch_with_registry(
                ModuleRegistry::global(),
                modules::Event::Validate(modules::validate::Event::Connect),
                ctx,
            )
            .await,
            modules::DispatchResult::Accept
        )
    }

    async fn validate_ehlo(&mut self, ctx: &mut Context) -> bool {
        matches!(
            ModuleRegistry::dispatch_with_registry(
                ModuleRegistry::global(),
                modules::Event::Validate(modules::validate::Event::Ehlo),
                ctx,
            )
            .await,
            modules::DispatchResult::Accept
        )
    }

    async fn validate_mail_from(&mut self, ctx: &mut Context) -> bool {
        // Perform SPF evaluation if DNS resolver is available
        if let Some(resolver) = &self.spf_dns_resolver
            && let Some(sender) = ctx.sender_email().cloned()
        {
            // Extract IP address from peer socket address
            let client_ip = self.client_ip(ctx);

            // HELO/EHLO hostname is stored in ctx.id
            // Clone to avoid borrow checker issues when mutating ctx later
            let helo_domain = ctx.id().to_string();

            // Create SPF evaluator and perform evaluation
            let evaluator = crate::spf::SpfEvaluator::new(Arc::clone(resolver), self.spf_config);

            let spf_evaluation = evaluator.evaluate(&sender, client_ip, &helo_domain).await;

            // Store SPF results in transaction context
            ctx.transaction.insert(
                Cow::Borrowed("spf_result"),
                spf_evaluation.result.to_string(),
            );
            if let Some(sender_domain) = dmarc::domain_from_address(sender.as_str()) {
                ctx.transaction
                    .insert(Cow::Borrowed("spf_domain"), sender_domain);
            }
            ctx.transaction
                .insert(Cow::Borrowed("spf_helo_domain"), helo_domain.clone());
            if let Some(explanation) = spf_evaluation.explanation {
                ctx.transaction
                    .insert(Cow::Borrowed("spf_explanation"), explanation);
            }

            tracing::info!(
                sender = %sender,
                client_ip = %client_ip,
                helo_domain = %helo_domain,
                spf_result = %spf_evaluation.result,
                "SPF evaluation completed"
            );
        }

        // Dispatch to modules for additional validation
        matches!(
            ModuleRegistry::dispatch_with_registry(
                ModuleRegistry::global(),
                modules::Event::Validate(modules::validate::Event::MailFrom),
                ctx,
            )
            .await,
            modules::DispatchResult::Accept
        )
    }

    async fn validate_rcpt_to(&mut self, ctx: &mut Context) -> bool {
        matches!(
            ModuleRegistry::dispatch_with_registry(
                ModuleRegistry::global(),
                modules::Event::Validate(modules::validate::Event::RcptTo),
                ctx,
            )
            .await,
            modules::DispatchResult::Accept
        )
    }

    async fn validate_auth(&mut self, ctx: &mut Context) -> bool {
        let accepted = matches!(
            ModuleRegistry::dispatch_with_registry(
                ModuleRegistry::global(),
                modules::Event::Validate(modules::validate::Event::Auth),
                ctx,
            )
            .await,
            modules::DispatchResult::Accept
        );

        if accepted {
            ctx.session
                .insert(Cow::Borrowed("auth"), "true".to_string());
            if ctx.response.is_none() {
                ctx.response = Some((
                    Status::Unknown(235),
                    Cow::Borrowed("Authentication successful"),
                ));
            }
        } else {
            ctx.session.remove("auth");
        }

        ctx.session.remove("auth_password");

        accepted
    }

    async fn handle_message(&mut self, ctx: &mut Context) -> bool {
        self.evaluate_arc(ctx).await;
        self.evaluate_dkim(ctx).await;
        self.evaluate_dmarc(ctx).await;

        let dmarc_policy = ctx.transaction.get("dmarc_policy").cloned();
        let dmarc_result = ctx
            .transaction
            .get("dmarc_result")
            .cloned()
            .unwrap_or_else(|| "none".to_string());

        let dmarc_action = if self.dmarc_enforce {
            dmarc_enforcement_action(dmarc_result.as_str(), dmarc_policy.as_deref())
        } else {
            None
        };

        ctx.transaction.insert(
            Cow::Borrowed("dmarc_action"),
            dmarc_action.unwrap_or("none").to_string(),
        );

        if let Some(action) = dmarc_action {
            if let Some(metrics) = metrics() {
                metrics.smtp.record_dmarc_enforcement(
                    action,
                    dmarc_policy.as_deref(),
                    dmarc_result.as_str(),
                );
            }
            tracing::warn!(
                dmarc_result = %dmarc_result,
                dmarc_policy = dmarc_policy.as_deref().unwrap_or("none"),
                dmarc_action = action,
                "DMARC policy enforcement rejected message"
            );
            let response = match action {
                "quarantine" => "DMARC policy quarantine (treated as reject)",
                _ => "DMARC policy rejected message",
            };
            ctx.response = Some((Status::Error, Cow::Borrowed(response)));
            return false;
        }

        if self.require_dkim && !dkim_is_pass(ctx) {
            let result = ctx
                .transaction
                .get("dkim_result")
                .map_or("none", String::as_str);
            if let Some(metrics) = metrics() {
                metrics.smtp.record_dkim_rejection(result);
            }
            tracing::warn!(dkim_result = %result, "DKIM verification failed");
            ctx.response = Some((Status::Error, Cow::Borrowed("DKIM verification failed")));
            return false;
        }

        // Dispatch validation
        let valid = matches!(
            ModuleRegistry::dispatch_with_registry(
                ModuleRegistry::global(),
                modules::Event::Validate(modules::validate::Event::Data),
                ctx,
            )
            .await,
            modules::DispatchResult::Accept
        );
        // Persist any header/body mutations performed by modules.
        ctx.persist_message_parts();

        // If validation passed, do the work (spooling)
        if valid {
            // Check if any module set a rejection response
            // Positive responses are < 400 (2xx and 3xx codes)
            let should_spool = ctx
                .response
                .as_ref()
                .is_none_or(|(status, _)| !status.is_temporary() && !status.is_permanent());

            if should_spool {
                self.spool_message(ctx).await;
            }
        }

        valid
    }
}

impl DefaultSmtpTransactionHandler {
    async fn evaluate_dkim(&self, ctx: &mut Context) {
        let Some(raw) = ctx.data.as_ref() else {
            ctx.transaction
                .insert(Cow::Borrowed("dkim_result"), "none".to_string());
            ctx.transaction.insert(
                Cow::Borrowed("dkim_reason"),
                "missing_message_data".to_string(),
            );
            return;
        };

        let evaluation = dkim::verify(raw).await;

        ctx.transaction
            .insert(Cow::Borrowed("dkim_result"), evaluation.result.to_string());
        if let Some(domain) = evaluation.domain {
            ctx.transaction.insert(Cow::Borrowed("dkim_domain"), domain);
        }
        if let Some(selector) = evaluation.selector {
            ctx.transaction
                .insert(Cow::Borrowed("dkim_selector"), selector);
        }
        if let Some(error) = evaluation.error {
            ctx.transaction.insert(Cow::Borrowed("dkim_error"), error);
        }

        if let Some(metrics) = metrics() {
            metrics.smtp.record_dkim_verification(evaluation.result);
        }

        match evaluation.result {
            "fail" | "permerror" | "temperror" => {
                let domain = ctx.transaction.get("dkim_domain").map(String::as_str);
                let selector = ctx.transaction.get("dkim_selector").map(String::as_str);
                let error = ctx.transaction.get("dkim_error").map(String::as_str);
                tracing::warn!(
                    dkim_result = %evaluation.result,
                    dkim_domain = domain.unwrap_or("unknown"),
                    dkim_selector = selector.unwrap_or("unknown"),
                    dkim_error = error.unwrap_or(""),
                    "DKIM verification failed"
                );
            }
            "neutral" | "none" => {
                tracing::debug!(dkim_result = %evaluation.result, "DKIM verification skipped");
            }
            _ => {}
        }
    }

    async fn evaluate_arc(&self, ctx: &mut Context) {
        let Some(raw) = ctx.data.as_ref() else {
            ctx.transaction
                .insert(Cow::Borrowed("arc_result"), "none".to_string());
            ctx.transaction.insert(
                Cow::Borrowed("arc_reason"),
                "missing_message_data".to_string(),
            );
            return;
        };

        let evaluation = arc::verify(raw).await;

        ctx.transaction
            .insert(Cow::Borrowed("arc_result"), evaluation.result.to_string());
        ctx.transaction.insert(
            Cow::Borrowed("arc_chain_len"),
            evaluation.chain_len.to_string(),
        );
        ctx.transaction.insert(
            Cow::Borrowed("arc_replayed"),
            evaluation.replayed.to_string(),
        );
        if let Some(hash) = evaluation.chain_hash {
            ctx.transaction
                .insert(Cow::Borrowed("arc_chain_hash"), hash);
        }
        if let Some(error) = evaluation.error {
            ctx.transaction.insert(Cow::Borrowed("arc_error"), error);
        }

        if let Some(metrics) = metrics() {
            metrics.smtp.record_arc_verification(
                evaluation.result,
                evaluation.replayed,
                evaluation.chain_len,
            );
        }

        if evaluation.replayed {
            tracing::warn!(
                arc_result = %evaluation.result,
                arc_chain_len = evaluation.chain_len,
                arc_chain_hash = %ctx.transaction.get("arc_chain_hash").map_or("", String::as_str),
                "ARC replay detected"
            );
        }

        match evaluation.result {
            "fail" | "permerror" | "temperror" => {
                let error = ctx.transaction.get("arc_error").map(String::as_str);
                tracing::warn!(
                    arc_result = %evaluation.result,
                    arc_error = error.unwrap_or(""),
                    "ARC verification failed"
                );
            }
            "neutral" | "none" => {
                tracing::debug!(arc_result = %evaluation.result, "ARC verification skipped");
            }
            _ => {}
        }
    }

    async fn evaluate_dmarc(&self, ctx: &mut Context) {
        let Some(resolver) = &self.spf_dns_resolver else {
            return;
        };

        let from_domain = match ctx.headers_view() {
            Ok(Some(headers)) => dmarc::extract_from_domain(&headers),
            Ok(None) => {
                ctx.transaction
                    .insert(Cow::Borrowed("dmarc_result"), "none".to_string());
                ctx.transaction.insert(
                    Cow::Borrowed("dmarc_reason"),
                    "missing_message_data".to_string(),
                );
                return;
            }
            Err(err) => {
                ctx.transaction
                    .insert(Cow::Borrowed("dmarc_result"), "permerror".to_string());
                ctx.transaction
                    .insert(Cow::Borrowed("dmarc_error"), err.to_string());
                return;
            }
        };

        let dkim_result = normalize_dkim_result(ctx);
        if ctx.transaction.get("dkim_domain").is_none()
            && let Ok(Some(headers)) = ctx.headers_view()
            && let Some(info) = dmarc::extract_dkim_info(&headers)
        {
            ctx.transaction
                .insert(Cow::Borrowed("dkim_domain"), info.domain.clone());
            if let Some(selector) = info.selector {
                ctx.transaction
                    .insert(Cow::Borrowed("dkim_selector"), selector);
            }
            if dkim_result.is_none() {
                ctx.transaction
                    .insert(Cow::Borrowed("dkim_result"), "none".to_string());
            }
        }

        let Some(from_domain) = from_domain else {
            ctx.transaction
                .insert(Cow::Borrowed("dmarc_result"), "none".to_string());
            ctx.transaction
                .insert(Cow::Borrowed("dmarc_reason"), "missing_from".to_string());
            return;
        };

        let mail_from_domain = ctx
            .sender_email()
            .and_then(|sender| dmarc::domain_from_address(sender.as_str()));
        let spf_result = ctx.transaction.get("spf_result").map(String::as_str);
        let dkim_result = dkim_result
            .as_deref()
            .or_else(|| ctx.transaction.get("dkim_result").map(String::as_str));
        let dkim_domain = ctx.transaction.get("dkim_domain").map(String::as_str);

        let evaluation = dmarc::evaluate(
            &**resolver,
            &from_domain,
            mail_from_domain.as_deref(),
            spf_result,
            dkim_result,
            dkim_domain,
        )
        .await;

        Self::record_dmarc_evaluation(ctx, from_domain, &evaluation);
        self.sample_dmarc_failure(ctx, &evaluation);
    }

    fn record_dmarc_evaluation(
        ctx: &mut Context,
        from_domain: String,
        evaluation: &dmarc::DmarcEvaluation,
    ) {
        ctx.transaction
            .insert(Cow::Borrowed("dmarc_domain"), from_domain);
        ctx.transaction
            .insert(Cow::Borrowed("dmarc_result"), evaluation.result.to_string());
        if let Some(policy) = evaluation.policy.clone() {
            ctx.transaction
                .insert(Cow::Borrowed("dmarc_policy"), policy);
        }
        ctx.transaction.insert(
            Cow::Borrowed("dmarc_spf_aligned"),
            evaluation.spf_aligned.to_string(),
        );
        ctx.transaction.insert(
            Cow::Borrowed("dmarc_dkim_aligned"),
            evaluation.dkim_aligned.to_string(),
        );
        if let Some(error) = evaluation.error.clone() {
            ctx.transaction.insert(Cow::Borrowed("dmarc_error"), error);
        }

        if let Some(metrics) = metrics() {
            metrics.smtp.record_dmarc_evaluation(
                evaluation.result.as_str(),
                evaluation.policy.as_deref(),
                evaluation.spf_aligned,
                evaluation.dkim_aligned,
            );
        }

        Self::log_dmarc_evaluation(ctx, evaluation);
    }

    fn log_dmarc_evaluation(ctx: &Context, evaluation: &dmarc::DmarcEvaluation) {
        match evaluation.result {
            dmarc::DmarcResult::Fail => {
                tracing::warn!(
                    dmarc_domain = %ctx.transaction.get("dmarc_domain").map_or("unknown", String::as_str),
                    dmarc_policy = %ctx.transaction.get("dmarc_policy").map_or("none", String::as_str),
                    dmarc_spf_aligned = %evaluation.spf_aligned,
                    dmarc_dkim_aligned = %evaluation.dkim_aligned,
                    "DMARC evaluation failed"
                );
            }
            dmarc::DmarcResult::TempError | dmarc::DmarcResult::PermError => {
                tracing::warn!(
                    dmarc_domain = %ctx.transaction.get("dmarc_domain").map_or("unknown", String::as_str),
                    dmarc_error = %ctx.transaction.get("dmarc_error").map_or("", String::as_str),
                    "DMARC evaluation error"
                );
            }
            dmarc::DmarcResult::None => {
                tracing::debug!("DMARC evaluation skipped (no record)");
            }
            dmarc::DmarcResult::Pass => {}
        }
    }

    fn sample_dmarc_failure(&self, ctx: &mut Context, evaluation: &dmarc::DmarcEvaluation) {
        if evaluation.result != dmarc::DmarcResult::Fail {
            return;
        }

        let sender = ctx
            .sender_email()
            .map_or_else(|| "unknown".to_string(), |value| value.as_str().to_string());
        let client_ip = self.client_ip(ctx);
        let policy = ctx
            .transaction
            .get("dmarc_policy")
            .map_or_else(|| "none".to_string(), |value| value.as_str().to_string());
        let domain = ctx
            .transaction
            .get("dmarc_domain")
            .map_or_else(|| "unknown".to_string(), |value| value.as_str().to_string());

        let sample_key = format!(
            "{}|{}|{}|{}|{}",
            sender,
            client_ip,
            policy,
            domain,
            ctx.id()
        );

        if !should_sample_dmarc(&sample_key) {
            return;
        }

        ctx.transaction
            .insert(Cow::Borrowed("dmarc_sampled"), "true".to_string());
        ctx.transaction.insert(
            Cow::Borrowed("dmarc_sample_rate_bps"),
            DMARC_SAMPLE_RATE_BPS.to_string(),
        );

        if let Some(metrics) = metrics() {
            metrics
                .smtp
                .record_dmarc_sample(&sender, client_ip, &policy);
        }

        tracing::info!(
            dmarc_sender = %sender,
            dmarc_client_ip = %client_ip,
            dmarc_policy = %policy,
            dmarc_domain = %domain,
            "Sampled DMARC failure"
        );
    }
}

fn normalize_dkim_result(ctx: &mut Context) -> Option<String> {
    if let Some(result) = ctx.transaction.get("dkim_result") {
        return Some(result.clone());
    }

    let verified = ctx.transaction.get("dkim_verified")?;

    let normalized =
        if verified.eq_ignore_ascii_case("true") || verified.eq_ignore_ascii_case("pass") {
            "pass"
        } else if verified.eq_ignore_ascii_case("false") || verified.eq_ignore_ascii_case("fail") {
            "fail"
        } else {
            return None;
        };

    let normalized = normalized.to_string();
    ctx.transaction
        .insert(Cow::Borrowed("dkim_result"), normalized.clone());
    Some(normalized)
}

fn dkim_is_pass(ctx: &Context) -> bool {
    ctx.transaction
        .get("dkim_result")
        .is_some_and(|value| value.eq_ignore_ascii_case("pass"))
}

const fn dmarc_enforcement_action(result: &str, policy: Option<&str>) -> Option<&'static str> {
    if !result.eq_ignore_ascii_case("fail") {
        return None;
    }

    match policy {
        Some(policy) if policy.eq_ignore_ascii_case("reject") => Some("reject"),
        Some(policy) if policy.eq_ignore_ascii_case("quarantine") => Some("quarantine"),
        _ => None,
    }
}

const DMARC_SAMPLE_DENOMINATOR: u32 = 10_000;
const DMARC_SAMPLE_RATE_BPS: u32 = 100;

fn should_sample_dmarc(key: &str) -> bool {
    let mut hasher = Sha256::new();
    hasher.update(key.as_bytes());
    let digest = hasher.finalize();
    let bucket = u32::from_be_bytes([digest[0], digest[1], digest[2], digest[3]]);
    bucket % DMARC_SAMPLE_DENOMINATOR < DMARC_SAMPLE_RATE_BPS
}

/// Helper function to determine if state requires validation.
///
/// This is used by the session orchestrator to decide whether to call
/// the business handler after an FSM transition.
///
/// # Arguments
///
/// * `state` - The current protocol state
///
/// # Returns
///
/// `true` if the state requires business logic validation
#[must_use]
pub const fn requires_validation(state: &State) -> bool {
    matches!(
        state,
        State::Connect(_)
            | State::Ehlo(_)
            | State::Helo(_)
            | State::Auth(_)
            | State::MailFrom(_)
            | State::RcptTo(_)
            | State::PostDot(_)
    )
}

#[cfg(test)]
mod tests {
    use empath_common::address::{Address, AddressList};
    use empath_common::address_parser;

    use super::*;
    use crate::MailParameters;

    type TestResult = std::result::Result<(), String>;

    #[test]
    fn test_default_handler_creation() -> TestResult {
        let audit_logger = AuditLoggerBuilder::default().build();
        let handler = DefaultSmtpTransactionHandler::new(
            None,
            "127.0.0.1:1234"
                .parse::<std::net::SocketAddr>()
                .map_err(|err| err.to_string())?,
            None,
            crate::spf::SpfConfig::default(),
            false,
            false,
            audit_logger,
        );
        assert!(handler.spool.is_none());
        assert_eq!(handler.peer.to_string(), "127.0.0.1:1234");
        Ok(())
    }

    #[test]
    fn test_default_handler_with_spool() -> TestResult {
        use empath_spool::MemoryBackingStore;

        let spool = Arc::new(MemoryBackingStore::default());
        let audit_logger = AuditLoggerBuilder::default().build();
        let handler = DefaultSmtpTransactionHandler::new(
            Some(spool),
            "127.0.0.1:1234"
                .parse::<std::net::SocketAddr>()
                .map_err(|err| err.to_string())?,
            None,
            crate::spf::SpfConfig::default(),
            false,
            false,
            audit_logger,
        );
        assert!(handler.spool.is_some());
        Ok(())
    }

    #[test]
    fn test_requires_validation() {
        use crate::state::*;

        // States that require validation
        assert!(requires_validation(&State::Connect(Connect)));
        assert!(requires_validation(&State::Ehlo(Ehlo {
            id: "test".to_string()
        })));
        assert!(requires_validation(&State::Helo(Helo {
            id: "test".to_string()
        })));
        assert!(requires_validation(&State::MailFrom(MailFrom {
            sender: None,
            params: MailParameters::default()
        })));
        assert!(requires_validation(&State::Auth(Auth {
            mechanism: "PLAIN".to_string(),
            awaiting_response: false,
        })));

        // States that don't require validation
        assert!(!requires_validation(&State::Data(Data)));
        assert!(!requires_validation(&State::Quit(Quit)));
        assert!(!requires_validation(&State::Invalid(Invalid {
            reason: String::new()
        })));
    }

    #[test]
    fn test_dmarc_enforcement_action() {
        assert_eq!(
            dmarc_enforcement_action("fail", Some("reject")),
            Some("reject")
        );
        assert_eq!(
            dmarc_enforcement_action("fail", Some("quarantine")),
            Some("quarantine")
        );
        assert_eq!(dmarc_enforcement_action("fail", Some("none")), None);
        assert_eq!(dmarc_enforcement_action("pass", Some("reject")), None);
        assert_eq!(dmarc_enforcement_action("temperror", Some("reject")), None);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn received_header_is_inserted_before_body() -> TestResult {
        let audit_logger = AuditLoggerBuilder::default().build();
        let handler = DefaultSmtpTransactionHandler::new(
            None,
            "203.0.113.5:2525"
                .parse::<std::net::SocketAddr>()
                .map_err(|err| err.to_string())?,
            None,
            crate::spf::SpfConfig::default(),
            false,
            false,
            audit_logger,
        );

        let message = b"Subject: hello\r\n\r\nhello world";
        let mut ctx = Context {
            data: Some(Arc::from(message.as_ref())),
            id: "client.example.com".to_string(),
            extended: true,
            banner: Arc::from("mail.example.com"),
            ..Context::default()
        };

        let recipient_mailbox = address_parser::parse_forward_path("<recipient@example.com>")
            .map_err(|err| format!("{err:?}"))?;
        let recipient_address = Address::from(recipient_mailbox);
        Arc::make_mut(&mut ctx.envelope)
            .recipients_mut()
            .replace(AddressList::from(vec![recipient_address]));

        ctx.session.insert(Cow::Borrowed("tls"), "true".to_string());

        handler.insert_received_header(&mut ctx);

        let data = ctx
            .data
            .as_ref()
            .ok_or_else(|| "message data missing".to_string())?;
        let header_end = data
            .windows(2)
            .position(|window| window == b"\r\n")
            .ok_or_else(|| "missing CRLF".to_string())?;
        let first_line = std::str::from_utf8(&data[..header_end]).map_err(|err| err.to_string())?;

        assert!(first_line.starts_with("Received: from client.example.com"));
        assert!(first_line.contains(" by mail.example.com (Empath)"));
        assert!(first_line.contains(" with ESMTPS id <"));
        assert!(first_line.contains(" for <recipient@example.com>"));
        Ok(())
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)]
    async fn test_spf_evaluation_integration() -> TestResult {
        use std::net::IpAddr;
        use std::sync::Mutex;

        use empath_common::address_parser::Mailbox;
        use empath_common::envelope::Envelope;

        use crate::spf::dns::{DnsLookupError, SpfDnsResolver};
        use crate::spf::{MailServer, SpfConfig, SpfResult};

        // Mock DNS resolver that returns SPF records
        #[derive(Debug)]
        struct MockSpfResolver {
            txt_records: Mutex<ahash::AHashMap<String, Vec<String>>>,
        }

        impl MockSpfResolver {
            fn new() -> Self {
                Self {
                    txt_records: Mutex::new(ahash::AHashMap::new()),
                }
            }

            fn add_txt(&self, domain: &str, records: Vec<String>) -> Result<(), DnsLookupError> {
                self.txt_records
                    .lock()
                    .map_err(|_| DnsLookupError::LookupFailed("lock poisoned".to_string()))?
                    .insert(domain.to_string(), records);
                Ok(())
            }
        }

        #[async_trait::async_trait]
        impl SpfDnsResolver for MockSpfResolver {
            async fn txt_lookup(
                &self,
                domain: &str,
            ) -> Result<Arc<Vec<String>>, crate::spf::dns::DnsLookupError> {
                let records = self
                    .txt_records
                    .lock()
                    .map_err(|_| DnsLookupError::LookupFailed("lock poisoned".to_string()))?;
                Ok(Arc::new(records.get(domain).cloned().unwrap_or_default()))
            }

            async fn ptr_lookup(
                &self,
                _ip: IpAddr,
            ) -> Result<Arc<Vec<String>>, crate::spf::dns::DnsLookupError> {
                Ok(Arc::new(Vec::new()))
            }

            async fn a_lookup(
                &self,
                _domain: &str,
            ) -> Result<Arc<Vec<IpAddr>>, crate::spf::dns::DnsLookupError> {
                let ip = "192.0.2.100"
                    .parse::<IpAddr>()
                    .map_err(|err| DnsLookupError::LookupFailed(err.to_string()))?;
                Ok(Arc::new(vec![ip]))
            }

            async fn mx_lookup(
                &self,
                _domain: &str,
            ) -> Result<Arc<Vec<MailServer>>, crate::spf::dns::DnsLookupError> {
                Ok(Arc::new(Vec::new()))
            }
        }

        // Setup mock resolver with SPF record
        let resolver = Arc::new(MockSpfResolver::new());
        resolver
            .add_txt(
                "example.com",
                vec!["v=spf1 ip4:192.0.2.0/24 -all".to_string()],
            )
            .map_err(|err| err.to_string())?;

        // Create transaction handler with SPF resolver
        let audit_logger = AuditLoggerBuilder::default().build();
        let mut handler = DefaultSmtpTransactionHandler::new(
            None,
            "192.0.2.100:1234"
                .parse::<std::net::SocketAddr>()
                .map_err(|err| err.to_string())?,
            Some(resolver),
            SpfConfig::default(),
            false,
            false,
            audit_logger,
        );

        // Create context with sender from example.com
        let mut envelope = Envelope::default();
        let mailbox = Mailbox {
            local_part: "user".to_string(),
            domain: "example.com".to_string(),
        };
        *envelope.sender_mut() = Some(mailbox.into());

        let mut ctx = Context {
            extended: true,
            envelope: Arc::new(envelope),
            id: "client.example.com".to_string(),
            banner: Arc::from("test.example.com"),
            ..Context::default()
        };

        // Call validate_mail_from which should perform SPF evaluation
        let _result = handler.validate_mail_from(&mut ctx).await;

        // Check that SPF result was stored in transaction context
        // (Note: module dispatch may return a non-accept result when no modules loaded,
        // but SPF evaluation should still have occurred)
        assert!(ctx.transaction.contains_key("spf_result"));
        let spf_result = ctx
            .transaction
            .get("spf_result")
            .ok_or_else(|| "spf_result missing".to_string())?;
        assert_eq!(spf_result, SpfResult::Pass.to_string().as_str());
        Ok(())
    }
}
