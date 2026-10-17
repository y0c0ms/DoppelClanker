//! Conflict display. When a local session and the team's copy differ, both are shown for the
//! user to choose. Nothing is merged or overwritten here; this only reports.

use sha2::{Digest, Sha256};

#[derive(Debug, PartialEq, Eq)]
pub enum Comparison {
    /// The same bytes on both sides. Nothing to show.
    Same,
    /// Only one side has the session. Shown as-is, not a conflict.
    OnlyLocal,
    OnlyTeam,
    /// Both sides have it and the bytes differ. The user must choose which to keep.
    Conflict { local_sha: String, team_sha: String },
}

fn sha(b: &[u8]) -> String { format!("{:x}", Sha256::digest(b)) }

/// Compare a local session with the team's copy. Returns what to show; never changes either side.
pub fn compare(local: Option<&[u8]>, team: Option<&[u8]>) -> Comparison {
    match (local, team) {
        (None, None) => Comparison::Same,
        (Some(_), None) => Comparison::OnlyLocal,
        (None, Some(_)) => Comparison::OnlyTeam,
        (Some(l), Some(t)) if l == t => Comparison::Same,
        (Some(l), Some(t)) => Comparison::Conflict { local_sha: sha(l), team_sha: sha(t) },
    }
}
