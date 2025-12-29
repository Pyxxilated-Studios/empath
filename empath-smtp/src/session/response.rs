use empath_common::status::Status;
use empath_common::{context, tracing};
use empath_ffi::modules::{self, Ev, Event as ModuleEvent};
use empath_tracing::traced;

use crate::session::{Event, Response, Session};
use crate::{State, state};

impl Session {
    /// Format and return the response to send to the client
    ///
    /// This is a pure formatter - all validation and work happens in `emit()` which is called before this.
    /// Just formats the response based on state and what `emit()` set in the context.
    #[traced(instrument(level = tracing::Level::TRACE, skip_all, ret), timing(precision = "ns"))]
    pub(crate) async fn response(&mut self, validate_context: &mut context::Context) -> Response {
        if self.context.sent {
            return (None, Event::ConnectionKeepAlive);
        }

        // If emit() set a response in the context, use it
        // Only close connection for Reject state, not all permanent errors
        if let Some((status, _)) = &validate_context.response {
            // Dispatch error event for 4xx and 5xx responses
            if status.is_temporary() || status.is_permanent() {
                let _ = modules::dispatch(ModuleEvent::Event(Ev::SmtpError), validate_context);
            }
        }

        if let Some((status, ref message)) = validate_context.response {
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
            State::Connect(_) => (
                Some(vec![format!("{} {}", Status::ServiceReady, self.banner)]),
                Event::ConnectionKeepAlive,
            ),
            State::Helo(_) => (
                Some(vec![format!(
                    "{} {} says hello to {}",
                    Status::Ok,
                    self.banner,
                    String::from_utf8_lossy(&self.context.message)
                )]),
                Event::ConnectionKeepAlive,
            ),
            State::Ehlo(_) => self.ehlo_response(validate_context),
            State::StartTls(_) => self.starttls_response(),
            State::MailFrom(_) | State::RcptTo(_) | State::PostDot(_) => (
                Some(vec![format!("{} Ok", Status::Ok)]),
                Event::ConnectionKeepAlive,
            ),
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
        // Check if STARTTLS extension is configured
        let tls_available = self
            .extensions
            .iter()
            .any(|ext| matches!(ext, crate::extensions::Extension::Starttls(_)));

        if tls_available {
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

    fn ehlo_response(&self, validate_context: &context::Context) -> Response {
        let client = String::from_utf8_lossy(&self.context.message);
        let mut items: Vec<String> = self
            .extensions
            .iter()
            .map(std::string::ToString::to_string)
            .collect();
        items.extend(
            validate_context
                .capabilities
                .iter()
                .map(std::string::ToString::to_string),
        );

        if items.is_empty() {
            return (
                Some(vec![format!(
                    "{} {} says hello to {}",
                    Status::Ok,
                    self.banner,
                    client
                )]),
                Event::ConnectionKeepAlive,
            );
        }

        let items_len = items.len();
        let mut lines = Vec::with_capacity(items_len + 1);
        lines.push(format!(
            "{}-{} says hello to {}",
            Status::Ok,
            self.banner,
            client
        ));

        for (idx, item) in items.into_iter().enumerate() {
            let is_last = idx + 1 == items_len;
            if is_last {
                lines.push(format!("{} {item}", Status::Ok));
            } else {
                lines.push(format!("{}-{item}", Status::Ok));
            }
        }

        (Some(lines), Event::ConnectionKeepAlive)
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
