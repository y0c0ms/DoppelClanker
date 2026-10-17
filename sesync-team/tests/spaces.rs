//! Spaces: each space has its own content key. A space's key does not open another space.
//! Removing a member rotates the key, so the removed member cannot read anything written
//! afterwards, while the remaining members keep access.

use sesync_team::spaces::{open, seal, Space, SpaceError};

#[test]
fn content_round_trips_inside_a_space() {
    let mut s = Space::new("team-alpha", &["alice", "bob"]);
    let sealed = s.seal_for("alice", b"session text").unwrap();
    assert_ne!(sealed, b"session text".to_vec(), "the stored bytes must not be plaintext");
    assert_eq!(s.open_for("bob", &sealed).unwrap(), b"session text".to_vec());
}

#[test]
fn one_space_cannot_open_another_spaces_content() {
    let mut a = Space::new("team-alpha", &["alice"]);
    let b = Space::new("team-beta", &["alice"]);
    let sealed = a.seal_for("alice", b"alpha only").unwrap();
    assert!(b.open_for("alice", &sealed).is_err(), "a different space's key must not open it");
}

#[test]
fn a_removed_member_cannot_read_content_written_after_removal() {
    let mut s = Space::new("team-alpha", &["alice", "bob"]);
    let before = s.seal_for("alice", b"before removal").unwrap();
    s.remove_member("bob").unwrap();
    let after = s.seal_for("alice", b"after removal").unwrap();
    assert!(matches!(s.open_for("bob", &after), Err(SpaceError::NotMember(_))),
        "bob must not open new content");
    assert_eq!(s.open_for("alice", &after).unwrap(), b"after removal".to_vec(),
        "remaining members keep access");
    assert_eq!(s.open_for("alice", &before).unwrap(), b"before removal".to_vec(),
        "content written before the removal stays readable to remaining members");
}

#[test]
fn removal_rotates_the_key() {
    let mut s = Space::new("team-alpha", &["alice", "bob"]);
    let k1 = s.key_version();
    s.remove_member("bob").unwrap();
    assert!(s.key_version() > k1, "removing a member must rotate the space key");
}

#[test]
fn a_non_member_cannot_seal_into_the_space() {
    let mut s = Space::new("team-alpha", &["alice"]);
    assert!(matches!(s.seal_for("mallory", b"x"), Err(SpaceError::NotMember(_))));
}

#[test]
fn tampered_ciphertext_is_refused() {
    let mut s = Space::new("team-alpha", &["alice"]);
    let mut sealed = s.seal_for("alice", b"integrity matters").unwrap();
    let last = sealed.len() - 1;
    sealed[last] ^= 0x01;
    assert!(s.open_for("alice", &sealed).is_err(), "a changed byte must be refused, not decrypted");
}

#[test]
fn seal_and_open_free_functions_match_the_space_methods() {
    let key = [7u8; 32];
    let sealed = seal(&key, b"nonce-test", b"payload").unwrap();
    assert_eq!(open(&key, &sealed).unwrap(), b"payload".to_vec());
}
