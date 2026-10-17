//! The team server as seen by the TUI: list a space's sessions. The TUI depends only on this
//! trait, so the network client can change without touching the screen logic, and tests run
//! with a fake.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpaceSession {
    pub id: String,
    pub title: String,
    pub updated_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteError {
    /// The server could not be reached at all.
    Unreachable,
    /// The server refused the request, for example a token that has expired.
    Refused(u16),
}

pub trait RemoteSessions {
    fn list(&self, space: &str) -> Result<Vec<SpaceSession>, RemoteError>;
}
