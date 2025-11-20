//! SMTP Finite State Machine implementation.
//!
//! This module provides a proper implementation of the `FiniteStateMachine` trait
//! for SMTP protocol state management. It ensures pure, side-effect-free state
//! transitions using only protocol-level context.

use empath_common::traits::fsm::FiniteStateMachine;

use crate::{command::Command, session_state::SessionState, state::State};

/// Implementation of the `FiniteStateMachine` trait for SMTP protocol states.
///
/// This implementation provides a formal, trait-compliant FSM for SMTP that:
/// - Uses `Command` as the input type
/// - Uses `SessionState` for state (not business `Context`)
/// - Performs pure state transitions without side effects
///
/// # Design Rationale
///
/// By implementing the `FiniteStateMachine` trait, we achieve:
/// - **Polymorphism**: State can be used generically with any FSM code
/// - **Purity**: Transitions are pure functions with no hidden side effects
/// - **Testability**: FSM can be tested in complete isolation
/// - **Separation of Concerns**: Protocol logic separated from business logic
///
/// # Example
///
/// ```rust
/// use empath_common::traits::fsm::FiniteStateMachine;
/// use empath_smtp::{
///     command::{Command, HeloVariant},
///     session_state::SessionState,
///     state::State,
/// };
///
/// let mut session_state = SessionState::new();
/// let state = State::default(); // Connect state
///
/// // Pure FSM transition using the trait method
/// let new_state = FiniteStateMachine::transition(
///     state,
///     Command::Helo(HeloVariant::Ehlo("client.example.com".to_string())),
///     &mut session_state,
/// );
///
/// assert_eq!(session_state.id(), "client.example.com");
/// assert!(session_state.is_extended());
/// ```
impl FiniteStateMachine for State {
    /// SMTP commands are the input to the FSM
    type Input = Command;

    /// Session state contains only FSM state (id, extended, envelope)
    ///
    /// This is intentionally separate from the business `Context` to ensure
    /// pure state transitions without business logic side effects.
    type Context = SessionState;

    /// Performs a pure state transition based on the input command.
    ///
    /// This method delegates to `State::transition_protocol()` which contains
    /// the actual transition logic. The FSM trait implementation provides a
    /// standard interface for generic FSM code.
    ///
    /// # Arguments
    ///
    /// * `input` - The SMTP command to process
    /// * `context` - Mutable protocol context for FSM state
    ///
    /// # Returns
    ///
    /// The new state after the transition
    ///
    /// # Purity
    ///
    /// This function is pure in the sense that:
    /// - It only modifies the protocol context (FSM state)
    /// - It has no side effects on business logic
    /// - It has no I/O operations
    /// - The same input + context always produces the same output
    fn transition(self, input: Self::Input, context: &mut Self::Context) -> Self {
        self.transition_protocol(input, context)
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use empath_common::{address::Address, address_parser, traits::fsm::FiniteStateMachine};

    use super::*;
    use crate::{MailParameters, command::HeloVariant};

    #[test]
    fn test_fsm_trait_ehlo_transition() {
        let mut session_state = SessionState::new();
        let state = State::default(); // Connect state

        // Use the FSM trait method explicitly
        let new_state = FiniteStateMachine::transition(
            state,
            Command::Helo(HeloVariant::Ehlo("client.example.com".to_string())),
            &mut session_state,
        );

        // Verify session state was updated
        assert_eq!(session_state.id(), "client.example.com");
        assert!(session_state.is_extended());

        // Verify we transitioned to Ehlo state
        assert!(matches!(new_state, State::Ehlo(_)));
    }

    #[test]
    fn test_fsm_trait_helo_transition() {
        let mut session_state = SessionState::new();
        let state = State::default();

        let new_state = FiniteStateMachine::transition(
            state,
            Command::Helo(HeloVariant::Helo("client.example.com".to_string())),
            &mut session_state,
        );

        assert_eq!(session_state.id(), "client.example.com");
        assert!(!session_state.is_extended()); // HELO does not set extended
        assert!(matches!(new_state, State::Helo(_)));
    }

    #[test]
    fn test_fsm_trait_mail_transaction() {
        let mut session_state = SessionState::with_id("client.example.com".to_string(), true);

        // Start with Ehlo state
        let state = State::Ehlo(crate::state::Ehlo {
            id: "client.example.com".to_string(),
        });

        // MAIL FROM
        let sender_mailbox = address_parser::parse_forward_path("<sender@example.com>").unwrap();
        let state = FiniteStateMachine::transition(
            state,
            Command::MailFrom(
                Some(Address::from(sender_mailbox)),
                MailParameters::default(),
            ),
            &mut session_state,
        );

        assert!(matches!(state, State::MailFrom(_)));
        assert!(session_state.envelope().sender().is_some());
    }

    #[test]
    fn test_fsm_trait_quit_from_any_state() {
        let mut session_state = SessionState::new();

        // Try QUIT from various states
        let states = vec![
            State::default(),
            State::Ehlo(crate::state::Ehlo {
                id: "test".to_string(),
            }),
            State::Helo(crate::state::Helo {
                id: "test".to_string(),
            }),
        ];

        for state in states {
            let new_state =
                FiniteStateMachine::transition(state, Command::Quit, &mut session_state);
            assert!(matches!(new_state, State::Quit(_)));
        }
    }

    #[test]
    fn test_fsm_trait_rset_clears_envelope() {
        let mut session_state = SessionState::with_id("client.example.com".to_string(), true);

        // Set up a mail transaction
        let sender_mailbox = address_parser::parse_forward_path("<sender@example.com>").unwrap();
        *session_state.envelope_mut().sender_mut() = Some(Address::from(sender_mailbox));

        let state = State::Ehlo(crate::state::Ehlo {
            id: "client.example.com".to_string(),
        });

        // RSET should clear the envelope
        let new_state = FiniteStateMachine::transition(state, Command::Rset, &mut session_state);

        assert!(matches!(new_state, State::Ehlo(_)));
        assert!(session_state.envelope().sender().is_none());
    }

    #[test]
    fn test_fsm_trait_polymorphic_usage() {
        // This test demonstrates that State can be used polymorphically
        // through the FiniteStateMachine trait
        fn run_fsm<F: FiniteStateMachine<Input = Command, Context = SessionState>>(
            fsm: F,
            input: Command,
            ctx: &mut SessionState,
        ) -> F {
            fsm.transition(input, ctx)
        }

        let mut session_state = SessionState::new();
        let state = State::default();

        let new_state = run_fsm(
            state,
            Command::Helo(HeloVariant::Ehlo("client.example.com".to_string())),
            &mut session_state,
        );

        assert!(matches!(new_state, State::Ehlo(_)));
        assert_eq!(session_state.id(), "client.example.com");
    }
}
