use core::fmt::{self, Display, Formatter};

use empath_common::{address::Address, context::Context};
use serde::{Deserialize, Serialize};

use crate::command::{Command, HeloVariant};

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
}

/// After RCPT TO command (at least one recipient)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RcptTo {
    pub sender: Option<Address>,
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
    pub fn transition(self, command: Command, ctx: &mut Context) -> Self {
        match (self, command) {
            // Connect state transitions
            (Self::Connect(_), Command::Helo(HeloVariant::Ehlo(id))) => {
                ctx.id.clone_from(&id);
                ctx.extended = true;
                Self::Ehlo(Ehlo { id })
            }
            (Self::Connect(_), Command::Helo(HeloVariant::Helo(id))) => {
                ctx.id.clone_from(&id);
                Self::Helo(Helo { id })
            }

            // EHLO/HELO transitions (can do STARTTLS or HELP)
            (Self::Ehlo(_) | Self::Helo(_), Command::StartTLS) if ctx.extended => {
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
                Command::MailFrom(sender, _),
            ) => {
                ctx.envelope.sender_mut().clone_from(&sender);
                Self::MailFrom(MailFrom { sender })
            }

            // Cannot do STARTTLS after mail transaction has started
            (Self::MailFrom(_) | Self::RcptTo(_) | Self::Data(_), Command::StartTLS) => {
                Self::Invalid(Invalid {
                    reason: "STARTTLS not allowed during mail transaction".to_string(),
                })
            }

            // Recipient collection (can add multiple recipients)
            (Self::MailFrom(state), Command::RcptTo(recipients)) => {
                if let Some(rcpts) = ctx.envelope.recipients_mut() {
                    rcpts.extend_from_slice(&recipients[..]);
                } else {
                    *ctx.envelope.recipients_mut() = Some(recipients);
                }
                Self::RcptTo(RcptTo {
                    sender: state.sender,
                })
            }
            (Self::RcptTo(state), Command::RcptTo(recipients)) => {
                if let Some(rcpts) = ctx.envelope.recipients_mut() {
                    rcpts.extend_from_slice(&recipients[..]);
                } else {
                    *ctx.envelope.recipients_mut() = Some(recipients);
                }
                Self::RcptTo(state) // Stay in RcptTo, accumulating recipients
            }

            // DATA command (must have at least one recipient)
            (Self::RcptTo(_), Command::Data) => Self::Data(Data),

            // After DATA response, client sends message body
            (Self::Data(_), _) => Self::Reading(Reading),

            // QUIT from any state
            (_, Command::Quit) => Self::Quit(Quit),

            // Invalid transitions
            (Self::Invalid(state), _) => Self::Invalid(state),
            (state, _) => Self::Invalid(Invalid {
                reason: format!("Invalid command sequence from {state}"),
            }),
        }
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
mod test {
    use empath_common::address::AddressList;
    use mailparse::addrparse;

    use super::*;

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

        let state = State::MailFrom(MailFrom { sender: None });
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
        let sender: AddressList = addrparse("sender@example.com").unwrap().into();
        let state = state.transition(
            Command::MailFrom(
                sender.iter().next().cloned(),
                crate::command::MailParameters::new(),
            ),
            &mut ctx,
        );
        assert!(matches!(state, State::MailFrom(_)));

        // RCPT TO
        let rcpt: AddressList = addrparse("recipient@example.com").unwrap().into();
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
}
