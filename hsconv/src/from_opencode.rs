//! OpenCode export -> omp/Pi journal.
//!
//! Input is the JSON printed by `opencode export <session-id>` (it may start
//! with a banner line; the JSON begins at the first `{`). Output is an omp
//! journal that `Journal::read` and every other converter in this crate
//! accept, so OpenCode becomes one more source for the same canonical form.
//!
//! Mapping:
//!   info.id            -> session header `id`
//!   info.directory     -> session header `cwd`
//!   info.title         -> session header `title` (O-penCode default
//!                         "New session - <ts>" titles are dropped, not kept)
//!   message.role       -> message.role (user | assistant)
//!   text parts         -> text content blocks
//!   tool parts         -> not converted yet (reported in `Stats`)
//!   reasoning parts    -> dropped (the README keeps reasoning for omp/Pi, but
//!                         an OpenCode reasoning block has no omp signature
//!                         and is not needed to resume the conversation)
//!   time.created (ms)  -> record `timestamp` (ISO); never the epoch

use serde_json::{json, Map, Value};

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub messages: usize,
    pub text_parts: usize,
    pub skipped_tool_parts: usize,
    pub skipped_reasoning_parts: usize,
}

/// Parse the export text (banner allowed) into a JSON value.
pub fn parse_export(raw: &str) -> Result<Value, String> {
    let start = raw.find('{').ok_or("export has no JSON object")?;
    serde_json::from_str(&raw[start..]).map_err(|e| format!("export is not valid JSON: {e}"))
}

/// Build the omp journal lines (header first, then one record per message).
pub fn to_journal_lines(export: &Value) -> Result<(Vec<String>, Stats), String> {
    let info = export.get("info").and_then(Value::as_object).ok_or("export has no `info`")?;
    let session_id = info.get("id").and_then(Value::as_str).ok_or("export info has no `id`")?;
    let cwd = info.get("directory").and_then(Value::as_str).unwrap_or("");
    let created_ms = info.get("time").and_then(|t| t.get("created")).and_then(Value::as_i64).unwrap_or(0);
    let title = clean_title(info.get("title").and_then(Value::as_str).unwrap_or(""));

    let messages = export.get("messages").and_then(Value::as_array).ok_or("export has no `messages`")?;
    let mut stats = Stats::default();

    let mut header = Map::new();
    header.insert("type".into(), "session".into());
    header.insert("version".into(), 3.into());
    header.insert("id".into(), session_id.into());
    header.insert("timestamp".into(), iso_ms(created_ms).into());
    header.insert("cwd".into(), cwd.into());
    if !title.is_empty() {
        header.insert("title".into(), title.clone().into());
    }
    let mut lines = vec![serde_json::to_string(&header).map_err(|e| e.to_string())?];

    let mut prev: Option<String> = None;
    for (i, m) in messages.iter().enumerate() {
        let minfo = m.get("info").and_then(Value::as_object).ok_or("message has no `info`")?;
        let role = minfo.get("role").and_then(Value::as_str).unwrap_or("");
        if role != "user" && role != "assistant" {
            continue;
        }
        let id = minfo.get("id").and_then(Value::as_str).map(str::to_string)
            .unwrap_or_else(|| format!("oc-{session_id}-{i}"));
        let created = minfo.get("time").and_then(|t| t.get("created")).and_then(Value::as_i64).unwrap_or(created_ms);

        let mut content: Vec<Value> = Vec::new();
        for p in m.get("parts").and_then(Value::as_array).into_iter().flatten() {
            match p.get("type").and_then(Value::as_str) {
                Some("text") => {
                    let text = p.get("text").and_then(Value::as_str).unwrap_or("");
                    if !text.trim().is_empty() {
                        content.push(json!({"type": "text", "text": text}));
                        stats.text_parts += 1;
                    }
                }
                Some("reasoning") => stats.skipped_reasoning_parts += 1,
                Some("step-start") | Some("step-finish") => {}
                Some(_) => stats.skipped_tool_parts += 1,
                None => {}
            }
        }
        if content.is_empty() {
            continue; // nothing to resume from an empty turn
        }

        let mut msg = Map::new();
        msg.insert("role".into(), role.into());
        msg.insert("content".into(), Value::Array(content));
        let mut rec = Map::new();
        rec.insert("type".into(), "message".into());
        rec.insert("id".into(), id.clone().into());
        rec.insert("parentId".into(), prev.clone().map(Value::from).unwrap_or(Value::Null));
        rec.insert("timestamp".into(), iso_ms(created).into());
        rec.insert("message".into(), Value::Object(msg));
        lines.push(serde_json::to_string(&rec).map_err(|e| e.to_string())?);
        prev = Some(id);
        stats.messages += 1;
    }

    if stats.messages == 0 {
        return Err("export has no user or assistant text to convert".into());
    }
    Ok((lines, stats))
}

/// O-penCode names an untitled session "New session - <timestamp>". That is
/// not a title, so it is dropped rather than carried into the mirror name.
fn clean_title(t: &str) -> String {
    let t = t.trim();
    if t.starts_with("New session - ") && t.ends_with('Z') {
        return String::new();
    }
    t.to_string()
}

/// ISO-8601 UTC with milliseconds, from epoch milliseconds.
fn iso_ms(ms: i64) -> String {
    let days = ms.div_euclid(86_400_000);
    let rem = ms.rem_euclid(86_400_000);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{:03}Z",
        rem / 3_600_000,
        rem / 60_000 % 60,
        rem / 1000 % 60,
        rem % 1000
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn export(text_user: &str, text_asst: &str) -> Value {
        json!({
            "info": {"id": "ses_test", "directory": "/tmp/x", "title": "New session - 2026-10-14T18:31:12.640Z",
                     "time": {"created": 1791000000000i64}},
            "messages": [
                {"info": {"id": "msg_u", "role": "user", "time": {"created": 1791000000000i64}},
                 "parts": [{"type": "text", "text": text_user}]},
                {"info": {"id": "msg_a", "role": "assistant", "time": {"created": 1791000001000i64}},
                 "parts": [{"type": "step-start"}, {"type": "reasoning", "text": "hmm"},
                           {"type": "text", "text": text_asst}, {"type": "step-finish"}]}
            ]
        })
    }

    #[test]
    fn keeps_prompt_and_answer_in_order() {
        let (lines, stats) = to_journal_lines(&export("ping", "pong")).unwrap();
        assert_eq!(lines.len(), 3, "header + 2 messages");
        let u: Value = serde_json::from_str(&lines[1]).unwrap();
        let a: Value = serde_json::from_str(&lines[2]).unwrap();
        assert_eq!(u["message"]["role"], "user");
        assert_eq!(u["message"]["content"][0]["text"], "ping");
        assert_eq!(a["message"]["role"], "assistant");
        assert_eq!(a["message"]["content"][0]["text"], "pong");
        assert_eq!(a["parentId"], u["id"], "answer must link to the prompt");
        assert_eq!(stats.messages, 2);
        assert_eq!(stats.skipped_reasoning_parts, 1);
    }

    #[test]
    fn default_title_is_dropped_not_copied() {
        let (lines, _) = to_journal_lines(&export("a", "b")).unwrap();
        let h: Value = serde_json::from_str(&lines[0]).unwrap();
        assert!(h.get("title").is_none(), "date-named default must not become a title");
    }

    #[test]
    fn timestamps_are_real_not_epoch() {
        let (lines, _) = to_journal_lines(&export("a", "b")).unwrap();
        let u: Value = serde_json::from_str(&lines[1]).unwrap();
        assert!(u["timestamp"].as_str().unwrap().starts_with("2026-"), "{}", u["timestamp"]);
    }

    #[test]
    fn banner_before_json_is_accepted() {
        let raw = format!("Exporting session: ses_test\n{}", export("a", "b"));
        assert!(parse_export(&raw).is_ok());
    }

    #[test]
    fn empty_session_is_an_error() {
        let v = json!({"info": {"id": "ses_e", "directory": "/", "time": {"created": 0}}, "messages": []});
        assert!(to_journal_lines(&v).is_err());
    }
}
