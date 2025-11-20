//! Business logic handler for SMTP sessions.
//!
//! This module provides the `BusinessHandler` trait that separates business
//! logic (validation, spooling, module dispatch) from protocol state management
//! (FSM) and I/O orchestration.

use std::{borrow::Cow, net::SocketAddr, sync::Arc};

use async_trait::async_trait;
use empath_common::{context::Context, status::Status};
use empath_ffi::modules;
use empath_spool::BackingStore;

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
/// use empath_smtp::transaction_handler::DefaultSmtpTransactionHandler;
/// use empath_spool::BackingStore;
///
/// # fn example(spool: Arc<dyn BackingStore>, peer: std::net::SocketAddr) {
/// let handler = DefaultSmtpTransactionHandler::new(Some(spool), peer);
/// // Use handler with session orchestrator
/// # }
/// ```
pub struct DefaultSmtpTransactionHandler {
    /// Optional spool for message persistence
    spool: Option<Arc<dyn BackingStore>>,
    /// Client peer address for audit logging
    peer: SocketAddr,
}

impl DefaultSmtpTransactionHandler {
    /// Creates a new default SMTP transaction handler.
    ///
    /// # Arguments
    ///
    /// * `spool` - Optional message spool for persistence
    /// * `peer` - Client peer address for audit logging
    #[must_use]
    pub const fn new(spool: Option<Arc<dyn BackingStore>>, peer: SocketAddr) -> Self {
        Self { spool, peer }
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
        ctx.metadata.remove("declared_size");

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
            let sender = ctx.sender();
            let recipients = ctx.recipients();
            let size = ctx.data.as_ref().map_or(0, |d| d.len());
            let from_ip = self.peer.to_string();

            empath_common::audit::log_message_received(
                &id.to_string(),
                &sender,
                &recipients,
                size,
                &from_ip,
            );
        }

        // Dispatch message received event
        modules::dispatch(modules::Event::Event(modules::Ev::SmtpMessageReceived), ctx);
    }
}

#[async_trait]
impl SmtpTransactionHandler for DefaultSmtpTransactionHandler {
    async fn validate_connect(&mut self, ctx: &mut Context) -> bool {
        // Dispatch connection opened event first
        modules::dispatch(modules::Event::Event(modules::Ev::ConnectionOpened), ctx);

        // Then validate
        modules::dispatch(
            modules::Event::Validate(modules::validate::Event::Connect),
            ctx,
        )
    }

    async fn validate_ehlo(&mut self, ctx: &mut Context) -> bool {
        modules::dispatch(
            modules::Event::Validate(modules::validate::Event::Ehlo),
            ctx,
        )
    }

    async fn validate_mail_from(&mut self, ctx: &mut Context) -> bool {
        modules::dispatch(
            modules::Event::Validate(modules::validate::Event::MailFrom),
            ctx,
        )
    }

    async fn validate_rcpt_to(&mut self, ctx: &mut Context) -> bool {
        modules::dispatch(
            modules::Event::Validate(modules::validate::Event::RcptTo),
            ctx,
        )
    }

    async fn handle_message(&mut self, ctx: &mut Context) -> bool {
        // Dispatch validation
        let valid = modules::dispatch(
            modules::Event::Validate(modules::validate::Event::Data),
            ctx,
        );

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
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::MailParameters;

    #[test]
    fn test_default_handler_creation() {
        let handler = DefaultSmtpTransactionHandler::new(None, "127.0.0.1:1234".parse().unwrap());
        assert!(handler.spool.is_none());
        assert_eq!(handler.peer.to_string(), "127.0.0.1:1234");
    }

    #[test]
    fn test_default_handler_with_spool() {
        use empath_spool::MemoryBackingStore;

        let spool = Arc::new(MemoryBackingStore::default());
        let handler =
            DefaultSmtpTransactionHandler::new(Some(spool), "127.0.0.1:1234".parse().unwrap());
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
}
