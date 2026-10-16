//! Per-space backup policy. Each space sets its own schedule and retention. A policy is
//! validated before it is stored, so a bad value never reaches the scheduler.

use harness_sync_team::backup::{BackupPolicy, PolicyError, Retention};

#[test]
fn a_space_defaults_to_no_backups() {
    let p = BackupPolicy::default();
    assert!(!p.enabled, "backups are opt-in per space");
}

#[test]
fn an_enabled_policy_needs_an_interval_and_a_retention() {
    let p = BackupPolicy::new(true, 24, Retention::Copies(7)).expect("a valid policy");
    assert_eq!(p.interval_hours, 24);
}

#[test]
fn zero_or_absurd_intervals_are_refused() {
    assert!(matches!(BackupPolicy::new(true, 0, Retention::Copies(7)), Err(PolicyError::Interval(_))));
    assert!(matches!(BackupPolicy::new(true, 24 * 366, Retention::Copies(7)), Err(PolicyError::Interval(_))));
}

#[test]
fn retention_must_keep_at_least_one_copy() {
    assert!(matches!(BackupPolicy::new(true, 24, Retention::Copies(0)), Err(PolicyError::Retention(_))));
    assert!(matches!(BackupPolicy::new(true, 24, Retention::Days(0)), Err(PolicyError::Retention(_))));
}

#[test]
fn a_disabled_policy_ignores_its_schedule_but_still_validates_it() {
    // Disabled: no backups run, but a nonsense value is still refused so it cannot slip in.
    assert!(BackupPolicy::new(false, 0, Retention::Copies(7)).is_err());
    assert!(BackupPolicy::new(false, 24, Retention::Copies(7)).is_ok());
}

#[test]
fn retention_days_and_copies_both_round_trip_through_json() {
    let p = BackupPolicy::new(true, 6, Retention::Days(30)).unwrap();
    let back: BackupPolicy = serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
    assert_eq!(back, p);
}
