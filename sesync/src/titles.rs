//! Title rules. Ported from daemon/sesync.py.

use serde_json::Value;

/// True for O-penCode's untouched default name, "New session - <ISO timestamp>".
/// Only that exact shape is stripped, so a user title that mentions a date survives.
fn is_opencode_default(t: &str) -> bool {
    let Some(rest) = t.strip_prefix("New session - ") else { return false };
    // YYYY-MM-DDTHH:MM:SS(.fff)Z
    let b = rest.as_bytes();
    if b.len() < 20 || !rest.ends_with('Z') {
        return false;
    }
    let digit = |i: usize| b.get(i).map_or(false, |c| c.is_ascii_digit());
    let shape = digit(0) && digit(1) && digit(2) && digit(3) && b[4] == b'-' && digit(5) && digit(6)
        && b[7] == b'-' && digit(8) && digit(9) && b[10] == b'T' && digit(11) && digit(12)
        && b[13] == b':' && digit(14) && digit(15) && b[16] == b':' && digit(17) && digit(18);
    if !shape {
        return false;
    }
    // Remaining characters between seconds and the Z: nothing, or .digits
    let mid = &rest[19..rest.len() - 1];
    mid.is_empty() || (mid.starts_with('.') && mid[1..].bytes().all(|c| c.is_ascii_digit()) && mid.len() > 1)
}

/// The name for an O-penCode session, or "" when it is untitled.
pub fn clean_opencode_title(title: Option<&str>) -> &str {
    let t = title.unwrap_or("").trim();
    if is_opencode_default(t) { "" } else { t }
}

/// Text of a user message's content: a string, or the first text block of a list.
fn user_text(msg: &Value) -> Option<String> {
    let content = msg.get("content")?;
    if let Some(s) = content.as_str() {
        return Some(s.to_string());
    }
    content.as_array()?.iter().find_map(|b| {
        (b.get("type")?.as_str()? == "text").then(|| b.get("text")?.as_str().map(str::to_string)).flatten()
    })
}

/// Claude Code's own continuation banners and wrapper text are never titles.
const CLAUDE_BANNER: [&str; 2] = ["This session is being continued", "Caveat:"];

/// (title, has_user_turn) for a Claude Code transcript, from its lines.
///
/// Precedence, as Claude Code's own reader applies it: custom-title (/rename or -n,
/// latest wins), then ai-title, then agent-name, then the first real user prompt.
pub fn claude_title_from_lines(lines: &[&str]) -> (String, bool) {
    let (mut custom, mut ai, mut agent, mut first, mut has_user) =
        (String::new(), String::new(), String::new(), String::new(), false);
    for line in lines {
        let Ok(rec) = serde_json::from_str::<Value>(line) else { continue };
        let kind = rec.get("type").and_then(Value::as_str).unwrap_or("");
        let field = |k: &str| rec.get(k).and_then(Value::as_str).map(|s| s.trim().to_string());
        match kind {
            "custom-title" => { if let Some(v) = field("customTitle").filter(|v| !v.is_empty()) { custom = v; } }
            "ai-title" => { if let Some(v) = field("aiTitle").filter(|v| !v.is_empty()) { ai = v; } }
            "agent-name" => { if let Some(v) = field("agentName").filter(|v| !v.is_empty()) { agent = v; } }
            "user" if rec.get("isMeta").and_then(Value::as_bool) != Some(true) => {
                if let Some(text) = rec.get("message").and_then(user_text) {
                    let text = text.trim();
                    if !text.is_empty() && !text.starts_with('<') {
                        has_user = true;
                        if first.is_empty() && !CLAUDE_BANNER.iter().any(|b| text.starts_with(b)) {
                            first = text.split('\n').next().unwrap_or("").chars().take(80).collect();
                        }
                    }
                }
            }
            _ => {}
        }
    }
    let title = [custom, ai, agent, first].into_iter().find(|t| !t.is_empty()).unwrap_or_default();
    (title, has_user)
}

/// (title, _) for an omp/Pi journal. The title record (omp's slot, or a rename)
/// wins over the header, which can hold an older name. Falls back to the first
/// user message. Never the filename.
pub fn omp_title_from_lines(lines: &[&str]) -> (String, bool) {
    let (mut header, mut slot, mut fallback) = (String::new(), String::new(), String::new());
    for line in lines {
        let Ok(rec) = serde_json::from_str::<Value>(line) else { continue };
        match rec.get("type").and_then(Value::as_str).unwrap_or("") {
            "session" => {
                if let Some(t) = rec.get("title").and_then(Value::as_str).filter(|t| !t.is_empty()) {
                    header = t.to_string();
                }
            }
            "title" => {
                if let Some(t) = rec.get("title").and_then(Value::as_str).map(str::trim).filter(|t| !t.is_empty()) {
                    slot = t.to_string();
                }
            }
            "session_info" => {
                if let Some(t) = rec.get("name").and_then(Value::as_str).map(str::trim).filter(|t| !t.is_empty()) {
                    slot = t.to_string();
                }
            }
            "message" if fallback.is_empty() => {
                if let Some(m) = rec.get("message") {
                    if m.get("role").and_then(Value::as_str) == Some("user") {
                        if let Some(text) = user_text(m) {
                            fallback = text.trim().split('\n').next().unwrap_or("").chars().take(80).collect();
                        }
                    }
                }
            }
            _ => {}
        }
    }
    let title = [slot, header, fallback].into_iter().find(|t| !t.is_empty()).unwrap_or_default();
    (title, true)
}
