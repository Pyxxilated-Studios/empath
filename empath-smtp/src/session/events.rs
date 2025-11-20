use empath_common::{context, tracing};
use empath_tracing::traced;
use tokio::io::{AsyncRead, AsyncWrite};

use crate::{State, session::Session, state, transaction_handler::SmtpTransactionHandler};

impl<Stream: AsyncRead + AsyncWrite + Unpin + Send + Sync> Session<Stream> {
    /// Handle validation and work for each state using SMTP transaction handler
    ///
    /// This delegates to the `SmtpTransactionHandler` trait which provides separation
    /// between protocol concerns (FSM) and business concerns (validation, spooling).
    ///
    /// Flow:
    /// 1. `SmtpTransactionHandler` dispatches to modules for validation
    /// 2. If validation passes, `SmtpTransactionHandler` performs work (spooling, audit)
    /// 3. State transitions happen separately in FSM layer
    #[traced(instrument(level = tracing::Level::TRACE, skip_all), timing)]
    pub(super) async fn emit(&mut self, validate_context: &mut context::Context) {
        let valid = match self.context.state {
            State::Connect(_) => {
                self.transaction_handler
                    .validate_connect(validate_context)
                    .await
            }
            State::Helo(_) | State::Ehlo(_) => {
                self.transaction_handler
                    .validate_ehlo(validate_context)
                    .await
            }
            State::MailFrom(_) => {
                self.transaction_handler
                    .validate_mail_from(validate_context)
                    .await
            }
            State::RcptTo(_) => {
                self.transaction_handler
                    .validate_rcpt_to(validate_context)
                    .await
            }
            State::PostDot(_) => {
                self.transaction_handler
                    .handle_message(validate_context)
                    .await
            }
            _ => return, // No validation needed for other states
        };

        // Update session state based on validation result
        if !valid {
            match self.context.state {
                // Only reject on critical failures (Connect, EHLO, RCPT TO)
                // MAIL FROM failures don't reject - they just return error
                State::Connect(_) | State::Ehlo(_) | State::Helo(_) | State::RcptTo(_) => {
                    self.context.state = State::Reject(state::Reject);
                }
                _ => {
                    // For other states, let the response speak for itself
                }
            }
        }
    }
}
