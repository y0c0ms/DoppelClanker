//! Integration tests for sesync_lock. Each test runs the built binary against a
//! private lock directory (SESYNC_LOCK_DIR), so tests never touch real
//! state and can run in parallel.

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

const BIN: &str = env!("CARGO_BIN_EXE_sesync-lock");

struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Self {
        let p = std::env::temp_dir().join(format!("sesync_lock-test-{}-{name}", std::process::id()));
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

fn run(dir: &Dir, args: &[&str]) -> Output {
    Command::new(BIN)
        .args(args)
        .env("SESYNC_LOCK_DIR", &dir.0)
        .output()
        .expect("run sesync_lock")
}

fn code(o: &Output) -> i32 {
    o.status.code().unwrap_or(-1)
}

/// A pid that is almost certainly not running. Chosen above the typical
/// pid_max on Linux and macOS default configs.
const DEAD_PID: u32 = 4_000_000;

fn self_pid() -> u32 {
    std::process::id()
}

#[test]
fn claim_free_conversation_succeeds() {
    let d = Dir::new("free");
    let o = run(&d, &["claim", "conv1", "claude", &self_pid().to_string(), "t"]);
    assert_eq!(code(&o), 0, "{o:?}");
    assert!(d.lock_file("conv1").exists());
}

#[test]
fn live_holder_blocks_other_harness() {
    let d = Dir::new("live");
    let me = self_pid().to_string();
    assert_eq!(code(&run(&d, &["claim", "c", "claude", &me, "t"])), 0);
    let o = run(&d, &["claim", "c", "omp", "1", "t"]);
    assert_eq!(code(&o), 2);
    assert!(String::from_utf8_lossy(&o.stdout).starts_with("claude "));
}

#[test]
fn same_owner_reclaim_is_reentrant() {
    let d = Dir::new("reentrant");
    let me = self_pid().to_string();
    assert_eq!(code(&run(&d, &["claim", "c", "claude", &me, "t"])), 0);
    assert_eq!(code(&run(&d, &["claim", "c", "claude", &me, "t"])), 0);
}

#[test]
fn stale_dead_pid_is_taken_over() {
    let d = Dir::new("stale");
    fs::write(d.lock_file("c"), format!("claude {DEAD_PID} 1 x\n")).unwrap();
    let o = run(&d, &["claim", "c", "omp", &self_pid().to_string(), "t"]);
    assert_eq!(code(&o), 0, "{o:?}");
    assert!(fs::read_to_string(d.lock_file("c")).unwrap().starts_with("omp "));
}

/// Regression: an empty lock file used to make `check` report free while
/// `claim` refused with holder "? ?", wedging the conversation.
#[test]
fn corrupt_lock_is_free_for_check_and_claimable() {
    let d = Dir::new("corrupt");
    fs::write(d.lock_file("c"), "").unwrap();
    assert_eq!(code(&run(&d, &["check", "c"])), 0, "check must treat corrupt as free");
    let o = run(&d, &["claim", "c", "claude", &self_pid().to_string(), "t"]);
    assert_eq!(code(&o), 0, "claim must reclaim corrupt: {o:?}");
}

#[test]
fn check_reports_live_holder() {
    let d = Dir::new("check");
    assert_eq!(code(&run(&d, &["claim", "c", "claude", &self_pid().to_string(), "t"])), 0);
    let o = run(&d, &["check", "c"]);
    assert_eq!(code(&o), 2);
}

#[test]
fn release_by_non_owner_is_refused() {
    let d = Dir::new("rel-other");
    let me = self_pid().to_string();
    assert_eq!(code(&run(&d, &["claim", "c", "claude", &me, "t"])), 0);
    assert_eq!(code(&run(&d, &["release", "c", "omp", "1"])), 2);
    assert!(d.lock_file("c").exists(), "non-owner release must not delete the lock");
}

#[test]
fn release_by_owner_frees_conversation() {
    let d = Dir::new("rel-owner");
    let me = self_pid().to_string();
    assert_eq!(code(&run(&d, &["claim", "c", "claude", &me, "t"])), 0);
    assert_eq!(code(&run(&d, &["release", "c", "claude", &me])), 0);
    assert_eq!(code(&run(&d, &["check", "c"])), 0);
}

#[test]
fn sweep_removes_stale_and_corrupt_keeps_live() {
    let d = Dir::new("sweep");
    fs::write(d.lock_file("dead"), format!("claude {DEAD_PID} 1 x\n")).unwrap();
    fs::write(d.lock_file("corrupt"), "").unwrap();
    assert_eq!(code(&run(&d, &["claim", "live", "claude", &self_pid().to_string(), "t"])), 0);
    let o = run(&d, &["sweep"]);
    assert_eq!(code(&o), 0);
    assert_eq!(String::from_utf8_lossy(&o.stdout).trim(), "2");
    assert!(!d.lock_file("dead").exists());
    assert!(!d.lock_file("corrupt").exists());
    assert!(d.lock_file("live").exists());
}

#[test]
fn ids_are_sanitised_against_path_escape() {
    let d = Dir::new("escape");
    let o = run(&d, &["claim", "../../evil", "claude", &self_pid().to_string(), "t"]);
    assert_eq!(code(&o), 0);
    assert!(d.lock_file("evil").exists(), "traversal chars must be stripped");
}
