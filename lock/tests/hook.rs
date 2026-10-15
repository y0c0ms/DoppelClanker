//! Tests for `hslock hook <event> <harness>`: the port of hooks/harness-sync-lock.ts.
//! The hook reads a JSON event on stdin and prints a decision on stdout.
//! Each test uses a private lock directory, so the real state is never touched.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

const BIN: &str = env!("CARGO_BIN_EXE_hslock");

struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Self {
        let p = std::env::temp_dir().join(format!("hslock-hook-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        Dir(p)
    }
    fn lock_file(&self, id: &str) -> PathBuf {
        self.0.join(format!("{id}.lock"))
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn hook(dir: &Dir, args: &[&str], stdin: &str) -> Output {
    let mut child = Command::new(BIN)
        .arg("hook")
        .args(args)
        .env("HARNESS_SYNC_LOCK_DIR", &dir.0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run hslock hook");
    child.stdin.take().unwrap().write_all(stdin.as_bytes()).unwrap();
    child.wait_with_output().unwrap()
}

fn code(o: &Output) -> i32 {
    o.status.code().unwrap_or(-1)
}

const DEAD_PID: u32 = 4_000_000;

#[test]
fn claim_on_turn_start_takes_the_lock() {
    let d = Dir::new("claim");
    let pid = std::process::id().to_string();
    let o = hook(&d, &["turn_start", "omp", "--pid", &pid, "--session", "conv1"], "{}");
    assert_eq!(code(&o), 0, "{o:?}");
    assert!(d.lock_file("conv1").exists(), "turn_start must claim the conversation");
}

#[test]
fn refused_claim_exits_2_and_names_the_holder() {
    let d = Dir::new("refused");
    let me = std::process::id().to_string();
    // Another live harness holds it: this process stands in for a live holder.
    let first = hook(&d, &["turn_start", "claude", "--pid", &me, "--session", "conv1"], "{}");
    assert_eq!(code(&first), 0, "{first:?}");
    let second = hook(&d, &["turn_start", "omp", "--pid", "1234", "--session", "conv1"], "{}");
    assert_eq!(code(&second), 2, "a refused claim must exit 2 so the harness blocks the turn");
    let out = String::from_utf8_lossy(&second.stdout);
    assert!(out.contains("claude"), "the refusal must name the holder, got: {out}");
}

#[test]
fn release_on_turn_end_frees_the_lock() {
    let d = Dir::new("release");
    let pid = std::process::id().to_string();
    assert_eq!(code(&hook(&d, &["turn_start", "omp", "--pid", &pid, "--session", "conv1"], "{}")), 0);
    let o = hook(&d, &["turn_end", "omp", "--pid", &pid, "--session", "conv1"], "{}");
    assert_eq!(code(&o), 0, "{o:?}");
    assert!(!d.lock_file("conv1").exists(), "turn_end must release the lock");
}

#[test]
fn assistant_message_end_releases_but_user_message_end_does_not() {
    let d = Dir::new("msgend");
    let pid = std::process::id().to_string();
    assert_eq!(code(&hook(&d, &["turn_start", "omp", "--pid", &pid, "--session", "conv1"], "{}")), 0);
    let user = hook(&d, &["message_end", "omp", "--pid", &pid, "--session", "conv1"],
                    r#"{"message":{"role":"user"}}"#);
    assert_eq!(code(&user), 0);
    assert!(d.lock_file("conv1").exists(), "a user message_end must not release");
    let asst = hook(&d, &["message_end", "omp", "--pid", &pid, "--session", "conv1"],
                    r#"{"message":{"role":"assistant"}}"#);
    assert_eq!(code(&asst), 0);
    assert!(!d.lock_file("conv1").exists(), "an assistant message_end must release");
}

#[test]
fn agent_end_that_will_continue_keeps_the_lock() {
    let d = Dir::new("continue");
    let pid = std::process::id().to_string();
    assert_eq!(code(&hook(&d, &["turn_start", "omp", "--pid", &pid, "--session", "conv1"], "{}")), 0);
    let o = hook(&d, &["agent_end", "omp", "--pid", &pid, "--session", "conv1"], r#"{"willContinue":true}"#);
    assert_eq!(code(&o), 0);
    assert!(d.lock_file("conv1").exists(), "auto-retry pending is still our turn");
}

#[test]
fn no_session_id_takes_no_lock_and_exits_0() {
    // --no-session: the hook has no session file, so the conversation is
    // deliberately unprotected (decided on #15). It must not error or block.
    let d = Dir::new("nosession");
    let pid = std::process::id().to_string();
    let o = hook(&d, &["turn_start", "omp", "--pid", &pid], "{}");
    assert_eq!(code(&o), 0, "{o:?}");
    let entries: Vec<_> = fs::read_dir(&d.0).unwrap().collect();
    assert!(entries.is_empty(), "no lock file may be written without a session id");
}

#[test]
fn dead_pid_claim_is_taken_over() {
    let d = Dir::new("stale");
    let dead = DEAD_PID.to_string();
    assert_eq!(code(&hook(&d, &["turn_start", "omp", "--pid", &dead, "--session", "conv1"], "{}")), 0);
    let o = hook(&d, &["turn_start", "claude", "--pid", &std::process::id().to_string(), "--session", "conv1"], "{}");
    assert_eq!(code(&o), 0, "a lock held by a dead pid must not block: {o:?}");
}
