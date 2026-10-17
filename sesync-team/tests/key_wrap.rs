//! Space keys are stored wrapped under a master key that comes from the cluster's sealed
//! secret. The database holds only the wrapped form: without the master key, a stored space
//! key opens nothing, and a wrapped key cannot be tampered with unnoticed.

use sesync_team::spaces::Space;
use sesync_team::wrap::{unwrap_space, wrap_space, WrapError};

const MASTER: [u8; 32] = [9u8; 32];

#[test]
fn a_space_round_trips_through_wrapping() {
    let mut s = Space::new("team-alpha", &["alice", "bob"]);
    let sealed = s.seal_for("alice", b"session text").unwrap();
    let wrapped = wrap_space(&s, &MASTER).unwrap();
    let back = unwrap_space(&wrapped, &MASTER).unwrap();
    assert_eq!(back.open_for("bob", &sealed).unwrap(), b"session text".to_vec());
}

#[test]
fn the_wrapped_form_does_not_contain_the_raw_key() {
    let s = Space::new("team-alpha", &["alice"]);
    let raw_key = s.key_bytes(1).unwrap();
    let wrapped = wrap_space(&s, &MASTER).unwrap();
    assert!(!wrapped.windows(32).any(|w| w == raw_key), "the database must not see a raw space key");
}

#[test]
fn a_wrong_master_key_cannot_unwrap_a_space() {
    let s = Space::new("team-alpha", &["alice"]);
    let wrapped = wrap_space(&s, &MASTER).unwrap();
    assert!(matches!(unwrap_space(&wrapped, &[1u8; 32]), Err(WrapError::Refused)));
}

#[test]
fn a_changed_wrapped_byte_is_refused() {
    let s = Space::new("team-alpha", &["alice"]);
    let mut wrapped = wrap_space(&s, &MASTER).unwrap();
    let last = wrapped.len() - 1;
    wrapped[last] ^= 1;
    assert!(matches!(unwrap_space(&wrapped, &MASTER), Err(WrapError::Refused)));
}
