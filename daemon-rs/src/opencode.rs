//! O-penCode as a source. Ported from daemon/harness_sync.py opencode_sessions and
//! opencode_first_prompt. The store is opened read-only; the reader never writes.

use rusqlite::{Connection, OpenFlags};
use serde_json::Value;
use std::path::Path;

pub struct Session {
    pub id: String,
    /// Last update, in seconds since the epoch (the store keeps milliseconds).
    pub updated: f64,
    pub directory: String,
    pub title: String,
}

/// Open read-only. Returns None when the database does not exist; it is never created.
fn open_ro(db: &Path) -> Option<Connection> {
    if !db.exists() {
        return None;
    }
    Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX).ok()
}

/// Live (not archived) sessions.
pub fn sessions(db: &Path) -> Vec<Session> {
    let Some(con) = open_ro(db) else { return Vec::new() };
    let Ok(mut stmt) = con.prepare(
        "select id, time_updated, directory, title from session where time_archived is null",
    ) else {
        return Vec::new();
    };
    let rows = stmt.query_map([], |r| {
        Ok(Session {
            id: r.get::<_, String>(0)?,
            updated: r.get::<_, i64>(1)? as f64 / 1000.0,
            directory: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
            title: r.get::<_, Option<String>>(3)?.unwrap_or_default(),
        })
    });
    match rows {
        Ok(it) => it.flatten().collect(),
        Err(_) => Vec::new(),
    }
}

/// The first real user prompt of a session: its first line, at most 80 characters.
/// Text that starts with '<' is a command wrapper, not a prompt. Empty when none.
pub fn first_prompt(db: &Path, session_id: &str) -> String {
    let Some(con) = open_ro(db) else { return String::new() };
    let Ok(mut stmt) = con.prepare(
        "select m.data, p.data from message m join part p on p.message_id = m.id \
         where m.session_id = ?1 order by m.time_created, p.time_created",
    ) else {
        return String::new();
    };
    let Ok(rows) = stmt.query_map([session_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))) else {
        return String::new();
    };
    for (msg_json, part_json) in rows.flatten() {
        let (Ok(msg), Ok(part)) = (serde_json::from_str::<Value>(&msg_json), serde_json::from_str::<Value>(&part_json)) else {
            continue;
        };
        if msg.get("role").and_then(Value::as_str) != Some("user")
            || part.get("type").and_then(Value::as_str) != Some("text")
        {
            continue;
        }
        let text = part.get("text").and_then(Value::as_str).unwrap_or("").trim();
        if !text.is_empty() && !text.starts_with('<') {
            return text.split('\n').next().unwrap_or("").chars().take(80).collect();
        }
    }
    String::new()
}

/// Test hook: attempt a write through a connection opened the way the reader opens
/// its store. Returns the error the database gives, which must be an error.
pub fn write_probe(db: &Path) -> Result<(), String> {
    let con = open_ro(db).ok_or("no database")?;
    con.execute("insert into session (id, directory, title, time_updated) values ('x','/','x',0)", [])
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Export a session with the opencode CLI and convert it to an omp-format journal
/// with `hsconv from-opencode`, under `scratch`. The export is kept as-is because
/// the CLI can print a banner before the JSON; the converter skips anything before
/// the first '{'. A failed export or conversion is an error.
pub fn export_and_convert(
    opencode: &Path,
    hsconv: &Path,
    session_id: &str,
    scratch: &Path,
) -> Result<std::path::PathBuf, String> {
    use std::process::Command;
    std::fs::create_dir_all(scratch).map_err(|e| e.to_string())?;
    let export = scratch.join(format!("{session_id}.export.json"));
    let journal = scratch.join(format!("{session_id}.journal.jsonl"));

    // Stdout goes straight to the export file, not a pipe. With a pipe the CLI
    // stopped after 196608 bytes and still exited 0, which truncated a 2.7 MB session.
    let file = std::fs::File::create(&export).map_err(|e| e.to_string())?;
    let out = Command::new(opencode)
        .args(["export", session_id])
        .stdout(file)
        .stderr(std::process::Stdio::piped())
        .output()
        .map_err(|e| format!("cannot run opencode export: {e}"))?;
    if !out.status.success() {
        let _ = std::fs::remove_file(&export);
        return Err(format!("opencode export failed: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }

    let conv = Command::new(hsconv)
        .args(["from-opencode"])
        .arg(&export)
        .arg("--out")
        .arg(&journal)
        .output()
        .map_err(|e| format!("cannot run hsconv: {e}"))?;
    let _ = std::fs::remove_file(&export);
    if !conv.status.success() {
        let _ = std::fs::remove_file(&journal);
        return Err(format!("hsconv from-opencode failed: {}", String::from_utf8_lossy(&conv.stderr).trim()));
    }
    Ok(journal)
}
