//! End to end: a harness continues a conversation inside a mirror. The next pass,
//! after the source changes, must not overwrite that mirror. Real hsconv, sandbox
//! stores only.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const BIN: &str = env!("CARGO_BIN_EXE_harness-sync");

fn hsconv() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../hsconv/target/release/hsconv")
}

fn sandbox(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("hs-cliDiv-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

fn run(sb: &Path) -> Output {
    Command::new(BIN)
        .arg("--once")
        .env("HARNESS_SYNC_DIR", sb.join("state"))
        .env("OMP_AGENT_DIR", sb.join("omp"))
        .env("PI_AGENT_DIR", sb.join("pi"))
        .env("CLAUDE_CONFIG_DIR", sb.join("claude"))
        .env("OPENCODE_DB", sb.join("no-opencode").join("opencode.db"))
        .env("HSCONV", hsconv())
        .env("HSLOCK", sb.join("no-hslock"))
        .env("HARNESS_SYNC_QUIET", "0")
        .output()
        .expect("run harness-sync")
}

fn session_file(sb: &Path, id: &str, body: &str) -> PathBuf {
    let dir = sb.join("omp").join("sessions").join("-tmp-proj");
    fs::create_dir_all(&dir).unwrap();
    let p = dir.join(format!("2026-10-01T10-00-00-000Z_{id}.jsonl"));
    let header = format!(r#"{{"type":"session","version":3,"id":"{id}","timestamp":"2026-10-01T10:00:00.000Z","cwd":"/tmp/proj"}}"#);
    let msg = format!(r#"{{"type":"message","id":"m1","parentId":null,"timestamp":"2026-10-01T10:00:01.000Z","message":{{"role":"user","content":"{body}"}}}}"#);
    fs::write(&p, format!("{header}\n{msg}\n")).unwrap();
    p
}

fn pi_mirror(sb: &Path) -> PathBuf {
    let mut found = Vec::new();
    if let Ok(rd) = fs::read_dir(sb.join("pi").join("sessions")) {
        for b in rd.flatten() {
            if let Ok(files) = fs::read_dir(b.path()) {
                found.extend(files.flatten().map(|f| f.path()));
            }
        }
    }
    assert_eq!(found.len(), 1, "exactly one pi mirror expected, got {found:?}");
    found.remove(0)
}

#[test]
fn a_mirror_continued_by_a_harness_is_not_overwritten() {
    let sb = sandbox("freeze");
    fs::create_dir_all(sb.join("pi").join("sessions")).unwrap();
    fs::create_dir_all(sb.join("claude").join("projects")).unwrap();
    let id = "aaaaaaaa-1111-2222-3333-444444444444";
    let src = session_file(&sb, id, &"first ".repeat(120));
    assert!(run(&sb).status.success());
    let mirror = pi_mirror(&sb);

    // A harness continues the conversation inside the pi mirror.
    std::thread::sleep(std::time::Duration::from_millis(20));
    let harness_text = "HARNESS WROTE THIS TURN";
    fs::write(&mirror, format!("{}\n{harness_text}\n", fs::read_to_string(&mirror).unwrap())).unwrap();

    // The source changes too, so the next pass wants to refresh the mirror.
    std::thread::sleep(std::time::Duration::from_millis(20));
    session_file(&sb, id, &"second ".repeat(120));
    let _ = &src;
    assert!(run(&sb).status.success());

    let after = fs::read_to_string(&mirror).unwrap();
    assert!(after.contains(harness_text), "the harness's turn must survive; the mirror was overwritten");
}
