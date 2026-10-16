//! Session copies kept compressed in the archive store. A copy goes in as
//! <name>.hsar and comes back byte for byte. A damaged copy is refused, and a failed
//! read never leaves a partial file behind.

use harness_sync::archive::{pack_file, unpack_file, ArchiveError};
use std::fs;
use std::path::PathBuf;

fn sandbox(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("hs-arcstore-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

fn session_text() -> Vec<u8> {
    let line = r#"{"type":"message","id":"m","message":{"role":"user","content":"repeated prompt text for a session"}}"#;
    let mut out = Vec::new();
    for _ in 0..4000 {
        out.extend_from_slice(line.as_bytes());
        out.push(b'\n');
    }
    out
}

#[test]
fn a_session_copy_round_trips_byte_for_byte_and_is_smaller() {
    let sb = sandbox("roundtrip");
    let src = sb.join("session.jsonl");
    fs::write(&src, session_text()).unwrap();
    let stored = sb.join("store").join("session.jsonl.hsar");
    fs::create_dir_all(stored.parent().unwrap()).unwrap();
    pack_file(&src, &stored, 6).unwrap();
    let back = sb.join("back.jsonl");
    unpack_file(&stored, &back).unwrap();
    assert_eq!(fs::read(&back).unwrap(), session_text(), "the copy must be exact");
    assert!(fs::metadata(&stored).unwrap().len() < fs::metadata(&src).unwrap().len() / 5,
        "repetitive session text must compress well");
}

#[test]
fn a_damaged_stored_copy_is_refused_and_leaves_no_file() {
    let sb = sandbox("damaged");
    let src = sb.join("session.jsonl");
    fs::write(&src, session_text()).unwrap();
    let stored = sb.join("session.jsonl.hsar");
    pack_file(&src, &stored, 6).unwrap();
    let mut bytes = fs::read(&stored).unwrap();
    let mid = bytes.len() / 2;
    bytes[mid] ^= 0xFF;
    fs::write(&stored, &bytes).unwrap();
    let back = sb.join("back.jsonl");
    assert!(unpack_file(&stored, &back).is_err(), "a damaged copy must be refused");
    assert!(!back.exists(), "a refused copy must not leave a file");
}

#[test]
fn a_store_copy_with_the_wrong_format_is_refused() {
    let sb = sandbox("wrongformat");
    let stored = sb.join("not-an-archive.hsar");
    fs::write(&stored, b"plain text, not an archive").unwrap();
    assert!(matches!(unpack_file(&stored, &sb.join("x")), Err(ArchiveError::BadMagic)));
}

#[test]
fn store_session_keeps_a_verified_compressed_copy_under_the_parent() {
    use harness_sync::archive_store::{store_session, read_session};
    let sb = sandbox("store-api");
    let src = sb.join("agent-a1.jsonl");
    fs::write(&src, session_text()).unwrap();
    let store = sb.join("archive");
    let copy = store_session(&store, "ec447302-155c-4280-9c34-2b5122fcd4a4", &src).unwrap();
    assert!(copy.starts_with(store.join("ec447302-155c-4280-9c34-2b5122fcd4a4")));
    assert_eq!(read_session(&copy).unwrap(), session_text(), "the stored copy reads back exactly");
}

#[test]
fn store_session_refuses_a_parent_id_that_is_not_a_plain_name() {
    use harness_sync::archive_store::store_session;
    let sb = sandbox("store-bad-parent");
    let src = sb.join("a.jsonl");
    fs::write(&src, session_text()).unwrap();
    assert!(store_session(&sb.join("archive"), "../escape", &src).is_err(),
        "a parent id must not climb out of the store");
}
