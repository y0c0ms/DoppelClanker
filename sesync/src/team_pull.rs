//! The pull side of team sync. A pull lists what the team holds and shows it. It never writes
//! into a local harness: that needs the user's confirmation, which is a separate step.

/// One team session as the pull sees it: an id and, when known, a title. Contents are not
/// fetched by a pull; they are fetched only on confirmed import.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteSession {
    pub id: String,
    pub title: String,
}

/// The pull's result for display. Holds only what is shown: nothing written, nothing imported.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct PullView {
    pub sessions: Vec<RemoteSession>,
}

/// Build the display for a pull from the team's session ids. Pure: it cannot write anywhere,
/// because it takes no path and returns only data.
pub fn view_of(remote_ids: &[String]) -> PullView {
    PullView {
        sessions: remote_ids.iter()
            .map(|id| RemoteSession { id: id.clone(), title: id.clone() })
            .collect(),
    }
}
