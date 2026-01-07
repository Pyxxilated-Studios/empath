//! Helper functions for transaction handling

use crate::State;

/// Helper function to determine if state requires validation.
///
/// This is used by the session orchestrator to decide whether to call
/// the business handler after an FSM transition.
///
/// # Arguments
///
/// * `state` - The current protocol state
///
/// # Returns
///
/// `true` if the state requires business logic validation
#[must_use]
pub const fn requires_validation(state: &State) -> bool {
    matches!(
        state,
        State::Connect(_)
            | State::Ehlo(_)
            | State::Helo(_)
            | State::Auth(_)
            | State::MailFrom(_)
            | State::RcptTo(_)
            | State::PostDot(_)
    )
}
