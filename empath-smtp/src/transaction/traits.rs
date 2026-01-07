//! SMTP transaction handler trait definition

use async_trait::async_trait;
use empath_common::context::Context;

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
