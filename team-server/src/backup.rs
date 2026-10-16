//! Per-space backup policy: whether a space is backed up, how often, and how long copies
//! are kept. Validated before it is stored, so the scheduler only ever sees good values.

use serde::{Deserialize, Serialize};

/// Longest interval allowed: one year. Anything longer is almost certainly a mistake.
pub const MAX_INTERVAL_HOURS: u32 = 24 * 365;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value")]
pub enum Retention {
    Copies(u32),
    Days(u32),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupPolicy {
    pub enabled: bool,
    pub interval_hours: u32,
    pub retention: Retention,
}

#[derive(Debug, PartialEq, Eq)]
pub enum PolicyError {
    Interval(String),
    Retention(String),
}

impl Default for BackupPolicy {
    fn default() -> Self {
        BackupPolicy { enabled: false, interval_hours: 24, retention: Retention::Copies(7) }
    }
}

impl BackupPolicy {
    /// Build a policy, refusing an interval or retention that would not work. An interval is
    /// checked even when backups are disabled, so a bad value can never be saved.
    pub fn new(enabled: bool, interval_hours: u32, retention: Retention) -> Result<BackupPolicy, PolicyError> {
        if interval_hours == 0 || interval_hours > MAX_INTERVAL_HOURS {
            return Err(PolicyError::Interval(format!(
                "interval must be 1 to {MAX_INTERVAL_HOURS} hours, got {interval_hours}")));
        }
        match retention {
            Retention::Copies(0) => return Err(PolicyError::Retention("keep at least one copy".into())),
            Retention::Days(0) => return Err(PolicyError::Retention("keep at least one day".into())),
            _ => {}
        }
        Ok(BackupPolicy { enabled, interval_hours, retention })
    }
}
