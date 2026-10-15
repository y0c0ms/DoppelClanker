//! O-penCode as a source, ported from daemon/harness_sync.py opencode_sessions and
//! opencode_first_prompt. Reads the real schema (session, message, part) read-only.
//! Tests use a database built with the same table and column names in a sandbox.

use harness_sync::opencode::{first_prompt, sessions};
use rusqlite::Connection;
use std::fs;
use std::path::PathBuf;

fn sandbox(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("hs-oc-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

fn make_db(path: &PathBuf) {
    let c = Connection::open(path).unwrap();
    c.execute_batch(
        "create table session (id text primary key, directory text, title text,
            time_updated integer, time_archived integer);
         create table message (id text primary key, session_id text, time_created integer, data text);
         create table part (id text primary key, message_id text, time_created integer, data text);
         insert into session values ('ses_live', '/proj', 'Live title', 1792083252020, null);
         insert into session values ('ses_archived', '/proj', 'Old', 1792083252020, 1);
         insert into message values ('m1', 'ses_live', 1, '{\"role\":\"user\"}');
         insert into part values ('p1', 'm1', 1, '{\"type\":\"text\",\"text\":\"first question here\\nmore\"}');
         insert into message values ('m2', 'ses_noprompt', 1, '{\"role\":\"user\"}');
         insert into part values ('p2', 'm2', 1, '{\"type\":\"text\",\"text\":\"<command-name>x\"}');",
    ).unwrap();
}

#[test]
fn live_sessions_are_listed_archived_ones_are_not() {
    let sb = sandbox("list");
    let db = sb.join("opencode.db");
    make_db(&db);
    let got = sessions(&db);
    let ids: Vec<&str> = got.iter().map(|s| s.id.as_str()).collect();
    assert!(ids.contains(&"ses_live"), "{ids:?}");
    assert!(!ids.contains(&"ses_archived"), "archived sessions are not synced: {ids:?}");
}

#[test]
fn updated_time_is_seconds_not_milliseconds() {
    let sb = sandbox("time");
    let db = sb.join("opencode.db");
    make_db(&db);
    let s = sessions(&db).into_iter().find(|s| s.id == "ses_live").unwrap();
    assert!((s.updated - 1792083252.020).abs() < 0.001, "got {}", s.updated);
}

#[test]
fn first_prompt_is_the_first_real_user_text_line() {
    let sb = sandbox("prompt");
    let db = sb.join("opencode.db");
    make_db(&db);
    assert_eq!(first_prompt(&db, "ses_live"), "first question here");
}

#[test]
fn command_wrappers_are_not_a_prompt() {
    let sb = sandbox("wrapper");
    let db = sb.join("opencode.db");
    make_db(&db);
    assert_eq!(first_prompt(&db, "ses_noprompt"), "", "text starting with '<' is a wrapper, not a prompt");
}

#[test]
fn a_missing_database_gives_nothing_and_is_not_created() {
    let sb = sandbox("missing");
    let db = sb.join("absent.db");
    assert!(sessions(&db).is_empty());
    assert_eq!(first_prompt(&db, "ses_x"), "");
    assert!(!db.exists(), "reading must never create the database");
}

#[test]
fn the_database_is_opened_read_only() {
    let sb = sandbox("ro");
    let db = sb.join("opencode.db");
    make_db(&db);
    // A write through the same path the reader uses must fail, so the reader cannot
    // change the store. This pins the mode=ro open, not just the queries.
    assert!(harness_sync::opencode::write_probe(&db).is_err(), "the reader's connection must be read-only");
}
