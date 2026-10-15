//! Claude Code session JSONL -> omp/Pi journal.
//!
//! Input is a Claude Code transcript (`~/.claude/projects/<cwd>/<uuid>.jsonl`).
//! Output is an omp journal that every other converter in this crate accepts.
//!
//! Mapping:
//!   uuid / parentUuid   -> id / parentId (the chain the reader walks)
//!   sessionId, cwd      -> session header
//!   user / assistant    -> message records (role from `message.role`)
//!   text blocks         -> text content blocks
//!   tool_use / tool_result, thinking, attachment, file-history-snapshot,
//!   mode, permission-mode, system, last-prompt -> not converted; counted
//!   isMeta user records and `<tag>` wrapper text -> not conversation turns
//!
//! Claude signs its reasoning blocks and cannot accept ours back, so thinking
//! is never carried into a Claude mirror; here it is dropped on the way in.

use serde_json::{json, Map, Value};

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub messages: usize,
    pub text_blocks: usize,
    pub skipped_tool_blocks: usize,
    pub skipped_thinking_blocks: usize,
    pub skipped_records: usize,
}

/// Convert the whole transcript text. Returns the omp journal lines, header
/// first. Lines that are not JSON are an error, not a silent skip.
pub fn to_journal_lines(raw: &str) -> Result<(Vec<String>, Stats), String> {
    let mut stats = Stats::default();
    let mut session_id = String::new();
    let mut cwd = String::new();
    let mut first_ts = String::new();
    let mut out: Vec<Map<String, Value>> = Vec::new();
    // uuid -> its parent uuid, for EVERY record (kept or skipped). A skipped
    // record (tool-only turn, attachment) is still a link in the chain, so a
    // kept message's parent is found by walking up through skipped ones.
    let mut parent_of: std::collections::HashMap<String, Option<String>> = Default::default();
    let mut kept_parent: Vec<(String, Option<String>)> = Vec::new();

    for (n, line) in raw.lines().enumerate() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        let rec: Value = serde_json::from_str(t).map_err(|e| format!("line {}: {e}", n + 1))?;
        let Value::Object(r) = rec else {
            return Err(format!("line {}: record is not an object", n + 1));
        };
        if session_id.is_empty() {
            if let Some(s) = r.get("sessionId").and_then(Value::as_str) {
                session_id = s.to_string();
            }
        }
        if cwd.is_empty() {
            if let Some(c) = r.get("cwd").and_then(Value::as_str) {
                cwd = c.to_string();
            }
        }
        let uuid_here = r.get("uuid").and_then(Value::as_str).unwrap_or("").to_string();
        if !uuid_here.is_empty() {
            let parent_here = r.get("parentUuid").and_then(Value::as_str).map(str::to_string);
            parent_of.insert(uuid_here, parent_here);
        }
        let kind = r.get("type").and_then(Value::as_str).unwrap_or("");
        if kind != "user" && kind != "assistant" {
            stats.skipped_records += 1;
            continue;
        }
        if r.get("isMeta").and_then(Value::as_bool).unwrap_or(false) {
            stats.skipped_records += 1;
            continue;
        }
        let Some(msg) = r.get("message").and_then(Value::as_object) else {
            stats.skipped_records += 1;
            continue;
        };
        let role = msg.get("role").and_then(Value::as_str).unwrap_or(kind);
        let mut content: Vec<Value> = Vec::new();
        match msg.get("content") {
            Some(Value::String(s)) => {
                if is_real_text(s) {
                    content.push(json!({"type": "text", "text": s}));
                    stats.text_blocks += 1;
                }
            }
            Some(Value::Array(blocks)) => {
                for b in blocks {
                    match b.get("type").and_then(Value::as_str) {
                        Some("text") => {
                            let s = b.get("text").and_then(Value::as_str).unwrap_or("");
                            if is_real_text(s) {
                                content.push(json!({"type": "text", "text": s}));
                                stats.text_blocks += 1;
                            }
                        }
                        Some("thinking") | Some("redacted_thinking") => stats.skipped_thinking_blocks += 1,
                        Some(_) => stats.skipped_tool_blocks += 1,
                        None => {}
                    }
                }
            }
            _ => {}
        }
        if content.is_empty() {
            stats.skipped_records += 1;
            continue;
        }
        let id = r.get("uuid").and_then(Value::as_str).unwrap_or("").to_string();
        let parent = r.get("parentUuid").and_then(Value::as_str).map(str::to_string);
        let ts = r.get("timestamp").and_then(Value::as_str).unwrap_or("").to_string();
        // Resolved after the loop, once every record is known.
        kept_parent.push((id.clone(), parent.clone()));
        if first_ts.is_empty() && !ts.is_empty() {
            first_ts = ts.clone();
        }
        let mut m = Map::new();
        m.insert("type".into(), "message".into());
        m.insert("id".into(), id.into());
        m.insert("parentId".into(), parent.map(Value::from).unwrap_or(Value::Null));
        m.insert("timestamp".into(), ts.into());
        m.insert("message".into(), json!({"role": role, "content": content}));
        out.push(m);
        stats.messages += 1;
    }

    if stats.messages == 0 {
        return Err("transcript has no user or assistant text to convert".into());
    }
    // Map each kept message's parent to its nearest KEPT ancestor, walking up
    // through skipped records. This keeps the chain unbroken.
    let kept: std::collections::HashSet<String> = out.iter()
        .filter_map(|m| m.get("id").and_then(Value::as_str).map(str::to_string)).collect();
    let resolve = |start: Option<String>| -> Option<String> {
        let mut cur = start;
        let mut seen = std::collections::HashSet::new();
        while let Some(id) = cur.take() {
            if kept.contains(&id) {
                return Some(id);
            }
            if !seen.insert(id.clone()) {
                return None; // cycle in the source; stop rather than loop
            }
            cur = parent_of.get(&id).cloned().flatten();
        }
        None
    };
    let resolved: std::collections::HashMap<String, Option<String>> = kept_parent.into_iter()
        .map(|(id, parent)| (id, resolve(parent)))
        .collect();
    for m in out.iter_mut() {
        let id = m.get("id").and_then(Value::as_str).unwrap_or("").to_string();
        let p = resolved.get(&id).cloned().flatten();
        m.insert("parentId".into(), p.map(Value::from).unwrap_or(Value::Null));
    }
    if session_id.is_empty() {
        return Err("transcript has no sessionId".into());
    }

    let mut header = Map::new();
    header.insert("type".into(), "session".into());
    header.insert("version".into(), 3.into());
    header.insert("id".into(), session_id.into());
    header.insert("timestamp".into(), if first_ts.is_empty() { "1970-01-01T00:00:00.000Z".into() } else { first_ts.into() });
    header.insert("cwd".into(), cwd.into());
    let mut lines = vec![serde_json::to_string(&header).map_err(|e| e.to_string())?];
    for m in &out {
        lines.push(serde_json::to_string(m).map_err(|e| e.to_string())?);
    }
    Ok((lines, stats))
}

/// Claude wraps local commands and reminders in <tags>; those are not turns.
fn is_real_text(s: &str) -> bool {
    let t = s.trim();
    !t.is_empty() && !t.starts_with('<')
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: &str = concat!(
        r#"{"type":"user","uuid":"u1","parentUuid":null,"sessionId":"sess-1","cwd":"/tmp/c","timestamp":"2026-10-01T10:00:00.000Z","message":{"role":"user","content":"hello claude"}}"#, "\n",
        r#"{"type":"assistant","uuid":"a1","parentUuid":"u1","sessionId":"sess-1","cwd":"/tmp/c","timestamp":"2026-10-01T10:00:01.000Z","message":{"role":"assistant","content":[{"type":"thinking","thinking":"hm"},{"type":"text","text":"hi there"}]}}"#, "\n",
        r#"{"type":"attachment","uuid":"x1","sessionId":"sess-1"}"#, "\n",
        r#"{"type":"user","uuid":"u2","parentUuid":"a1","sessionId":"sess-1","timestamp":"2026-10-01T10:00:02.000Z","message":{"role":"user","content":[{"type":"text","text":"second"}]}}"#, "\n",
    );

    #[test]
    fn keeps_turns_in_order_and_links_them() {
        let (lines, stats) = to_journal_lines(T).unwrap();
        assert_eq!(lines.len(), 4, "header + 3 messages");
        let a: Value = serde_json::from_str(&lines[2]).unwrap();
        assert_eq!(a["parentId"], "u1");
        assert_eq!(a["message"]["content"][0]["text"], "hi there");
        assert_eq!(stats.messages, 3);
    }

    #[test]
    fn thinking_is_dropped_never_carried() {
        let (lines, stats) = to_journal_lines(T).unwrap();
        assert!(!lines.join("\n").contains("hm"), "thinking leaked into the journal");
        assert_eq!(stats.skipped_thinking_blocks, 1);
    }

    #[test]
    fn header_carries_session_and_cwd() {
        let (lines, _) = to_journal_lines(T).unwrap();
        let h: Value = serde_json::from_str(&lines[0]).unwrap();
        assert_eq!(h["id"], "sess-1");
        assert_eq!(h["cwd"], "/tmp/c");
    }

    #[test]
    fn meta_and_tag_wrappers_are_not_turns() {
        let raw = concat!(
            r#"{"type":"user","uuid":"m","sessionId":"s","isMeta":true,"message":{"role":"user","content":"meta"}}"#, "\n",
            r#"{"type":"user","uuid":"w","sessionId":"s","message":{"role":"user","content":"<command-name>/x</command-name>"}}"#, "\n",
            r#"{"type":"user","uuid":"r","sessionId":"s","message":{"role":"user","content":"real"}}"#, "\n",
        );
        let (lines, stats) = to_journal_lines(raw).unwrap();
        assert_eq!(lines.len(), 2);
        assert_eq!(stats.messages, 1);
    }

    #[test]
    fn skipped_tool_turn_does_not_break_the_chain() {
        // u1 -> a_tool (tool_use only, skipped) -> u2 must still link u2 to u1.
        let raw = concat!(
            r#"{"type":"user","uuid":"u1","parentUuid":null,"sessionId":"s","message":{"role":"user","content":"first"}}"#, "\n",
            r#"{"type":"assistant","uuid":"a_tool","parentUuid":"u1","sessionId":"s","message":{"role":"assistant","content":[{"type":"tool_use","id":"t","name":"Bash"}]}}"#, "\n",
            r#"{"type":"user","uuid":"u2","parentUuid":"a_tool","sessionId":"s","message":{"role":"user","content":"second"}}"#, "\n",
        );
        let (lines, _) = to_journal_lines(raw).unwrap();
        let u2: Value = serde_json::from_str(&lines[2]).unwrap();
        assert_eq!(u2["parentId"], "u1", "chain must skip the tool-only turn and link to u1");
    }

    #[test]
    fn active_path_is_reachable_through_the_reader() {
        // The converted journal must be walkable by the crate's own reader.
        let raw = concat!(
            r#"{"type":"user","uuid":"u1","parentUuid":null,"sessionId":"s","message":{"role":"user","content":"one"}}"#, "\n",
            r#"{"type":"assistant","uuid":"a1","parentUuid":"u1","sessionId":"s","message":{"role":"assistant","content":[{"type":"text","text":"two"}]}}"#, "\n",
            r#"{"type":"assistant","uuid":"a_tool","parentUuid":"a1","sessionId":"s","message":{"role":"assistant","content":[{"type":"tool_use","id":"t"}]}}"#, "\n",
            r#"{"type":"user","uuid":"u2","parentUuid":"a_tool","sessionId":"s","message":{"role":"user","content":"three"}}"#, "\n",
        );
        let (lines, _) = to_journal_lines(raw).unwrap();
        let path = std::env::temp_dir().join(format!("hs-fromclaude-{}.jsonl", std::process::id()));
        std::fs::write(&path, lines.join("\n") + "\n").unwrap();
        let j = crate::journal::Journal::read(&path).unwrap();
        let active = j.messages();
        let _ = std::fs::remove_file(&path);
        assert_eq!(active.len(), 3, "all three kept turns must be on the active path");
    }

    #[test]
    fn garbage_line_is_an_error() {
        assert!(to_journal_lines("not json\n").is_err());
    }

    #[test]
    fn no_turns_is_an_error() {
        let raw = r#"{"type":"attachment","uuid":"x","sessionId":"s"}"#;
        assert!(to_journal_lines(raw).is_err());
    }
}
