//! Finding session files on disk. Ported from daemon/harness_sync.py.
//! A file is a session only if its name matches the native pattern exactly.

use std::fs;
use std::path::{Path, PathBuf};

/// `<timestamp>_<uuid>.jsonl`, the omp and Pi journal name.
pub fn parse_omp_name(name: &str) -> Option<String> {
    let stem = name.strip_suffix(".jsonl")?;
    let (ts, id) = stem.split_once('_')?;
    if !is_timestamp(ts) || !is_uuid(id) {
        return None;
    }
    Some(id.to_string())
}

/// `<uuid>.jsonl`, the Claude Code transcript name. Subagent files are not sessions.
pub fn parse_claude_name(name: &str) -> Option<String> {
    let id = name.strip_suffix(".jsonl")?;
    if is_uuid(id) { Some(id.to_string()) } else { None }
}

/// `2026-10-01T10-00-00-000Z`: digits and dashes in the exact places, then Z.
fn is_timestamp(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 24
        && s.ends_with('Z')
        && b[10] == b'T'
        && [4, 7, 13, 16, 19].iter().all(|&i| b[i] == b'-')
        && [0, 1, 2, 3, 5, 6, 8, 9, 11, 12, 14, 15, 17, 18, 20, 21, 22]
            .iter()
            .all(|&i| b[i].is_ascii_digit())
}

/// 8-4-4-4-12 lowercase hex.
fn is_uuid(s: &str) -> bool {
    let parts: Vec<&str> = s.split('-').collect();
    let lens = [8, 4, 4, 4, 12];
    parts.len() == 5
        && parts.iter().zip(lens).all(|(p, n)| p.len() == n && p.bytes().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()))
}

/// Files one directory below `root`, i.e. the Python glob `*/*.jsonl`.
fn one_level_down(root: &Path, keep: fn(&str) -> bool) -> Vec<PathBuf> {
    let Ok(dirs) = fs::read_dir(root) else { return Vec::new() };
    let mut out = Vec::new();
    for d in dirs.flatten().filter(|d| d.path().is_dir()) {
        let Ok(files) = fs::read_dir(d.path()) else { continue };
        for f in files.flatten() {
            let p = f.path();
            if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
                if p.is_file() && keep(name) {
                    out.push(p);
                }
            }
        }
    }
    out.sort();
    out
}

/// omp or Pi session files under `<agent_dir>/sessions/*/`.
pub fn jsonl_sessions(agent_dir: &Path) -> Vec<PathBuf> {
    one_level_down(&agent_dir.join("sessions"), |n| parse_omp_name(n).is_some())
}

/// Claude Code transcripts under `<claude_dir>/projects/*/`.
pub fn claude_sessions(claude_dir: &Path) -> Vec<PathBuf> {
    one_level_down(&claude_dir.join("projects"), |n| parse_claude_name(n).is_some())
}

/// Claude subagent transcripts: `<projects>/<bucket>/<parent-uuid>/subagents/*.jsonl`.
/// Returns (parent session id, subagent file). They belong to the parent session.
pub fn claude_subagents(claude_dir: &Path) -> Vec<(String, PathBuf)> {
    let Ok(buckets) = fs::read_dir(claude_dir.join("projects")) else { return Vec::new() };
    let mut out = Vec::new();
    for b in buckets.flatten().filter(|b| b.path().is_dir()) {
        let Ok(parents) = fs::read_dir(b.path()) else { continue };
        for p in parents.flatten().filter(|p| p.path().is_dir()) {
            let parent = p.file_name().to_string_lossy().into_owned();
            let sub = p.path().join("subagents");
            let Ok(files) = fs::read_dir(&sub) else { continue };
            for f in files.flatten() {
                let path = f.path();
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if path.is_file() && name.ends_with(".jsonl") {
                    out.push((parent.clone(), path));
                }
            }
        }
    }
    out.sort();
    out
}

/// Gemini CLI subagent sessions: `<gemini>/tmp/<slug>/chats/<parentSessionId>/*.jsonl`.
/// Returns (parent session id, subagent file).
pub fn gemini_subagents(gemini_dir: &Path) -> Vec<(String, PathBuf)> {
    let Ok(projects) = fs::read_dir(gemini_dir.join("tmp")) else { return Vec::new() };
    let mut out = Vec::new();
    for p in projects.flatten().filter(|p| p.path().is_dir()) {
        let Ok(parents) = fs::read_dir(p.path().join("chats")) else { continue };
        for par in parents.flatten().filter(|x| x.path().is_dir()) {
            let parent = par.file_name().to_string_lossy().into_owned();
            let Ok(files) = fs::read_dir(par.path()) else { continue };
            for f in files.flatten() {
                let path = f.path();
                if path.is_file() && path.extension().map_or(false, |e| e == "jsonl") {
                    out.push((parent.clone(), path));
                }
            }
        }
    }
    out.sort();
    out
}

/// Gemini CLI main sessions: `<gemini>/tmp/<slug>/chats/session-*.jsonl`, one directory
/// below each project's `chats` folder. Subagent sessions sit in `chats/<parentId>/` and
/// are not main sessions, so they are not returned.
pub fn gemini_sessions(gemini_dir: &Path) -> Vec<PathBuf> {
    let root = gemini_dir.join("tmp");
    let Ok(projects) = fs::read_dir(&root) else { return Vec::new() };
    let mut out = Vec::new();
    for p in projects.flatten().filter(|p| p.path().is_dir()) {
        let chats = p.path().join("chats");
        let Ok(files) = fs::read_dir(&chats) else { continue };
        for f in files.flatten() {
            let path = f.path();
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if path.is_file() && name.starts_with("session-") && name.ends_with(".jsonl") {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}
