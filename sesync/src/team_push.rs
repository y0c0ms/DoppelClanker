//! The push side of team sync: which local sessions may be sent to the team.
//!
//! The rule is fixed by the operator: only sessions the user marked are eligible. This module
//! decides the selection only; sealing and sending are separate, and both need a held lease.

use crate::state::State;

/// The sessions that may be pushed: those that exist locally and are marked. Order follows the
/// input, so the result is stable.
pub fn select_for_push(state: &State, local_sessions: &[String]) -> Vec<String> {
    local_sessions.iter().filter(|s| state.is_marked(s)).cloned().collect()
}
