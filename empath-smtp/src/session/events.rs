use std::borrow::Cow;

use empath_common::{context, internal, status::Status, tracing};
use empath_ffi::modules;
use empath_tracing::traced;
use tokio::io::{AsyncRead, AsyncWrite};

use crate::{State, session::Session, state};

impl<Stream: AsyncRead + AsyncWrite + Unpin + Send + Sync> Session<Stream> {
    /// Handle validation and work for each state
    ///
    /// Flow:
    /// 1. Dispatch to core module first (sets default responses, validation)
    /// 2. Then dispatch to user modules (can override responses, reject)
    /// 3. If validation passed, do the work (spooling)
    #[traced(instrument(level = tracing::Level::TRACE, skip_all), timing)]
    pub(super) async fn emit(&mut self, validate_context: &mut context::Context) {
        match self.context.state {
            State::Connect(_) => self.handle_connect(validate_context),
            State::Helo(_) | State::Ehlo(_) => self.handle_ehlo(validate_context),
            State::MailFrom(_) => Self::handle_mail_from(validate_context),
            State::RcptTo(_) => self.handle_rcpt_to(validate_context),
            State::PostDot(_) => self.handle_post_dot(validate_context).await,
            _ => {}
        }
    }

    /// Handle Connect state event
    fn handle_connect(&mut self, validate_context: &mut context::Context) {
        modules::dispatch(
            modules::Event::Event(modules::Ev::ConnectionOpened),
            validate_context,
        );

        if !modules::dispatch(
            modules::Event::Validate(modules::validate::Event::Connect),
            validate_context,
        ) {
            self.context.state = State::Reject(state::Reject);
        }
    }

    /// Handle HELO/EHLO state event
    fn handle_ehlo(&mut self, validate_context: &mut context::Context) {
        if !modules::dispatch(
            modules::Event::Validate(modules::validate::Event::Ehlo),
            validate_context,
        ) {
            self.context.state = State::Reject(state::Reject);
        }
    }

    /// Handle MAIL FROM state event
    fn handle_mail_from(validate_context: &mut context::Context) {
        if !modules::dispatch(
            modules::Event::Validate(modules::validate::Event::MailFrom),
            validate_context,
        ) {
            // Don't change state for validation failures like SIZE - just return error
        }
    }

    /// Handle RCPT TO state event
    fn handle_rcpt_to(&mut self, validate_context: &mut context::Context) {
        if !modules::dispatch(
            modules::Event::Validate(modules::validate::Event::RcptTo),
            validate_context,
        ) {
            self.context.state = State::Reject(state::Reject);
        }
    }

    /// Handle `PostDot` state event (message complete)
    async fn handle_post_dot(&self, validate_context: &mut context::Context) {
        // Dispatch validation
        let valid = modules::dispatch(
            modules::Event::Validate(modules::validate::Event::Data),
            validate_context,
        );

        // If validation passed, do the work (spooling)
        if valid {
            // Check if any module set a rejection response
            // Positive responses are < 400 (2xx and 3xx codes)
            let should_spool = validate_context
                .response
                .as_ref()
                .is_none_or(|(status, _)| !status.is_temporary() && !status.is_permanent());

            if should_spool {
                self.spool_message(validate_context).await;
            }
        }
    }

    /// Spool message after validation passes
    #[traced(instrument(level = tracing::Level::TRACE, skip_all), timing)]
    async fn spool_message(&self, validate_context: &mut context::Context) {
        internal!("Spooling message");

        let tracking_id = if let Some(spool) = &self.spool
            && validate_context.data.is_some()
        {
            match spool.write(validate_context).await {
                Ok(id) => Some(id),
                Err(e) => {
                    internal!(level = ERROR, "Failed to spool message: {e}");
                    validate_context.response = Some((
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
        validate_context.metadata.remove("declared_size");

        // Set success response with tracking ID
        validate_context.response = Some((
            Status::Ok,
            tracking_id.as_ref().map_or_else(
                || Cow::Borrowed("Ok: queued"),
                |id| Cow::Owned(format!("Ok: queued as {id}")),
            ),
        ));

        // Dispatch message received event
        modules::dispatch(
            modules::Event::Event(modules::Ev::SmtpMessageReceived),
            validate_context,
        );
    }
}
