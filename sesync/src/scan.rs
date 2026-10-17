//! Change detection rule. Ported from daemon/sesync.py _scan_journals.
//! Kept as a pure function so every rule can be tested without a filesystem.

/// Files smaller than this are not sessions (a warm-up file, not a conversation).
pub const MIN_BYTES: u64 = 512;

pub struct ScanInput {
    pub size: u64,
    /// Last write time, seconds since the epoch.
    pub mtime: f64,
    pub now: f64,
    /// True when the path is a mirror the daemon created. Those are never sources.
    pub is_mirror: bool,
    pub max_age_secs: f64,
    /// Seconds of silence required before a file is treated as finished.
    pub quiet_secs: f64,
    /// The content hash differs from the one recorded at the last sync.
    pub hash_changed: bool,
    /// Every live (not diverged) mirror already carries the current hash.
    pub live_mirrors_current: bool,
    /// Mirrors recorded for this source.
    pub mirror_count: usize,
    /// How many harness targets should have a mirror.
    pub wanted_mirrors: usize,
}

/// Should this source be (re-)synced now?
pub fn should_sync(i: &ScanInput) -> bool {
    if i.is_mirror {
        return false; // loop prevention
    }
    if i.size < MIN_BYTES {
        return false;
    }
    if i.now - i.mtime > i.max_age_secs {
        return false;
    }
    if i.now - i.mtime < i.quiet_secs {
        return false; // still being written
    }
    let up_to_date = !i.hash_changed && i.live_mirrors_current && i.mirror_count >= i.wanted_mirrors;
    !up_to_date
}
