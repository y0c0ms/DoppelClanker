//! The daemon's lock client, ported from daemon/sesync.py lock_holder and
//! sync_source. Uses the real sesync_lock binary with a private lock directory.
//! Behaviour pinned here: a missing binary never blocks; a refused claim skips the
//! source; the daemon always releases what it claimed.

use sesync::lock::{holder_of, with_claim, ClaimOutcome};
use std::fs;
use std::path::PathBuf;

fn sesync_lock() -> PathBuf {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../sesync-lock/target/debug/sesync-lock");
    assert!(p.exists(), "build lock first: cargo build --manifest-path sesync-lock/Cargo.toml");
    p
}

/// A private lock directory for one test. Each test sets the variable on its own
/// child process only, so tests cannot see each other's locks when run in parallel.
fn lock_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("hs-lockc-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn holder_is_none_when_free() {
    let d = lock_dir("free");
    assert_eq!(holder_of(&sesync_lock(), Some(&d), "conv-free"), None);
}

#[test]
fn missing_binary_never_blocks() {
    let d = lock_dir("missing");
    let missing = PathBuf::from("/nonexistent/sesync_lock");
    assert_eq!(holder_of(&missing, Some(&d), "conv"), None);
    let ran = with_claim(&missing, Some(&d), "conv", "daemon", 1, "/src", || 42);
    assert_eq!(ran, ClaimOutcome::Ran(42), "a missing lock binary must not skip the sync");
}

#[test]
fn refused_claim_skips_the_work_and_names_the_holder() {
    let d = lock_dir("refused");
    let bin = sesync_lock();
    // Another harness claims it first.
    let other = std::process::Command::new(&bin)
        .env("SESYNC_LOCK_DIR", &d)
        .args(["claim", "conv-r", "claude", &std::process::id().to_string(), "t"])
        .output()
        .unwrap();
    assert!(other.status.success());
    let mut ran = false;
    let out = with_claim(&bin, Some(&d), "conv-r", "daemon", 1, "/src", || ran = true);
    assert!(!ran, "the work must not run while another harness holds the conversation");
    match out {
        ClaimOutcome::Skipped(holder) => assert!(holder.starts_with("claude"), "got {holder}"),
        other => panic!("expected Skipped, got {other:?}"),
    }
}

#[test]
fn claim_is_released_after_the_work() {
    let d = lock_dir("release");
    let bin = sesync_lock();
    let me = std::process::id();
    let out = with_claim(&bin, Some(&d), "conv-x", "daemon", me, "/src", || 7);
    assert_eq!(out, ClaimOutcome::Ran(7));
    assert!(!d.join("conv-x.lock").exists(), "the daemon must release what it claimed");
}

#[test]
fn claim_is_released_even_when_the_work_panics() {
    let d = lock_dir("panic");
    let bin = sesync_lock();
    let me = std::process::id();
    let r = std::panic::catch_unwind(|| {
        with_claim(&bin, Some(&d), "conv-p", "daemon", me, "/src", || panic!("boom"));
    });
    assert!(r.is_err());
    assert!(!d.join("conv-p.lock").exists(), "a panic must not leave the conversation locked");
}
