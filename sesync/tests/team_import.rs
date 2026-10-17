//! Confirmed import: a team session is written into a harness only when the user confirms it,
//! and never over an existing local session. Without confirmation nothing is written.
use sesync::team_import::{import_confirmed, ImportError, ImportOutcome};

fn sandbox(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("sesync-import-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn an_unconfirmed_import_writes_nothing() {
    let d = sandbox("unconfirmed");
    let target = d.join("session.jsonl");
    let r = import_confirmed(&target, b"team body", false);
    assert_eq!(r, Err(ImportError::NotConfirmed));
    assert!(!target.exists(), "no file without confirmation");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_confirmed_import_writes_the_session() {
    let d = sandbox("confirmed");
    let target = d.join("session.jsonl");
    assert_eq!(import_confirmed(&target, b"team body", true), Ok(ImportOutcome::Written));
    assert_eq!(std::fs::read(&target).unwrap(), b"team body");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn an_existing_local_session_is_never_overwritten() {
    let d = sandbox("exists");
    let target = d.join("session.jsonl");
    std::fs::write(&target, b"local work").unwrap();
    let r = import_confirmed(&target, b"team body", true);
    assert_eq!(r, Err(ImportError::WouldOverwrite), "an existing session is kept");
    assert_eq!(std::fs::read(&target).unwrap(), b"local work", "local content is untouched");
    let _ = std::fs::remove_dir_all(&d);
}
