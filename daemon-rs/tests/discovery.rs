//! Discovery, ported from daemon/harness_sync.py jsonl_sessions / claude_sessions.
//! Only files with the exact native names are sessions; anything else is ignored.

use harness_sync::discovery::{claude_sessions, jsonl_sessions, parse_claude_name, parse_omp_name};
use std::fs;
use std::path::PathBuf;

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("hs-disc-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

const ID: &str = "aaaaaaaa-1111-2222-3333-444444444444";

#[test]
fn omp_name_needs_timestamp_and_uuid() {
    assert!(parse_omp_name(&format!("2026-10-01T10-00-00-000Z_{ID}.jsonl")).is_some());
    assert!(parse_omp_name(&format!("{ID}.jsonl")).is_none(), "no timestamp prefix");
    assert!(parse_omp_name(&format!("2026-10-01T10-00-00-000Z_{ID}.json")).is_none(), "wrong extension");
}

#[test]
fn omp_name_rejects_a_bad_timestamp_even_with_a_valid_uuid() {
    // The uuid is valid; only the timestamp is wrong. This pins the timestamp check.
    assert!(parse_omp_name(&format!("not-a-timestamp_{ID}.jsonl")).is_none());
    assert!(parse_omp_name(&format!("2026-10-01T10:00:00.000Z_{ID}.jsonl")).is_none(), "colons instead of dashes");
}

#[test]
fn claude_name_is_a_bare_uuid_jsonl() {
    assert!(parse_claude_name(&format!("{ID}.jsonl")).is_some());
    assert!(parse_claude_name("notes.jsonl").is_none());
    assert!(parse_claude_name(&format!("agent-{ID}.jsonl")).is_none(), "subagent files are not sessions");
}

#[test]
fn omp_sessions_are_found_one_directory_deep_only() {
    let root = tmp("omp");
    let sess = root.join("sessions").join("-tmp-proj");
    fs::create_dir_all(&sess).unwrap();
    let good = sess.join(format!("2026-10-01T10-00-00-000Z_{ID}.jsonl"));
    fs::write(&good, "").unwrap();
    fs::write(sess.join("junk.jsonl"), "").unwrap();
    fs::create_dir_all(root.join("sessions").join("a").join("b")).unwrap();
    fs::write(root.join("sessions").join("a").join("b").join(format!("2026-10-01T10-00-00-000Z_{ID}.jsonl")), "").unwrap();
    let found = jsonl_sessions(&root);
    assert_eq!(found, vec![good], "only the native name, one level deep");
}

#[test]
fn missing_agent_dir_is_empty_not_an_error() {
    assert!(jsonl_sessions(&tmp("none").join("absent")).is_empty());
}

#[test]
fn claude_sessions_are_found_in_projects() {
    let root = tmp("claude");
    let proj = root.join("projects").join("-home-x");
    fs::create_dir_all(&proj).unwrap();
    let good = proj.join(format!("{ID}.jsonl"));
    fs::write(&good, "").unwrap();
    fs::write(proj.join("memo.jsonl"), "").unwrap();
    assert_eq!(claude_sessions(&root), vec![good]);
}
