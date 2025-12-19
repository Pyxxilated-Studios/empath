//! Business logic handler for SMTP sessions.
//!
//! This module provides the `BusinessHandler` trait that separates business
//! logic (validation, spooling, module dispatch) from protocol state management
//! (FSM) and I/O orchestration.

use std::{borrow::Cow, net::SocketAddr, sync::Arc};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use empath_common::{context::Context, status::Status};
use empath_ffi::modules;
use empath_spool::BackingStore;
use ulid::Ulid;

use crate::State;

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
/// use empath_smtp::{transaction_handler::DefaultSmtpTransactionHandler, spf::SpfConfig};
/// use empath_spool::BackingStore;
///
/// # fn example(spool: Arc<dyn BackingStore>, peer: std::net::SocketAddr) {
/// let handler = DefaultSmtpTransactionHandler::new(Some(spool), peer, None, SpfConfig::default());
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
    #[must_use]
    pub const fn new(
        spool: Option<Arc<dyn BackingStore>>,
        peer: SocketAddr,
        spf_dns_resolver: Option<Arc<dyn crate::spf::SpfDnsResolver>>,
        spf_config: crate::spf::SpfConfig,
    ) -> Self {
        Self {
            spool,
            peer,
            spf_dns_resolver,
            spf_config,
        }
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
            let from_ip = self.peer.to_string();

            empath_common::audit::log_message_received(
                &id.to_string(),
                sender,
                &recipients,
                size,
                &from_ip,
            );
        }

        // Dispatch message received event (fail open).
        let _ = modules::dispatch(modules::Event::Event(modules::Ev::SmtpMessageReceived), ctx);
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
        let client_ip = self.peer.ip();
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
        let _ = modules::dispatch(modules::Event::Event(modules::Ev::ConnectionOpened), ctx);

        // Then validate
        matches!(
            modules::dispatch(
                modules::Event::Validate(modules::validate::Event::Connect),
                ctx,
            ),
            modules::DispatchResult::Accept
        )
    }

    async fn validate_ehlo(&mut self, ctx: &mut Context) -> bool {
        matches!(
            modules::dispatch(
                modules::Event::Validate(modules::validate::Event::Ehlo),
                ctx,
            ),
            modules::DispatchResult::Accept
        )
    }

    async fn validate_mail_from(&mut self, ctx: &mut Context) -> bool {
        // Perform SPF evaluation if DNS resolver is available
        if let Some(resolver) = &self.spf_dns_resolver
            && let Some(sender) = ctx.sender_email().cloned()
        {
            // Extract IP address from peer socket address
            let client_ip = self.peer.ip();

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
            modules::dispatch(
                modules::Event::Validate(modules::validate::Event::MailFrom),
                ctx,
            ),
            modules::DispatchResult::Accept
        )
    }

    async fn validate_rcpt_to(&mut self, ctx: &mut Context) -> bool {
        matches!(
            modules::dispatch(
                modules::Event::Validate(modules::validate::Event::RcptTo),
                ctx,
            ),
            modules::DispatchResult::Accept
        )
    }

    async fn handle_message(&mut self, ctx: &mut Context) -> bool {
        // Dispatch validation
        let valid = matches!(
            modules::dispatch(
                modules::Event::Validate(modules::validate::Event::Data),
                ctx,
            ),
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
            | State::MailFrom(_)
            | State::RcptTo(_)
            | State::PostDot(_)
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use empath_common::{
        address::{Address, AddressList},
        address_parser,
    };

    use super::*;
    use crate::MailParameters;

    #[test]
    fn test_default_handler_creation() {
        let handler = DefaultSmtpTransactionHandler::new(
            None,
            "127.0.0.1:1234".parse().unwrap(),
            None,
            crate::spf::SpfConfig::default(),
        );
        assert!(handler.spool.is_none());
        assert_eq!(handler.peer.to_string(), "127.0.0.1:1234");
    }

    #[test]
    fn test_default_handler_with_spool() {
        use empath_spool::MemoryBackingStore;

        let spool = Arc::new(MemoryBackingStore::default());
        let handler = DefaultSmtpTransactionHandler::new(
            Some(spool),
            "127.0.0.1:1234".parse().unwrap(),
            None,
            crate::spf::SpfConfig::default(),
        );
        assert!(handler.spool.is_some());
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

        // States that don't require validation
        assert!(!requires_validation(&State::Data(Data)));
        assert!(!requires_validation(&State::Quit(Quit)));
        assert!(!requires_validation(&State::Invalid(Invalid {
            reason: String::new()
        })));
    }

    #[test]
    fn received_header_is_inserted_before_body() {
        let handler = DefaultSmtpTransactionHandler::new(
            None,
            "203.0.113.5:2525".parse().unwrap(),
            None,
            crate::spf::SpfConfig::default(),
        );

        let message = b"Subject: hello\r\n\r\nhello world";
        let mut ctx = Context {
            data: Some(Arc::from(message.as_ref())),
            id: "client.example.com".to_string(),
            extended: true,
            banner: Arc::from("mail.example.com"),
            ..Context::default()
        };

        let recipient_mailbox =
            address_parser::parse_forward_path("<recipient@example.com>").unwrap();
        let recipient_address = Address::from(recipient_mailbox);
        Arc::make_mut(&mut ctx.envelope)
            .recipients_mut()
            .replace(AddressList::from(vec![recipient_address]));

        ctx.session.insert(Cow::Borrowed("tls"), "true".to_string());

        handler.insert_received_header(&mut ctx);

        let data = ctx.data.as_ref().expect("message data");
        let header_end = data
            .windows(2)
            .position(|window| window == b"\r\n")
            .expect("missing CRLF");
        let first_line = std::str::from_utf8(&data[..header_end]).unwrap();

        assert!(first_line.starts_with("Received: from client.example.com"));
        assert!(first_line.contains(" by mail.example.com (Empath)"));
        assert!(first_line.contains(" with ESMTPS id <"));
        assert!(first_line.contains(" for <recipient@example.com>"));
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)]
    async fn test_spf_evaluation_integration() {
        use std::{net::IpAddr, sync::Mutex};

        use empath_common::{address_parser::Mailbox, envelope::Envelope};

        use crate::spf::{MailServer, SpfConfig, SpfResult, dns::SpfDnsResolver};

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

            fn add_txt(&self, domain: &str, records: Vec<String>) {
                self.txt_records
                    .lock()
                    .unwrap()
                    .insert(domain.to_string(), records);
            }
        }

        #[async_trait::async_trait]
        impl SpfDnsResolver for MockSpfResolver {
            async fn txt_lookup(
                &self,
                domain: &str,
            ) -> Result<Arc<Vec<String>>, crate::spf::dns::DnsLookupError> {
                Ok(Arc::new(
                    self.txt_records
                        .lock()
                        .unwrap()
                        .get(domain)
                        .cloned()
                        .unwrap_or_default(),
                ))
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
                Ok(Arc::new(vec!["192.0.2.100".parse().unwrap()]))
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
        resolver.add_txt(
            "example.com",
            vec!["v=spf1 ip4:192.0.2.0/24 -all".to_string()],
        );

        // Create transaction handler with SPF resolver
        let mut handler = DefaultSmtpTransactionHandler::new(
            None,
            "192.0.2.100:1234".parse().unwrap(),
            Some(resolver),
            SpfConfig::default(),
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
        // (Note: modules::dispatch may return a non-accept result when no modules loaded,
        // but SPF evaluation should still have occurred)
        assert!(ctx.transaction.contains_key("spf_result"));
        let spf_result = ctx.transaction.get("spf_result").unwrap();
        assert_eq!(spf_result, SpfResult::Pass.to_string().as_str());
    }
}
