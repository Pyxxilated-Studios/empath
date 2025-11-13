use core::bstr;

use empath_common::{context, status::Status, tracing};
use empath_tracing::traced;
use tokio::io::{AsyncRead, AsyncWrite};

use crate::{
    State,
    session::{Event, Response, Session},
    state,
};

impl<Stream: AsyncRead + AsyncWrite + Unpin + Send + Sync> Session<Stream> {
    /// Format and return the response to send to the client
    ///
    /// This is a pure formatter - all validation and work happens in `emit()`.
    /// Just formats the response based on state and what `emit()` set in the context.
    #[traced(instrument(level = tracing::Level::TRACE, skip_all, ret), timing(precision = "ns"))]
    pub(super) async fn response(&mut self, validate_context: &mut context::Context) -> Response {
        if self.context.sent {
            return (None, Event::ConnectionKeepAlive);
        }

        // Emit events, do validation and work first
        self.emit(validate_context).await;

        // If emit() set a response in the context, use it
        // Only close connection for Reject state, not all permanent errors
        if let Some((status, ref message)) = validate_context.response {
            // Record error metrics for 4xx and 5xx responses
            if empath_metrics::is_enabled()
                && (status.is_temporary() || status.is_permanent())
            {
                empath_metrics::metrics().smtp.record_error(status.into());
            }

            let event = if matches!(self.context.state, State::Reject(_)) && status.is_permanent() {
                Event::ConnectionClose
            } else {
                Event::ConnectionKeepAlive
            };

            return (Some(vec![format!("{status} {message}")]), event);
        }

        // Otherwise, provide default responses for states not handled by emit()
        self.default_response(validate_context)
    }

    /// Provide default responses for states not handled by `emit()`
    fn default_response(&mut self, validate_context: &context::Context) -> Response {
        match &self.context.state {
            State::Helo(_) => (
                Some(vec![format!(
                    "{} {} says hello to {}",
                    Status::Ok,
                    self.banner,
                    bstr::ByteStr::new(&self.context.message)
                )]),
                Event::ConnectionKeepAlive,
            ),
            State::StartTls(_) => self.starttls_response(),
            State::Data(_) => self.data_response(validate_context),
            State::Quit(_) => (
                Some(vec![format!("{} Bye", Status::GoodBye)]),
                Event::ConnectionClose,
            ),
            State::Invalid(_) => (
                Some(vec![format!(
                    "{} {}",
                    Status::InvalidCommandSequence,
                    self.context.state
                )]),
                Event::ConnectionClose,
            ),
            State::Reject(_) => {
                // Reject should have response set by emit(), but provide fallback
                (
                    Some(vec![format!("{} Unavailable", Status::Unavailable)]),
                    Event::ConnectionClose,
                )
            }
            // States handled by emit() (Connect, Ehlo, MailFrom, RcptTo, PostDot) should have set a response
            // States like Reading, Close, and others that don't need responses
            _ => (None, Event::ConnectionKeepAlive),
        }
    }

    /// Generate response for STARTTLS command
    fn starttls_response(&self) -> Response {
        if self.tls_context.is_some() {
            (
                Some(vec![format!("{} Ready to begin TLS", Status::ServiceReady)]),
                Event::ConnectionKeepAlive,
            )
        } else {
            (
                Some(vec![format!("{} TLS not available", Status::Error)]),
                Event::ConnectionClose,
            )
        }
    }

    /// Generate response for DATA command and transition to Reading state
    fn data_response(&mut self, validate_context: &context::Context) -> Response {
        self.context.state = State::Reading(state::Reading);

        // Pre-allocate message buffer based on SIZE parameter if declared
        if let Some(params) = validate_context.envelope.mail_params()
            && let Some(Some(size_str)) = params.get("SIZE")
            && let Ok(declared_size) = size_str.parse::<usize>()
        {
            // Reserve capacity to avoid reallocations during message receipt
            self.context.message.reserve(declared_size);
        }

        (
            Some(vec![format!(
                "{} End data with <CR><LF>.<CR><LF>",
                Status::StartMailInput
            )]),
            Event::ConnectionKeepAlive,
        )
    }
}
