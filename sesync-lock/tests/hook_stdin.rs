//! The hook must behave the same whether or not it reads its stdin. A hook that exits
//! before reading (no session id returns early) closes the pipe; the writer then gets a
//! broken pipe. That must not fail the caller, and the exit code must still be the
//! hook's own. Previously this was an unwrap that panicked on CI (broken pipe).

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

const BIN: &str = env!("CARGO_BIN_EXE_sesync-lock");

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("sesync_lock-stdin-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn a_hook_that_exits_without_reading_stdin_still_exits_zero() {
    // No --session: the hook returns before reading. Write a large payload so the pipe
    // is certainly closed before the write finishes.
    let lock_dir = dir("early-exit");
    let mut child = Command::new(BIN)
        .args(["hook", "turn_start", "omp", "--pid", "1"])
        .env("SESYNC_LOCK_DIR", &lock_dir)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().unwrap();
    let payload = "x".repeat(1 << 20);
    let mut stdin = child.stdin.take().unwrap();
    // The write may fail with a broken pipe; that is the case under test, not an error.
    let _ = stdin.write_all(payload.as_bytes());
    drop(stdin);
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(0), "the hook's own exit code must be 0");
    assert!(std::fs::read_dir(&lock_dir).unwrap().next().is_none(), "no lock may be written");
}

#[test]
fn a_hook_that_reads_stdin_and_claims_still_works() {
    let lock_dir = dir("reads");
    let mut child = Command::new(BIN)
        .args(["hook", "agent_end", "omp", "--pid", "1", "--session", "aaaaaaaa-1111-2222-3333-444444444444"])
        .env("SESYNC_LOCK_DIR", &lock_dir)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().unwrap();
    child.stdin.take().unwrap().write_all(br#"{"willContinue":true}"#).unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(0));
}
