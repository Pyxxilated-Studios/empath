use core::fmt::{self, Display, Formatter};

use empath_common::{address::Address, context::Context};
use serde::{Deserialize, Serialize};

use crate::{
    command::{Command, HeloVariant},
    session_state::SessionState,
};

/// Sealed trait to prevent external state implementations
mod sealed {
    pub trait Sealed {}
}

/// Marker trait for valid SMTP states
pub trait SmtpState: sealed::Sealed + core::fmt::Debug {}

// ============================================================================
// State Definitions
// ============================================================================

/// Initial connection state - client just connected
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Connect;

/// After successful EHLO command (extended SMTP)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ehlo {
    pub id: String,
}

/// After successful HELO command (basic SMTP)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Helo {
    pub id: String,
}

/// HELP command was issued
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Help {
    pub from_ehlo: bool,
}

/// After successful STARTTLS negotiation (only from EHLO/HELO, not mid-transaction)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartTls;

/// After MAIL FROM command (beginning of mail transaction)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MailFrom {
    pub sender: Option<Address>,
    pub params: super::MailParameters,
}

/// After RCPT TO command (at least one recipient)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RcptTo {
    pub sender: Option<Address>,
    pub params: super::MailParameters,
}

/// After DATA command (ready to receive message body)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Data;

/// Reading message data (after DATA command, before end-of-data marker)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reading;

/// After end-of-data marker (.\r\n), message complete
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PostDot;

/// Client issued QUIT command
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Quit;

/// Invalid command or sequence
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Invalid {
    pub reason: String,
}

/// Connection rejected by validation
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reject;

/// Connection closing
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Close;

// ============================================================================
// Sealed Trait Implementations
// ============================================================================

impl sealed::Sealed for Connect {}
impl sealed::Sealed for Ehlo {}
impl sealed::Sealed for Helo {}
impl sealed::Sealed for Help {}
impl sealed::Sealed for StartTls {}
impl sealed::Sealed for MailFrom {}
impl sealed::Sealed for RcptTo {}
impl sealed::Sealed for Data {}
impl sealed::Sealed for Reading {}
impl sealed::Sealed for PostDot {}
impl sealed::Sealed for Quit {}
impl sealed::Sealed for Invalid {}
impl sealed::Sealed for Reject {}
impl sealed::Sealed for Close {}

impl SmtpState for Connect {}
impl SmtpState for Ehlo {}
impl SmtpState for Helo {}
impl SmtpState for Help {}
impl SmtpState for StartTls {}
impl SmtpState for MailFrom {}
impl SmtpState for RcptTo {}
impl SmtpState for Data {}
impl SmtpState for Reading {}
impl SmtpState for PostDot {}
impl SmtpState for Quit {}
impl SmtpState for Invalid {}
impl SmtpState for Reject {}
impl SmtpState for Close {}

// ============================================================================
// State Enum for Dynamic Dispatch
// ============================================================================

/// Type-safe state enum that wraps all possible states
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum State {
    Connect(Connect),
    Ehlo(Ehlo),
    Helo(Helo),
    Help(Help),
    StartTls(StartTls),
    MailFrom(MailFrom),
    RcptTo(RcptTo),
    Data(Data),
    Reading(Reading),
    PostDot(PostDot),
    Quit(Quit),
    Invalid(Invalid),
    Reject(Reject),
    Close(Close),
}

impl Default for State {
    fn default() -> Self {
        Self::Connect(Connect)
    }
}

impl Display for State {
    fn fmt(&self, fmt: &mut Formatter<'_>) -> Result<(), fmt::Error> {
        fmt.write_str(match self {
            Self::Reading(_) | Self::PostDot(_) => "",
            Self::Connect(_) => "Connect",
            Self::Close(_) => "Close",
            Self::Ehlo(_) => "EHLO",
            Self::Helo(_) => "HELO",
            Self::Help(_) => "HELP",
            Self::StartTls(_) => "STARTTLS",
            Self::MailFrom(_) => "MAIL",
            Self::RcptTo(_) => "RCPT",
            Self::Data(_) => "DATA",
            Self::Quit(_) => "QUIT",
            Self::Invalid(_) => "INVALID",
            Self::Reject(_) => "Rejected",
        })
    }
}

// ============================================================================
// Type-Safe Transition Methods
// ============================================================================

impl State {
    /// Transition from current state based on received command
    ///
    /// This method enforces valid state transitions at runtime while using
    /// type-safe state structs internally
    #[must_use]
    /// Pure FSM transition using session state
    ///
    /// This is the new transition method that works with `SessionState`
    /// instead of the full business `Context`. It contains only pure protocol
    /// state transitions without side effects on business logic.
    ///
    /// # Arguments
    ///
    /// * `command` - The SMTP command to process
    /// * `session_state` - Session-level state (id, extended, envelope)
    ///
    /// # Returns
    ///
    /// The new state after transition
    pub fn transition_protocol(self, command: Command, session_state: &mut SessionState) -> Self {
        match (self, command) {
            // Connect state transitions
            (Self::Connect(_), Command::Helo(HeloVariant::Ehlo(id))) => {
                session_state.id.clone_from(&id);
                session_state.extended = true;
                Self::Ehlo(Ehlo { id })
            }
            (Self::Connect(_), Command::Helo(HeloVariant::Helo(id))) => {
                session_state.id.clone_from(&id);
                Self::Helo(Helo { id })
            }

            // EHLO/HELO transitions (can do STARTTLS or HELP)
            (Self::Ehlo(_) | Self::Helo(_), Command::StartTLS) if session_state.extended => {
                Self::StartTls(StartTls)
            }
            (Self::Ehlo(_), Command::Help) => Self::Help(Help { from_ehlo: true }),
            (Self::Helo(_), Command::Help) => Self::Help(Help { from_ehlo: false }),

            // Begin mail transaction (only from authenticated/ready states, NOT from MailFrom/RcptTo/Data)
            (
                Self::Ehlo(_)
                | Self::Helo(_)
                | Self::StartTls(_)
                | Self::Help(_)
                | Self::PostDot(_),
                Command::MailFrom(sender, params),
            ) => {
                session_state.envelope.sender_mut().clone_from(&sender);
                // Store all MAIL FROM parameters in envelope for module access
                *session_state.envelope.mail_params_mut() = Some(params.clone().into());
                Self::MailFrom(MailFrom { sender, params })
            }

            // Cannot do STARTTLS after mail transaction has started
            (Self::MailFrom(_) | Self::RcptTo(_) | Self::Data(_), Command::StartTLS) => {
                Self::Invalid(Invalid {
                    reason: "STARTTLS not allowed during mail transaction".to_string(),
                })
            }

            // Recipient collection (can add multiple recipients)
            (Self::MailFrom(state), Command::RcptTo(recipients)) => {
                if let Some(rcpts) = session_state.envelope.recipients_mut() {
                    rcpts.extend_from_slice(&recipients[..]);
                } else {
                    *session_state.envelope.recipients_mut() = Some(recipients);
                }
                Self::RcptTo(RcptTo {
                    sender: state.sender,
                    params: state.params,
                })
            }
            (Self::RcptTo(state), Command::RcptTo(recipients)) => {
                if let Some(rcpts) = session_state.envelope.recipients_mut() {
                    rcpts.extend_from_slice(&recipients[..]);
                } else {
                    *session_state.envelope.recipients_mut() = Some(recipients);
                }
                Self::RcptTo(state) // Stay in RcptTo, accumulating recipients
            }

            // DATA command (must have at least one recipient)
            (Self::RcptTo(_), Command::Data) => Self::Data(Data),

            // After DATA response, client sends message body
            (Self::Data(_), _) => Self::Reading(Reading),

            // RSET clears transaction state and returns to ready state (EHLO or HELO)
            (_, Command::Rset) => {
                // Clear transaction state (envelope only, no business logic metadata)
                *session_state.envelope.sender_mut() = None;
                *session_state.envelope.recipients_mut() = None;
                *session_state.envelope.mail_params_mut() = None;
                if session_state.extended {
                    Self::Ehlo(Ehlo {
                        id: session_state.id.clone(),
                    })
                } else {
                    Self::Helo(Helo {
                        id: session_state.id.clone(),
                    })
                }
            }

            // QUIT from any state
            (_, Command::Quit) => Self::Quit(Quit),

            // AUTH not yet implemented - return invalid for now
            (_, Command::Auth) => Self::Invalid(Invalid {
                reason: "AUTH not implemented".to_string(),
            }),

            // Invalid transitions
            (Self::Invalid(state), _) => Self::Invalid(state),
            (state, _) => Self::Invalid(Invalid {
                reason: format!("Invalid command sequence from {state}"),
            }),
        }
    }

    /// Legacy transition method for backward compatibility
    ///
    /// This method maintains backward compatibility with existing code that
    /// uses the full business `Context`. It delegates to `transition_protocol`
    /// and syncs the protocol state back to the context.
    ///
    /// # Deprecated
    ///
    /// New code should use `transition_protocol` with `SessionState` instead.
    /// This method will be removed in a future version after all call sites
    /// are migrated.
    ///
    /// # Arguments
    ///
    /// * `command` - The SMTP command to process
    /// * `ctx` - Business context (contains protocol fields + business logic)
    ///
    /// # Returns
    ///
    /// The new state after transition
    #[must_use]
    pub fn transition(self, command: Command, ctx: &mut Context) -> Self {
        // Extract session state from business context
        let mut session_state = SessionState::from_context(ctx);

        // Clear metadata for RSET command (business logic side effect)
        if matches!(command, Command::Rset) {
            ctx.metadata.clear();
        }

        // Perform pure FSM transition
        let new_state = self.transition_protocol(command, &mut session_state);

        // Sync session state back to business context
        session_state.sync_to_context(ctx);

        new_state
    }

    /// Check if this state represents an error condition
    #[must_use]
    pub const fn is_error(&self) -> bool {
        matches!(self, Self::Invalid(_) | Self::Reject(_))
    }

    /// Check if this state should close the connection
    #[must_use]
    pub const fn should_close(&self) -> bool {
        matches!(self, Self::Quit(_) | Self::Close(_) | Self::Reject(_))
    }

    /// Check if we're in a mail transaction (between MAIL FROM and `PostDot`)
    #[must_use]
    pub const fn in_transaction(&self) -> bool {
        matches!(
            self,
            Self::MailFrom(_) | Self::RcptTo(_) | Self::Data(_) | Self::Reading(_)
        )
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod test {
    use empath_common::{
        address::{Address, AddressList},
        address_parser,
    };

    use super::*;
    use crate::MailParameters;

    #[test]
    fn connect_to_ehlo() {
        let mut ctx = Context::default();
        let state = State::default();

        let new_state = state.transition(
            Command::Helo(HeloVariant::Ehlo("client.example.com".to_string())),
            &mut ctx,
        );

        assert!(matches!(new_state, State::Ehlo(_)));
        assert_eq!(ctx.id, "client.example.com");
        assert!(ctx.extended);
    }

    #[test]
    fn ehlo_to_starttls() {
        let mut ctx = Context {
            extended: true,
            ..Context::default()
        };

        let state = State::Ehlo(Ehlo {
            id: "client.example.com".to_string(),
        });
        let new_state = state.transition(Command::StartTLS, &mut ctx);

        assert!(matches!(new_state, State::StartTls(_)));
    }

    #[test]
    fn prevent_starttls_after_mail_from() {
        let mut ctx = Context {
            extended: true,
            ..Context::default()
        };

        let state = State::MailFrom(MailFrom {
            sender: None,
            params: MailParameters::new(),
        });
        let new_state = state.transition(Command::StartTLS, &mut ctx);

        assert!(matches!(new_state, State::Invalid(_)));
        if let State::Invalid(invalid) = new_state {
            assert!(
                invalid
                    .reason
                    .contains("not allowed during mail transaction")
            );
        }
    }

    #[test]
    fn mail_transaction_flow() {
        let mut ctx = Context {
            extended: true,
            ..Context::default()
        };

        // EHLO
        let state = State::default();
        let state = state.transition(
            Command::Helo(HeloVariant::Ehlo("client.example.com".to_string())),
            &mut ctx,
        );
        assert!(matches!(state, State::Ehlo(_)));

        // MAIL FROM
        let sender_mailbox = address_parser::parse_forward_path("<sender@example.com>").unwrap();
        let state = state.transition(
            Command::MailFrom(
                Some(Address::from(sender_mailbox)),
                crate::command::MailParameters::new(),
            ),
            &mut ctx,
        );
        assert!(matches!(state, State::MailFrom(_)));

        // RCPT TO
        let rcpt_mailbox = address_parser::parse_forward_path("<recipient@example.com>").unwrap();
        let rcpt = AddressList::from(vec![Address::from(rcpt_mailbox)]);
        let state = state.transition(Command::RcptTo(rcpt), &mut ctx);
        assert!(matches!(state, State::RcptTo(_)));

        // DATA
        let state = state.transition(Command::Data, &mut ctx);
        assert!(matches!(state, State::Data(_)));
    }

    #[test]
    fn quit_from_any_state() {
        let mut ctx = Context::default();

        // From Connect
        let state = State::default();
        let state = state.transition(Command::Quit, &mut ctx);
        assert!(matches!(state, State::Quit(_)));
        assert!(state.should_close());

        // From Ehlo
        let state = State::Ehlo(Ehlo {
            id: "test".to_string(),
        });
        let state = state.transition(Command::Quit, &mut ctx);
        assert!(matches!(state, State::Quit(_)));
    }

    #[test]
    fn rset_clears_transaction() {
        let mut ctx = Context {
            extended: true,
            id: "client.example.com".to_string(),
            ..Context::default()
        };

        // Start with MailFrom state
        let sender_mailbox = address_parser::parse_forward_path("<sender@example.com>").unwrap();
        let sender_addr = Address::from(sender_mailbox);
        *ctx.envelope.sender_mut() = Some(sender_addr.clone());

        let state = State::MailFrom(MailFrom {
            sender: Some(sender_addr),
            params: MailParameters::new(),
        });

        // Verify sender is set
        assert!(ctx.envelope.sender().is_some());

        // RSET should clear transaction and return to EHLO
        let state = state.transition(Command::Rset, &mut ctx);
        assert!(matches!(state, State::Ehlo(_)));

        // Verify envelope is cleared
        assert!(ctx.envelope.sender().is_none());
        assert!(ctx.envelope.recipients().is_none());
        assert!(ctx.envelope.mail_params().is_none());
    }

    #[test]
    fn auth_returns_invalid() {
        let mut ctx = Context::default();
        let state = State::Ehlo(Ehlo {
            id: "test".to_string(),
        });

        let state = state.transition(Command::Auth, &mut ctx);
        assert!(matches!(state, State::Invalid(_)));
        if let State::Invalid(invalid) = state {
            assert!(invalid.reason.contains("not implemented"));
        }
    }
}
