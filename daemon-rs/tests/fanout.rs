//! Every source is mirrored into every installed target, not one. The Python daemon
//! fans out to TARGETS (omp, pi, opencode, claude). An omp session must produce a pi
//! mirror and an O-penCode session, when the O-penCode store exists.
//! Uses the real hsconv and the real opencode CLI; skipped without the CLI.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const BIN: &str = env!("CARGO_BIN_EXE_harness-sync");

fn hsconv() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../hsconv/target/release/hsconv")
}

fn sandbox(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("hs-fanout-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

fn have_cli() -> bool {
    std::env::var_os("PATH").map_or(false, |p| std::env::split_paths(&p).any(|d| d.join("opencode").is_file()))
}

fn run(sb: &Path) -> Output {
    Command::new(BIN)
        .arg("--once")
        .env("HARNESS_SYNC_DIR", sb.join("state"))
        .env("OMP_AGENT_DIR", sb.join("omp"))
        .env("PI_AGENT_DIR", sb.join("pi"))
        .env("CLAUDE_CONFIG_DIR", sb.join("claude"))
        .env("OPENCODE_DB", sb.join("xdg").join("data").join("opencode").join("opencode.db"))
        .env("OPENCODE_BIN", "opencode")
        .env("HSCONV", hsconv())
        .env("HSLOCK", sb.join("no-hslock"))
        .env("HARNESS_SYNC_QUIET", "0")
        .env("XDG_DATA_HOME", sb.join("xdg").join("data"))
        .env("XDG_CONFIG_HOME", sb.join("xdg").join("cfg"))
        .env("XDG_STATE_HOME", sb.join("xdg").join("state"))
        .env("XDG_CACHE_HOME", sb.join("xdg").join("cache"))
        .output()
        .expect("run harness-sync")
}

fn walk(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = fs::read_dir(root) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() { out.extend(walk(&p)); } else if p.extension().map_or(false, |x| x == "jsonl") { out.push(p); }
        }
    }
    out
}

#[test]
fn an_omp_session_is_mirrored_into_pi_and_opencode() {
    if !have_cli() { eprintln!("skipped: opencode CLI not installed"); return; }
    let sb = sandbox("fanout");
    // Source: one omp session, old enough to pass the quiet check (QUIET=0 here).
    let id = "aaaaaaaa-1111-2222-3333-444444444444";
    let dir = sb.join("omp").join("sessions").join("-tmp-proj");
    fs::create_dir_all(&dir).unwrap();
    fs::create_dir_all(sb.join("pi").join("sessions")).unwrap();
    fs::create_dir_all(sb.join("claude").join("projects")).unwrap();
    let msg = format!("{}", "hello fanout ".repeat(60));
    fs::write(dir.join(format!("2026-10-01T10-00-00-000Z_{id}.jsonl")),
        format!("{{\"type\":\"session\",\"version\":3,\"id\":\"{id}\",\"timestamp\":\"2026-10-01T10:00:00.000Z\",\"cwd\":\"/tmp/proj\"}}\n\
{{\"type\":\"message\",\"id\":\"m1\",\"parentId\":null,\"timestamp\":\"2026-10-01T10:00:01.000Z\",\"message\":{{\"role\":\"user\",\"content\":\"{msg}\"}}}}\n")).unwrap();

    // An O-penCode store must exist for that target to be installed.
    let mut init = Command::new("opencode");
    init.args(["session", "list"])
        .env("XDG_DATA_HOME", sb.join("xdg").join("data"))
        .env("XDG_CONFIG_HOME", sb.join("xdg").join("cfg"))
        .env("XDG_STATE_HOME", sb.join("xdg").join("state"))
        .env("XDG_CACHE_HOME", sb.join("xdg").join("cache"));
    assert!(init.output().unwrap().status.success());

    let o = run(&sb);
    assert!(o.status.success(), "stderr: {}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(walk(&sb.join("pi").join("sessions")).len(), 1, "one pi mirror");

    let listed = Command::new("opencode")
        .args(["session", "list"])
        .env("XDG_DATA_HOME", sb.join("xdg").join("data"))
        .env("XDG_CONFIG_HOME", sb.join("xdg").join("cfg"))
        .env("XDG_STATE_HOME", sb.join("xdg").join("state"))
        .env("XDG_CACHE_HOME", sb.join("xdg").join("cache"))
        .output().unwrap();
    let text = String::from_utf8_lossy(&listed.stdout);
    assert!(text.lines().filter(|l| l.starts_with("ses_")).count() >= 1,
            "the omp session must be mirrored into O-penCode too:\n{text}");
}
