use std::borrow::Cow;

use empath_common::{context, error::SessionError, internal, status::Status, tracing};
use empath_tracing::traced;
use tokio::io::{AsyncRead, AsyncWrite};

use crate::{State, command::Command, state};

use super::{Context, Session};

impl<Stream: AsyncRead + AsyncWrite + Unpin + Send + Sync> Session<Stream> {
    /// Receive and process data from the client
    ///
    /// Returns `Ok(true)` if the connection should be closed, `Ok(false)` to continue.
    ///
    /// # Errors
    /// Returns `SessionError` if there's a protocol error or I/O failure.
    #[traced(instrument(level = tracing::Level::TRACE, skip_all, ret), timing)]
    pub(super) async fn receive(
        &mut self,
        validate_context: &mut context::Context,
    ) -> Result<bool, SessionError> {
        let mut received_data = [0; 4096];

        match self.connection.receive(&mut received_data).await {
            // Consider any errors received here to be fatal
            Err(err) => {
                internal!("Error: {err}");
                Err(SessionError::Protocol(err.to_string()))
            }
            Ok(0) => {
                // Reading 0 bytes means the other side has closed the
                // connection or is done writing, then so are we.
                Ok(true)
            }
            Ok(bytes_read) => {
                let received = &received_data[..bytes_read];

                if matches!(self.context.state, State::Reading(_)) {
                    self.handle_data_reception(received, validate_context);
                } else {
                    self.handle_command_reception(received, validate_context);
                }

                Ok(false)
            }
        }
    }

    /// Handle reception of message data (during DATA state)
    fn handle_data_reception(
        &mut self,
        received: &[u8],
        validate_context: &mut context::Context,
    ) {
        // Check if adding received data would exceed limit (BEFORE extending buffer)
        // This prevents the buffer overflow vulnerability where an attacker could
        // consume up to max_message_size + 4095 bytes before being rejected
        // Use checked_add to prevent integer overflow on 32-bit systems
        if self.max_message_size > 0 {
            let total_size = self.context.message.len().saturating_add(received.len());

            if total_size > self.max_message_size {
                validate_context.response = Some((
                    Status::ExceededStorage,
                    Cow::Owned(format!(
                        "Actual message size {total_size} bytes exceeds maximum allowed size {} bytes",
                        self.max_message_size
                    )),
                ));
                self.context.state = State::Close(state::Close);
                self.context.sent = false;
                return;
            }
        }

        self.context.message.extend(received);

        if self.context.message.ends_with(b"\r\n.\r\n") {
            // Move the message buffer to avoid double cloning
            let message = std::mem::take(&mut self.context.message);

            self.context = Context {
                state: State::PostDot(state::PostDot),
                message: message.clone(),
                sent: false,
            };

            validate_context.data = Some(message.into());
        }
    }

    /// Handle reception of SMTP commands
    fn handle_command_reception(
        &mut self,
        received: &[u8],
        validate_context: &mut context::Context,
    ) {
        use empath_common::incoming;

        let command = Command::try_from(received).unwrap_or_else(|e| e);
        let message = command.inner().as_bytes().to_vec();

        incoming!("{command}");

        self.context = Context {
            state: self
                .context
                .state
                .clone()
                .transition(command, validate_context),
            message,
            sent: false,
        };

        tracing::debug!("Transitioned to {:#?}", self.context);
    }
}
