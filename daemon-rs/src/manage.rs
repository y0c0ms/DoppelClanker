//! Session management: list, delete and rename. Ported from daemon/hs_manage.py.
//! Every file operation is limited to the session roots, every operation holds
//! the conversation lock, and mirrors are removed before their source.

use crate::lock::{with_claim, ClaimOutcome};
use crate::state::State;
use std::fs;
use std::path::{Path, PathBuf};

pub const MAX_TITLE_CHARS: usize = 80;

/// The three directories session files may live under.
pub struct Roots {
    pub omp: PathBuf,
    pub pi: PathBuf,
    pub claude: PathBuf,
}

impl Roots {
    fn all(&self) -> [&Path; 3] {
        [&self.omp, &self.pi, &self.claude]
    }

    /// True only when `path` resolves under one of the roots. Canonicalising first
    /// stops `..` and symlinks from escaping.
    pub fn contains(&self, path: &Path) -> bool {
        let Ok(resolved) = path.canonicalize() else { return false };
        self.all().iter().any(|r| r.canonicalize().map(|rr| resolved.starts_with(rr)).unwrap_or(false))
    }
}

#[derive(Debug, Clone)]
pub struct Group {
    pub fmt: String,
    pub key: String,
    pub title: String,
    pub mirrors: Vec<(String, String)>,
}

impl Group {
    pub fn file(fmt: &str, key: String, title: &str) -> Group {
        Group { fmt: fmt.into(), key, title: title.into(), mirrors: Vec::new() }
    }
    pub fn add_mirror(&mut self, target: &str, path: String) {
        self.mirrors.push((target.into(), path));
    }
}

/// Trim and check a new name: 1 to 80 characters, no control characters.
pub fn validate_title(title: &str) -> Result<String, String> {
    let t = title.trim();
    if t.is_empty() {
        return Err("title is empty".into());
    }
    if t.chars().any(|c| (c as u32) < 32 || c as u32 == 127) {
        return Err("title contains control characters".into());
    }
    if t.chars().count() > MAX_TITLE_CHARS {
        return Err(format!("title is longer than {MAX_TITLE_CHARS} characters"));
    }
    Ok(t.to_string())
}

/// Every source on disk, with the mirrors recorded for it in state. Mirror files
/// are never listed as sources. A source only in state (no file on disk) is kept
/// as an orphan so it can still be cleaned up.
pub fn list_groups(roots: &Roots, state: &State) -> Vec<Group> {
    let mut by_key: std::collections::BTreeMap<String, Group> = std::collections::BTreeMap::new();

    // Files on disk: omp and Pi journals, plus Claude transcripts.
    let mut on_disk: Vec<(String, &str)> = Vec::new();
    // The roots are `<agent>/sessions` and `<claude>/projects`; discovery takes the
    // agent or config directory above them, so step up one level.
    let up = |p: &PathBuf| p.parent().map(Path::to_path_buf).unwrap_or_else(|| p.clone());
    for p in crate::discovery::jsonl_sessions(&up(&roots.omp)) {
        on_disk.push((p.display().to_string(), "omp"));
    }
    for p in crate::discovery::jsonl_sessions(&up(&roots.pi)) {
        on_disk.push((p.display().to_string(), "pi"));
    }
    for p in crate::discovery::claude_sessions(&up(&roots.claude)) {
        on_disk.push((p.display().to_string(), "claude"));
    }
    for (key, fmt) in on_disk {
        if state.mirror_keys.contains(&key) {
            continue;
        }
        by_key.insert(key.clone(), Group::file(fmt, key.clone(), &file_name(&key)));
    }

    // Sources recorded in state, with their mirrors. Keep orphans too.
    for (key, src) in &state.sources {
        if state.mirror_keys.contains(key) {
            continue;
        }
        let g = by_key.entry(key.clone()).or_insert_with(|| Group::file(&src.fmt, key.clone(), &file_name(key)));
        for (t, m) in &src.mirrors {
            g.add_mirror(t, m.path.clone());
        }
    }
    by_key.into_values().collect()
}

fn file_name(p: &str) -> String {
    Path::new(p).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.into())
}

/// Delete a source and its mirrors. Refused if the source or any mirror is outside
/// the roots. Returns a log of what was removed, mirrors first.
pub fn delete_group(roots: &Roots, g: &Group, lock_dir: &Path, hslock: &Path) -> Result<Vec<String>, String> {
    let src = PathBuf::from(&g.key);
    if !roots.contains(&src) {
        return Err(format!("source outside session roots, not deleting: {}", src.display()));
    }
    for (_, m) in &g.mirrors {
        if !roots.contains(Path::new(m)) {
            return Err(format!("mirror outside session roots, not deleting: {m}"));
        }
    }
    let conv = conversation_id(&g.key);
    let outcome = with_claim(hslock, Some(lock_dir), &conv, "tui", std::process::id(), &g.key, || {
        let mut log = Vec::new();
        for (target, m) in &g.mirrors {
            if Path::new(m).exists() {
                fs::remove_file(m).map_err(|e| e.to_string())?;
            }
            log.push(format!("mirror {target}: {m}"));
        }
        if src.exists() {
            fs::remove_file(&src).map_err(|e| e.to_string())?;
        }
        log.push(format!("source {}", g.key));
        Ok::<Vec<String>, String>(log)
    });
    match outcome {
        ClaimOutcome::Skipped(h) => Err(format!("conversation is in use by {h}; try again when its turn ends")),
        ClaimOutcome::Ran(r) => r,
    }
}

/// Rename a source by rewriting only its title record. Refused outside the roots.
pub fn rename_group(roots: &Roots, g: &Group, title: &str, lock_dir: &Path, hslock: &Path) -> Result<String, String> {
    let new = validate_title(title)?;
    let path = PathBuf::from(&g.key);
    if !roots.contains(&path) {
        return Err(format!("source outside session roots, not renaming: {}", path.display()));
    }
    let conv = conversation_id(&g.key);
    let outcome = with_claim(hslock, Some(lock_dir), &conv, "tui", std::process::id(), &g.key, || {
        let text = fs::read_to_string(&path).map_err(|e| e.to_string())?;
        let rewritten = rewrite_title(&text, &g.fmt, &new);
        let tmp = path.with_extension("rename.tmp");
        fs::write(&tmp, rewritten).map_err(|e| e.to_string())?;
        fs::rename(&tmp, &path).map_err(|e| e.to_string())
    });
    match outcome {
        ClaimOutcome::Skipped(h) => Err(format!("conversation is in use by {h}")),
        ClaimOutcome::Ran(r) => r.map(|_| new),
    }
}

/// Replace the title in the native record for `fmt`. Other records are untouched.
fn rewrite_title(text: &str, fmt: &str, title: &str) -> String {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    if fmt == "omp" {
        // The header carries the title; a `title` record is the slot omp reads first.
        for l in lines.iter_mut() {
            if l.starts_with("{\"type\":\"session\"") {
                if let Ok(mut v) = serde_json::from_str::<serde_json::Value>(l) {
                    v["title"] = serde_json::Value::String(title.into());
                    *l = v.to_string();
                }
            }
        }
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

fn conversation_id(key: &str) -> String {
    let name = file_name(key);
    let stem = name.strip_suffix(".jsonl").unwrap_or(&name);
    stem.rsplit('_').next().unwrap_or(stem).to_string()
}
