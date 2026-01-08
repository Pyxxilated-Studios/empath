use std::borrow::Cow;

use empath_common::status::Status;
use empath_common::traits::fsm::FiniteStateMachine;
use empath_common::{context, incoming};

use crate::auth::{AuthMechanism, PlainAuth, PlainCredentials};
use crate::command::{Command, XclientParameters};
use crate::session::{AuthPending, Context, Session};
use crate::session_state::SessionState;
use crate::{State, state};

trait ApplyAuthCredentials {
    fn apply_to_context(&self, validate_context: &mut context::Context);
}

impl ApplyAuthCredentials for PlainCredentials {
    fn apply_to_context(&self, validate_context: &mut context::Context) {
        validate_context.set_authzid(self.authzid.clone());
        validate_context.set_auth_user(self.authcid.clone());
        validate_context.set_auth_password(self.password.clone());
    }
}

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
                // Fresh empty buffer for subsequent commands; the body was moved into data.
                message: Vec::new(),
                sent: false,
                auth_pending: None,
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
        if let Some(pending) = self.context.auth_pending.take() {
            self.handle_auth_response(received, &pending, validate_context);
            return;
        }

        let command = Command::try_from(received).unwrap_or_else(|e| e);
        let message = command.inner().as_bytes().to_vec();

        incoming!("{command}");

        if self.handle_auth_command(&command, validate_context) {
            self.context.message = message;
            self.context.sent = false;
            return;
        }

        Self::apply_command_response(&command, validate_context);

        // Handle RSET transaction clearing before transition (business logic side effect)
        // RSET clears transaction context but preserves session context
        let is_rset = matches!(command, Command::Rset);
        if is_rset {
            validate_context.transaction.clear();
        }

        // Extract protocol state from business context
        let mut session_state = SessionState::from_context(validate_context);

        // Pure FSM transition using the FiniteStateMachine trait
        // Use std::mem::take to avoid cloning state - the old state is immediately replaced
        let new_state = FiniteStateMachine::transition(
            std::mem::take(&mut self.context.state),
            command,
            &mut session_state,
        );

        // Update session state
        self.context = Context {
            state: new_state,
            message,
            sent: false,
            auth_pending: self.context.auth_pending.take(),
        };

        // Sync protocol state back to business context
        session_state.sync_to_context(validate_context);

        tracing::debug!("Transitioned to {:#?}", self.context);
    }

    fn apply_command_response(command: &Command, validate_context: &mut context::Context) {
        match command {
            Command::Noop(_) => {
                validate_context.response = Some((Status::Ok, Cow::Borrowed("Ok")));
            }
            Command::Vrfy(_) => {
                validate_context.response =
                    Some((Status::Unknown(252), Cow::Borrowed("Cannot VRFY user")));
            }
            Command::Expn(_) => {
                validate_context.response =
                    Some((Status::Unknown(252), Cow::Borrowed("Cannot expand list")));
            }
            Command::Etrn(_) | Command::Turn => {
                validate_context.response = Some((
                    Status::Unknown(502),
                    Cow::Borrowed("Command not implemented"),
                ));
            }
            Command::Xclient(params) => {
                Self::apply_xclient(params, validate_context);
                validate_context.response = Some((Status::Ok, Cow::Borrowed("Ok")));
            }
            _ => {}
        }
    }

    fn apply_xclient(params: &XclientParameters, validate_context: &mut context::Context) {
        fn apply_value(ctx: &mut context::Context, key: &'static str, value: &str) {
            if value.eq_ignore_ascii_case("UNKNOWN") {
                ctx.session.remove(key);
            } else {
                ctx.session.insert(Cow::Borrowed(key), value.to_string());
            }
        }

        if let Some(addr) = params.get("ADDR") {
            apply_value(validate_context, "client_ip", addr);
        }
        if let Some(name) = params.get("NAME") {
            apply_value(validate_context, "client_name", name);
        }
        if let Some(helo) = params.get("HELO") {
            apply_value(validate_context, "client_helo", helo);
        }
        if let Some(proto) = params.get("PROTO") {
            apply_value(validate_context, "client_proto", proto);
        }
        if let Some(login) = params.get("LOGIN") {
            apply_value(validate_context, "client_login", login);
        }
        if let Some(dest_addr) = params.get("DESTADDR") {
            apply_value(validate_context, "dest_addr", dest_addr);
        }
        if let Some(dest_port) = params.get("DESTPORT") {
            apply_value(validate_context, "dest_port", dest_port);
        }
        if let Some(port) = params.get("PORT") {
            apply_value(validate_context, "client_port", port);
        }
    }

    fn handle_auth_command(
        &mut self,
        command: &Command,
        validate_context: &mut context::Context,
    ) -> bool {
        let Command::Auth {
            mechanism,
            initial_response,
        } = command
        else {
            return false;
        };

        self.context.sent = false;

        if self.context.state.in_transaction() {
            validate_context.response = Some((
                Status::InvalidCommandSequence,
                Cow::Borrowed("Bad sequence of commands"),
            ));
            return true;
        }

        if !validate_context.extended {
            validate_context.response = Some((
                Status::InvalidCommandSequence,
                Cow::Borrowed("EHLO required before AUTH"),
            ));
            return true;
        }

        if !validate_context
            .capabilities
            .iter()
            .any(|cap| cap.as_str() == "AUTH")
        {
            validate_context.response =
                Some((Status::Unknown(502), Cow::Borrowed("AUTH not available")));
            return true;
        }

        if !validate_context.is_tls() {
            validate_context.response = Some((
                Status::Unknown(530),
                Cow::Borrowed("Must issue STARTTLS first"),
            ));
            return true;
        }

        if !mechanism.eq_ignore_ascii_case(PlainAuth::NAME) {
            validate_context.response = Some((
                Status::Unknown(504),
                Cow::Borrowed("Unrecognized authentication type"),
            ));
            return true;
        }

        if initial_response.as_ref().is_some_and(|value| value == "=") {
            self.context.auth_pending = Some(AuthPending {
                mechanism: mechanism.clone(),
            });
            self.context.state = State::Auth(state::Auth {
                mechanism: mechanism.clone(),
                awaiting_response: true,
            });
            validate_context.response = Some((Status::Unknown(334), Cow::Borrowed("")));
            return true;
        }

        if let Some(initial_response) = initial_response {
            if let Err(err) = Self::apply_auth::<PlainAuth>(initial_response, validate_context) {
                validate_context.response =
                    Some((Status::Unknown(535), Cow::Owned(err.to_string())));
                return true;
            }

            self.context.state = State::Auth(state::Auth {
                mechanism: mechanism.clone(),
                awaiting_response: false,
            });
            return true;
        }

        self.context.auth_pending = Some(AuthPending {
            mechanism: mechanism.clone(),
        });
        self.context.state = State::Auth(state::Auth {
            mechanism: mechanism.clone(),
            awaiting_response: true,
        });
        validate_context.response = Some((Status::Unknown(334), Cow::Borrowed("")));
        true
    }

    fn handle_auth_response(
        &mut self,
        received: &[u8],
        pending: &AuthPending,
        validate_context: &mut context::Context,
    ) {
        let response = std::str::from_utf8(received)
            .unwrap_or("")
            .trim()
            .trim_end_matches('\r')
            .trim_end_matches('\n')
            .to_string();

        if response == "*" {
            validate_context.response = Some((
                Status::Unknown(501),
                Cow::Borrowed("Authentication canceled"),
            ));
            self.context.state = Self::ready_state(validate_context);
            self.context.sent = false;
            return;
        }

        if let Err(err) = Self::apply_auth::<PlainAuth>(&response, validate_context) {
            validate_context.response = Some((Status::Unknown(535), Cow::Owned(err.to_string())));
            self.context.state = Self::ready_state(validate_context);
            self.context.sent = false;
            return;
        }

        self.context.state = State::Auth(state::Auth {
            mechanism: pending.mechanism.clone(),
            awaiting_response: false,
        });
        self.context.sent = false;
    }

    fn apply_auth<M: AuthMechanism>(
        response: &str,
        validate_context: &mut context::Context,
    ) -> Result<(), crate::auth::AuthError>
    where
        M::Credentials: ApplyAuthCredentials,
    {
        let creds = M::decode(response)?;
        validate_context.set_auth_mechanism(M::NAME.to_string());
        creds.apply_to_context(validate_context);
        Ok(())
    }

    fn ready_state(validate_context: &context::Context) -> State {
        if validate_context.extended {
            State::Ehlo(state::Ehlo {
                id: validate_context.id.clone(),
            })
        } else {
            State::Helo(state::Helo {
                id: validate_context.id.clone(),
            })
        }
    }
}
