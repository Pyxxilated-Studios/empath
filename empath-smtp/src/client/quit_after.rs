//! Controls at which point the SMTP client should terminate the session.
//!
//! Similar to the `--quit-after` flag in the `swaks` tool, this allows testing
//! SMTP servers by stopping at various points in the protocol conversation.
//!
//! When using `quit_after` with `execute()`, the client will:
//! 1. Execute commands up to and including the specified point
//! 2. Send QUIT immediately after
//! 3. Ignore any additional commands in the builder
//!
//! This matches swaks behavior where `--quit-after MAIL` sends MAIL FROM,
//! then QUIT, even if RCPT TO and DATA are specified.

/// Determines when the SMTP client should quit the session.
///
/// This is useful for integration testing to verify server behavior at different
/// stages of the SMTP conversation.
///
/// # Swaks Compatibility
///
/// This enum matches the behavior of `swaks --quit-after`:
/// - Executes all commands up to and including the specified point
/// - Sends QUIT immediately after that point (when using `execute()`)
/// - Does not execute any commands added after the quit point
///
/// # Example
///
/// ```no_run
/// # use empath_smtp::client::{SmtpClientBuilder, QuitAfter};
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// // This will send: EHLO, MAIL FROM, QUIT
/// // (RCPT TO and DATA are never sent)
/// let responses = SmtpClientBuilder::new("localhost:2525", "localhost")
///     .ehlo("client.example.com")
///     .mail_from("sender@example.com")
///     .rcpt_to("recipient@example.com")  // Not executed
///     .data_with_content("test")          // Not executed
///     .quit_after(QuitAfter::MailFrom)
///     .execute()
///     .await?;
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum QuitAfter {
    /// Receive server greeting (220), then send QUIT immediately.
    Connect,

    /// Send HELO/EHLO, receive response (250), then send QUIT.
    Greeting,

    /// Send MAIL FROM, receive response (250), then send QUIT.
    MailFrom,

    /// Send RCPT TO, receive response (250), then send QUIT.
    RcptTo,

    /// Send message data and final dot, receive response (250), then send QUIT.
    /// This is equivalent to a complete SMTP transaction.
    DataEnd,

    /// Complete all specified commands, then send QUIT at the end.
    #[default]
    Never,
}

impl QuitAfter {
    /// Returns `true` if the client should quit after the connect phase.
    #[must_use]
    pub const fn should_quit_after_connect(self) -> bool {
        matches!(self, Self::Connect)
    }

    /// Returns `true` if the client should quit after the greeting phase.
    #[must_use]
    pub const fn should_quit_after_greeting(self) -> bool {
        matches!(self, Self::Greeting)
    }

    /// Returns `true` if the client should quit after MAIL FROM.
    #[must_use]
    pub const fn should_quit_after_mail_from(self) -> bool {
        matches!(self, Self::MailFrom)
    }

    /// Returns `true` if the client should quit after RCPT TO.
    #[must_use]
    pub const fn should_quit_after_rcpt_to(self) -> bool {
        matches!(self, Self::RcptTo)
    }

    /// Returns `true` if the client should quit after sending message data.
    #[must_use]
    pub const fn should_quit_after_data_end(self) -> bool {
        matches!(self, Self::DataEnd)
    }
}
