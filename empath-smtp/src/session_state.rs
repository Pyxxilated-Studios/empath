//! SMTP session state for finite state machine.
//!
//! This module defines the `SessionState` struct which contains only
//! the protocol state needed by the SMTP FSM for pure state transitions.
//! This is separate from the business context (validation, metadata, etc.)
//! to enable clean separation of concerns.

use empath_common::{context::Context, envelope::Envelope};
use serde::{Deserialize, Serialize};

/// SMTP session state for the finite state machine.
///
/// This struct contains only the protocol state required for FSM transitions:
/// - Client identifier (EHLO/HELO)
/// - ESMTP mode flag
/// - Mail envelope (sender, recipients, parameters)
///
/// This is intentionally separate from business logic context (validation
/// results, metadata, capabilities, etc.) to enable pure FSM transitions
/// without side effects.
///
/// # Design Rationale
///
/// By separating protocol state from business state, we achieve:
/// - Pure state transitions (no hidden side effects)
/// - Testable FSM in isolation
/// - Clear separation of concerns
/// - Proper implementation of the `FiniteStateMachine` trait
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionState {
    /// Client identifier from EHLO/HELO command
    pub id: String,

    /// Extended SMTP mode (ESMTP)
    ///
    /// `true` if client sent EHLO (extended SMTP), `false` for HELO (basic SMTP)
    pub extended: bool,

    /// Mail envelope containing transaction state
    ///
    /// Includes:
    /// - Sender address (MAIL FROM)
    /// - Recipient addresses (RCPT TO)
    /// - MAIL FROM parameters (SIZE, BODY, etc.)
    /// - RCPT TO parameters (NOTIFY, ORCPT, etc.)
    pub envelope: Envelope,
}

impl SessionState {
    /// Creates a new session state with default values
    #[must_use]
    pub fn new() -> Self {
        Self {
            id: String::new(),
            extended: false,
            envelope: Envelope::default(),
        }
    }

    /// Creates a session state with a specific client ID and ESMTP mode
    #[must_use]
    pub fn with_id(id: String, extended: bool) -> Self {
        Self {
            id,
            extended,
            envelope: Envelope::default(),
        }
    }

    /// Resets the mail transaction state while preserving session state
    ///
    /// Clears the envelope (sender, recipients, parameters) and any
    /// transaction-specific metadata, but keeps the client ID and
    /// ESMTP mode flag. Used when handling RSET command.
    pub fn reset_transaction(&mut self) {
        self.envelope = Envelope::default();
    }

    /// Returns a reference to the client identifier
    #[inline]
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Returns whether extended SMTP (ESMTP) is enabled
    #[inline]
    #[must_use]
    pub const fn is_extended(&self) -> bool {
        self.extended
    }

    /// Returns a reference to the mail envelope
    #[inline]
    #[must_use]
    pub const fn envelope(&self) -> &Envelope {
        &self.envelope
    }

    /// Returns a mutable reference to the mail envelope
    #[inline]
    pub const fn envelope_mut(&mut self) -> &mut Envelope {
        &mut self.envelope
    }

    /// Extracts session state from a business context
    ///
    /// Creates a `SessionState` by copying protocol-level fields from a
    /// business `Context`. This enables FSM transitions to work with pure
    /// protocol state while the business logic maintains the full context.
    #[must_use]
    pub fn from_context(ctx: &Context) -> Self {
        Self {
            id: ctx.id.clone(),
            extended: ctx.extended,
            envelope: ctx.envelope.clone(),
        }
    }

    /// Synchronizes session state back to business context
    ///
    /// Updates the protocol-level fields in a business `Context` from this
    /// `SessionState`. This is used after FSM transitions to ensure the
    /// business context reflects the new protocol state.
    pub fn sync_to_context(&self, ctx: &mut Context) {
        ctx.id.clone_from(&self.id);
        ctx.extended = self.extended;
        ctx.envelope.clone_from(&self.envelope);
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use empath_common::{address::Address, address_parser};

    use super::*;

    #[test]
    fn test_new_session_state() {
        let ctx = SessionState::new();
        assert_eq!(ctx.id(), "");
        assert!(!ctx.is_extended());
    }

    #[test]
    fn test_with_id() {
        let ctx = SessionState::with_id("client.example.com".to_string(), true);
        assert_eq!(ctx.id(), "client.example.com");
        assert!(ctx.is_extended());
    }

    #[test]
    fn test_reset_transaction() {
        let mut ctx = SessionState::with_id("client.example.com".to_string(), true);

        // Set up a mail transaction
        let sender_mailbox = address_parser::parse_forward_path("<sender@example.com>").unwrap();
        *ctx.envelope_mut().sender_mut() = Some(Address::from(sender_mailbox));

        // Reset should clear transaction but keep session state
        ctx.reset_transaction();

        assert_eq!(ctx.id(), "client.example.com");
        assert!(ctx.is_extended());
        assert!(ctx.envelope().sender().is_none());
    }

    #[test]
    fn test_envelope_access() {
        let mut ctx = SessionState::new();

        let mailbox = address_parser::parse_forward_path("<test@example.com>").unwrap();
        *ctx.envelope_mut().sender_mut() = Some(Address::from(mailbox));
        assert!(ctx.envelope().sender().is_some());
    }

    #[test]
    fn test_from_context() {
        let mut business_ctx = Context {
            id: "mail.example.com".to_string(),
            extended: true,
            ..Default::default()
        };

        let sender_mailbox = address_parser::parse_forward_path("<sender@example.com>").unwrap();
        *business_ctx.envelope.sender_mut() = Some(Address::from(sender_mailbox));

        let session_state = SessionState::from_context(&business_ctx);

        assert_eq!(session_state.id(), "mail.example.com");
        assert!(session_state.is_extended());
        assert!(session_state.envelope().sender().is_some());
    }

    #[test]
    fn test_sync_to_context() {
        let mut session_state = SessionState::with_id("client.example.com".to_string(), true);

        let sender_mailbox = address_parser::parse_forward_path("<sender@example.com>").unwrap();
        *session_state.envelope_mut().sender_mut() = Some(Address::from(sender_mailbox));

        let mut business_ctx = Context::default();
        session_state.sync_to_context(&mut business_ctx);

        assert_eq!(business_ctx.id, "client.example.com");
        assert!(business_ctx.extended);
        assert!(business_ctx.envelope.sender().is_some());
    }

    #[test]
    fn test_roundtrip_conversion() {
        let mut original_ctx = Context {
            extended: true,
            id: "test.example.com".to_string(),
            ..Default::default()
        };

        let sender_mailbox = address_parser::parse_forward_path("<sender@example.com>").unwrap();
        *original_ctx.envelope.sender_mut() = Some(Address::from(sender_mailbox));

        // Convert to session state
        let session_state = SessionState::from_context(&original_ctx);

        // Sync back to a new business context
        let mut new_ctx = Context::default();
        session_state.sync_to_context(&mut new_ctx);

        // Verify protocol fields match
        assert_eq!(new_ctx.id, original_ctx.id);
        assert_eq!(new_ctx.extended, original_ctx.extended);
        assert!(new_ctx.envelope.sender().is_some());
    }
}
