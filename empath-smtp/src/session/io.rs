use std::borrow::Cow;

use empath_common::{context, incoming, status::Status, traits::fsm::FiniteStateMachine};

use crate::{
    State,
    command::Command,
    session::{Context, Session},
    session_state::SessionState,
    state,
};

impl Session {
    /// Handle reception of message data (during DATA state)
    ///
    /// Called by the orchestrator when receiving data in the Reading state.
    pub(crate) fn handle_data_reception(
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
    ///
    /// Called by the orchestrator when receiving commands (non-Reading states).
    pub(crate) fn handle_command_reception(
        &mut self,
        received: &[u8],
        validate_context: &mut context::Context,
    ) {
        let command = Command::try_from(received).unwrap_or_else(|e| e);
        let message = command.inner().as_bytes().to_vec();

        incoming!("{command}");

        // Handle RSET transaction clearing before transition (business logic side effect)
        // RSET clears transaction context but preserves session context
        let is_rset = matches!(command, Command::Rset);
        if is_rset {
            validate_context.transaction.clear();
        }

        // Extract protocol state from business context
        let mut session_state = SessionState::from_context(validate_context);

        // Pure FSM transition using the FiniteStateMachine trait
        let new_state =
            FiniteStateMachine::transition(self.context.state.clone(), command, &mut session_state);

        // Update session state
        self.context = Context {
            state: new_state,
            message,
            sent: false,
        };

        // Sync protocol state back to business context
        session_state.sync_to_context(validate_context);

        tracing::debug!("Transitioned to {:#?}", self.context);
    }
}
