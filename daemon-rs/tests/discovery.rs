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

#[test]
fn claude_subagent_files_are_found_under_their_parent_session() {
    // <projects>/<bucket>/<parent-uuid>/subagents/agent-<id>.jsonl belongs to the parent.
    let root = tmp("claude-sub");
    let parent = "ec447302-155c-4280-9c34-2b5122fcd4a4";
    let sub = root.join("projects").join("-proj").join(parent).join("subagents");
    fs::create_dir_all(&sub).unwrap();
    let a = sub.join("agent-a1.jsonl");
    fs::write(&a, "").unwrap();
    fs::write(sub.join("notes.txt"), "").unwrap();
    let found = harness_sync::discovery::claude_subagents(&root);
    assert_eq!(found, vec![(parent.to_string(), a)]);
}

#[test]
fn gemini_subagent_files_are_found_under_their_parent_session() {
    let root = tmp("gemini-sub2");
    let parent = "0f3c9a12-1111-4222-8333-444455556666";
    let sub = root.join("tmp").join("proj").join("chats").join(parent);
    fs::create_dir_all(&sub).unwrap();
    let a = sub.join("sub-1.jsonl");
    fs::write(&a, "").unwrap();
    assert_eq!(harness_sync::discovery::gemini_subagents(&root), vec![(parent.to_string(), a)]);
}

#[test]
fn subagent_files_never_appear_as_main_sessions() {
    // The main-session lists must not include anything under a subagents folder, so a
    // subagent run can never become a conversation of its own.
    let root = tmp("claude-main-excl");
    let parent = "ec447302-155c-4280-9c34-2b5122fcd4a4";
    let sub = root.join("projects").join("-proj").join(parent).join("subagents");
    fs::create_dir_all(&sub).unwrap();
    fs::write(sub.join("agent-a1.jsonl"), "").unwrap();
    assert!(claude_sessions(&root).iter().all(|p| !p.to_string_lossy().contains("subagents")));
}

#[test]
fn gemini_session_files_are_found_under_each_project_chats_folder() {
    // ~/.gemini/tmp/<slug>/chats/session-<ts>-<id>.jsonl is a Gemini session; other files
    // in the project folder (logs.json, checkpoints) are not.
    let root = tmp("gemini");
    let chats = root.join("tmp").join("proj").join("chats");
    fs::create_dir_all(&chats).unwrap();
    let good = chats.join("session-2026-10-16T10-00-0f3c9a12.jsonl");
    fs::write(&good, "").unwrap();
    fs::write(root.join("tmp").join("proj").join("logs.json"), "[]").unwrap();
    fs::write(chats.join("notes.txt"), "").unwrap();
    assert_eq!(harness_sync::discovery::gemini_sessions(&root), vec![good]);
}

#[test]
fn a_gemini_subagent_file_is_not_a_main_session() {
    // Subagent sessions live in chats/<parentSessionId>/<id>.jsonl; they belong to their
    // parent and are not listed as conversations of their own.
    let root = tmp("gemini-sub");
    let chats = root.join("tmp").join("proj").join("chats");
    let sub = chats.join("0f3c9a12-1111-4222-8333-444455556666");
    fs::create_dir_all(&sub).unwrap();
    fs::write(sub.join("agent-1.jsonl"), "").unwrap();
    assert!(harness_sync::discovery::gemini_sessions(&root).is_empty());
}
