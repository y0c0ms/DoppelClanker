//! Authenticator join. A 6-digit RFC 6238 code, 30-second steps, SHA-1 (what every
//! authenticator app supports). Each code is accepted once. Five wrong codes lock joining
//! for 15 minutes. The check proves the user holds the authenticator, not the key: the
//! encryption key still moves by key-wrap.

use harness_sync_team::totp::{code_at, JoinGate, JoinError, STEP_SECS, LOCK_SECS, MAX_FAILURES};

/// RFC 6238 test vector: secret "12345678901234567890" (ASCII), time 59 -> 8-digit 94287082;
/// the 6-digit form is the last six digits: 287082.
#[test]
fn matches_the_rfc_6238_test_vector() {
    let secret = b"12345678901234567890";
    assert_eq!(code_at(secret, 59), "287082");
}

#[test]
fn a_code_is_valid_for_its_thirty_second_step() {
    let secret = b"12345678901234567890";
    // Start on a step boundary: t + STEP_SECS - 1 must still be the same step.
    let t = 1_000_020u64; // 1_000_020 = 33334 * 30
    assert_eq!(t % STEP_SECS, 0);
    let c = code_at(secret, t);
    assert_eq!(c.len(), 6);
    assert_eq!(code_at(secret, t + STEP_SECS - 1), c, "same step, same code");
    assert_ne!(code_at(secret, t + STEP_SECS), c, "next step, new code");
}

#[test]
fn a_correct_code_joins_once_and_is_not_accepted_again() {
    let secret = b"12345678901234567890";
    let mut g = JoinGate::new(secret.to_vec());
    let t = 1_000_000u64;
    let c = code_at(secret, t);
    g.verify(&c, t).expect("a correct code joins");
    assert_eq!(g.verify(&c, t), Err(JoinError::AlreadyUsed), "the same code cannot be used twice");
}

#[test]
fn a_wrong_code_is_refused() {
    let mut g = JoinGate::new(b"12345678901234567890".to_vec());
    assert_eq!(g.verify("000000", 1_000_000), Err(JoinError::Wrong));
}

#[test]
fn five_wrong_codes_lock_joining_for_fifteen_minutes() {
    assert_eq!(MAX_FAILURES, 5);
    assert_eq!(LOCK_SECS, 15 * 60);
    let secret = b"12345678901234567890";
    let mut g = JoinGate::new(secret.to_vec());
    let t = 1_000_000u64;
    for _ in 0..MAX_FAILURES {
        let _ = g.verify("000000", t);
    }
    // Even the correct code is refused while locked.
    let good_now = code_at(secret, t + 1);
    assert!(matches!(g.verify(&good_now, t + 1), Err(JoinError::Locked { .. })), "locked: even the right code is refused");
    // After the lock ends, the code for that moment is accepted.
    let after = t + LOCK_SECS + 1;
    assert!(g.verify(&code_at(secret, after), after).is_ok(), "the lock ends after 15 minutes");
}

#[test]
fn a_success_clears_the_failure_count() {
    let secret = b"12345678901234567890";
    let mut g = JoinGate::new(secret.to_vec());
    let t = 1_000_000u64;
    for _ in 0..(MAX_FAILURES - 1) {
        let _ = g.verify("000000", t);
    }
    g.verify(&code_at(secret, t), t).unwrap();
    // Four more wrong codes must not lock now: the count was cleared.
    for _ in 0..(MAX_FAILURES - 1) {
        let _ = g.verify("000000", t + STEP_SECS);
    }
    assert!(!matches!(g.verify("000000", t + STEP_SECS), Err(JoinError::Locked { .. })));
}
