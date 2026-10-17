//! Divergence guard, ported from daemon/sesync.py sync_source. When a harness
//! continues a conversation inside a mirror, the mirror's content changes. The daemon
//! must freeze that mirror and never overwrite it. A mirror with no recorded
//! fingerprint (from before fingerprinting) is also frozen, to be safe.

use sesync::divergence::{fingerprint, check_mirror, MirrorCheck};
use std::fs;
use std::path::PathBuf;

fn sandbox(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("hs-div-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn an_untouched_mirror_is_still_current() {
    let sb = sandbox("same");
    let p = sb.join("m.jsonl");
    fs::write(&p, "mirror body").unwrap();
    let fp = fingerprint(&p);
    assert_eq!(check_mirror(&p, &fp), MirrorCheck::Current);
}

#[test]
fn a_mirror_written_by_a_harness_is_frozen() {
    let sb = sandbox("changed");
    let p = sb.join("m.jsonl");
    fs::write(&p, "mirror body").unwrap();
    let fp = fingerprint(&p);
    // A harness continues the conversation: the file grows and its mtime moves.
    std::thread::sleep(std::time::Duration::from_millis(20));
    fs::write(&p, "mirror body plus a new turn").unwrap();
    assert_eq!(check_mirror(&p, &fp), MirrorCheck::Diverged,
               "a continued mirror must be frozen, not refreshed");
}

#[test]
fn a_mirror_with_no_recorded_fingerprint_is_frozen() {
    let sb = sandbox("legacy");
    let p = sb.join("m.jsonl");
    fs::write(&p, "old mirror").unwrap();
    assert_eq!(check_mirror(&p, ""), MirrorCheck::Diverged,
               "a mirror predating fingerprints cannot be proven untouched, so it is frozen");
}

#[test]
fn a_missing_mirror_is_reported_missing_not_current() {
    let sb = sandbox("gone");
    let p = sb.join("gone.jsonl");
    let fp = "123:456";
    assert_eq!(check_mirror(&p, fp), MirrorCheck::Missing);
}
