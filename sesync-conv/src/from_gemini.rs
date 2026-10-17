//! Gemini CLI session JSONL -> omp/Pi journal.
//!
//! Input is a Gemini CLI session (`~/.gemini/tmp/<slug>/chats/session-*.jsonl`), the
//! modern append-only event log. Output is an omp journal that every other converter
//! in this crate accepts.
//!
//! Reading (the same replay Gemini CLI does on resume):
//!   line with string sessionId + projectHash -> metadata, shallow-merged
//!   line with string `id`, no `$patch`        -> MessageRecord, upserted by id in place
//!   {"$set": {...}}                           -> metadata update; a `messages` array
//!                                                replaces the whole message list
//!   {"$patch": {...}}                         -> content replace for `id`, toolCalls
//!                                                results for `id`, removeIds, orderIds;
//!                                                other keys ignored and counted
//!   {"$rewindTo": "<id>"}                     -> that message and all after it removed
//!   non-JSON lines                            -> skipped and counted, as Gemini does
//!
//! Mapping:
//!   sessionId, startTime  -> session header id, timestamp (then lastUpdated; never the epoch)
//!   directories[0]        -> session header cwd (empty if absent)
//!   summary               -> session header title
//!   user / gemini         -> user / assistant message records, parentId = previous kept
//!   info, error, warning  -> UI only; not converted, counted
//!   text parts            -> text content blocks
//!   functionCall          -> toolCall block {id, name, arguments} in the assistant message
//!   functionResponse      -> one toolResult message per response {toolCallId, toolName,
//!                            content, isError = "error" in response}, chained in order
//!   inlineData            -> image block {data, mimeType}
//!   fileData              -> not converted; counted
//!   thought parts, thoughts[] -> dropped and counted; thoughtSignature is never copied
//!
//! A gemini message whose content has a functionCall or thought part is modern: its
//! content is the only truth. Otherwise it is legacy and is rebuilt from text plus one
//! functionCall per `toolCalls[]` entry; an entry's `result` that no later user message
//! answers before the next gemini message is emitted as a synthesized toolResult.
//!
//! Results pair with calls by id: a result whose call was never seen (orphan) or a second
//! result for the same call (duplicate) is dropped and counted, as in from_claude.rs.
//! Format research from google-gemini/gemini-cli source (Apache-2.0).

use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub messages: usize,
    pub text_parts: usize,
    pub images: usize,
    pub tool_calls: usize,
    pub tool_results: usize,
    pub synthesized_tool_results: usize,
    pub orphan_tool_results: usize,
    pub duplicate_tool_results: usize,
    pub skipped_thoughts: usize,
    pub thoughts_kept: usize,
    pub skipped_ui_messages: usize,
    pub skipped_file_parts: usize,
    pub skipped_parts: usize,
    pub skipped_records: usize,
    pub bad_lines: usize,
    pub ignored_patch_keys: usize,
}

/// Replay the event log into (metadata, messages in order).
fn replay(raw: &str, stats: &mut Stats) -> (Map<String, Value>, Vec<Map<String, Value>>) {
    let mut meta = Map::new();
    let mut msgs: Vec<Map<String, Value>> = Vec::new();
    let pos = |msgs: &Vec<Map<String, Value>>, id: &str| {
        msgs.iter().position(|m| m.get("id").and_then(Value::as_str) == Some(id))
    };
    for line in raw.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        let Ok(Value::Object(r)) = serde_json::from_str::<Value>(t) else {
            stats.bad_lines += 1;
            continue;
        };
        if let Some(Value::String(target)) = r.get("$rewindTo") {
            if let Some(i) = pos(&msgs, target) {
                msgs.truncate(i);
            }
            continue;
        }
        if let Some(set) = r.get("$set").and_then(Value::as_object) {
            for (k, v) in set {
                if k == "messages" {
                    if let Value::Array(list) = v {
                        msgs = list.iter().filter_map(|m| m.as_object().cloned()).collect();
                    }
                } else {
                    meta.insert(k.clone(), v.clone());
                }
            }
            continue;
        }
        if let Some(p) = r.get("$patch").and_then(Value::as_object) {
            let target = p.get("id").and_then(Value::as_str);
            for (k, v) in p {
                match k.as_str() {
                    "id" => {}
                    "content" => {
                        if let Some(i) = target.and_then(|id| pos(&msgs, id)) {
                            msgs[i].insert("content".into(), v.clone());
                        }
                    }
                    "toolCalls" => {
                        let Some(i) = target.and_then(|id| pos(&msgs, id)) else { continue };
                        let Some(patches) = v.as_array() else { continue };
                        let Some(Value::Array(calls)) = msgs[i].get_mut("toolCalls") else { continue };
                        for tp in patches {
                            let Some(cid) = tp.get("id").and_then(Value::as_str) else { continue };
                            if let Some(Value::Object(c)) = calls.iter_mut()
                                .find(|c| c.get("id").and_then(Value::as_str) == Some(cid)) {
                                if let Some(res) = tp.get("result") {
                                    c.insert("result".into(), res.clone());
                                }
                            }
                        }
                    }
                    "removeIds" => {
                        let ids: HashSet<&str> = v.as_array().map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
                        msgs.retain(|m| !ids.contains(m.get("id").and_then(Value::as_str).unwrap_or("")));
                    }
                    "orderIds" => {
                        let order: Vec<&str> = v.as_array().map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
                        let mut rest = std::mem::take(&mut msgs);
                        for id in order {
                            if let Some(i) = pos(&rest, id) {
                                msgs.push(rest.remove(i));
                            }
                        }
                        msgs.append(&mut rest);
                    }
                    _ => stats.ignored_patch_keys += 1,
                }
            }
            continue;
        }
        if matches!(r.get("sessionId"), Some(Value::String(_))) && matches!(r.get("projectHash"), Some(Value::String(_))) {
            for (k, v) in r {
                meta.insert(k, v);
            }
            continue;
        }
        if let Some(id) = r.get("id").and_then(Value::as_str).map(str::to_string) {
            match pos(&msgs, &id) {
                Some(i) => msgs[i] = r,
                None => msgs.push(r),
            }
            continue;
        }
        stats.skipped_records += 1;
    }
    (meta, msgs)
}

/// A PartListUnion as a list of parts. A bare string is one text part.
fn parts_of(v: Option<&Value>) -> Vec<Value> {
    let one = |p: &Value| match p {
        Value::String(s) => json!({"text": s}),
        other => other.clone(),
    };
    match v {
        Some(Value::Array(a)) => a.iter().map(one).collect(),
        Some(Value::Null) | None => Vec::new(),
        Some(p) => vec![one(p)],
    }
}

fn as_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// The text of a functionResponse's `response`: output, or error. Returns (text, isError).
fn response_text(resp: Option<&Value>) -> (String, bool) {
    match resp {
        Some(Value::Object(o)) if o.contains_key("error") => (as_text(&o["error"]), true),
        Some(Value::Object(o)) if o.contains_key("output") => (as_text(&o["output"]), false),
        Some(v) => (as_text(v), false),
        None => (String::new(), false),
    }
}

/// The text of a legacy toolCalls[].result (a PartListUnion). Returns (text, isError).
fn legacy_result_text(res: &Value) -> (String, bool) {
    let parts = parts_of(Some(res));
    if let Some(fr) = parts.iter().find_map(|p| p.get("functionResponse")) {
        return response_text(fr.get("response"));
    }
    let texts: Vec<&str> = parts.iter().filter_map(|p| p.get("text").and_then(Value::as_str)).collect();
    if !texts.is_empty() {
        return (texts.join("\n"), false);
    }
    (as_text(res), false)
}

fn tool_result(id: &str, name: &str, text: String, is_error: bool) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("role".into(), "toolResult".into());
    m.insert("toolCallId".into(), id.into());
    m.insert("toolName".into(), name.into());
    m.insert("content".into(), json!([{"type": "text", "text": text}]));
    m.insert("isError".into(), is_error.into());
    m
}

struct Out {
    records: Vec<Map<String, Value>>,
    last: Option<String>,
}

impl Out {
    fn push(&mut self, id: String, ts: &str, message: Value, stats: &mut Stats) {
        let mut m = Map::new();
        m.insert("type".into(), "message".into());
        m.insert("id".into(), id.clone().into());
        m.insert("parentId".into(), self.last.clone().map(Value::from).unwrap_or(Value::Null));
        m.insert("timestamp".into(), ts.into());
        m.insert("message".into(), message);
        self.records.push(m);
        self.last = Some(id);
        stats.messages += 1;
    }
}

/// Legacy results not yet answered by a functionResponse: (call id, name, result, source msg id).
type Pending = Vec<(String, String, Value, String)>;

fn flush(pending: &mut Pending, answered: &mut HashSet<String>, out: &mut Out, ts: &str, stats: &mut Stats) {
    for (n, (cid, name, res, src)) in pending.drain(..).enumerate() {
        if !answered.insert(cid.clone()) {
            continue;
        }
        let (text, is_error) = legacy_result_text(&res);
        out.push(format!("{src}-s{n}"), ts, Value::Object(tool_result(&cid, &name, text, is_error)), stats);
        stats.tool_results += 1;
        stats.synthesized_tool_results += 1;
    }
}

/// Convert the whole session text. Returns the omp journal lines, header first.
/// Non-JSON lines are skipped and counted, as Gemini CLI does on resume.
pub fn to_journal_lines(raw: &str) -> Result<(Vec<String>, Stats), String> {
    let mut stats = Stats::default();
    let (meta, msgs) = replay(raw, &mut stats);
    let session_id = meta.get("sessionId").and_then(Value::as_str).unwrap_or("").to_string();
    if session_id.is_empty() {
        return Err("session has no sessionId".into());
    }
    let meta_ts = ["startTime", "lastUpdated"].iter()
        .find_map(|k| meta.get(*k).and_then(Value::as_str).filter(|s| !s.is_empty()))
        .unwrap_or("")
        .to_string();

    let mut out = Out { records: Vec::new(), last: None };
    let mut calls: HashMap<String, String> = HashMap::new();
    let mut answered: HashSet<String> = HashSet::new();
    let mut pending: Pending = Vec::new();
    let mut last_ts = meta_ts.clone();

    for m in &msgs {
        let id = m.get("id").and_then(Value::as_str).unwrap_or("").to_string();
        let kind = m.get("type").and_then(Value::as_str).unwrap_or("");
        let ts = m.get("timestamp").and_then(Value::as_str).filter(|s| !s.is_empty())
            .map(str::to_string).unwrap_or_else(|| meta_ts.clone());
        match kind {
            "user" | "gemini" => {}
            "info" | "error" | "warning" => {
                stats.skipped_ui_messages += 1;
                continue;
            }
            _ => {
                stats.skipped_records += 1;
                continue;
            }
        }
        let parts = parts_of(m.get("content"));
        let mut content: Vec<Value> = Vec::new();
        let model = m.get("model").and_then(Value::as_str).unwrap_or("gemini").to_string();

        if kind == "user" {
            let responses: Vec<&Value> = parts.iter().filter_map(|p| p.get("functionResponse")).collect();
            let response_ids: HashSet<&str> = responses.iter().filter_map(|r| r.get("id").and_then(Value::as_str)).collect();
            pending.retain(|(cid, ..)| !response_ids.contains(cid.as_str()));
            if responses.is_empty() {
                flush(&mut pending, &mut answered, &mut out, &last_ts, &mut stats);
            }
            let mut results: Vec<Map<String, Value>> = Vec::new();
            for p in &parts {
                if let Some(fr) = p.get("functionResponse") {
                    let cid = fr.get("id").and_then(Value::as_str).unwrap_or("").to_string();
                    let Some(name) = calls.get(&cid).cloned() else {
                        stats.orphan_tool_results += 1;
                        continue;
                    };
                    if !answered.insert(cid.clone()) {
                        stats.duplicate_tool_results += 1;
                        continue;
                    }
                    let name = fr.get("name").and_then(Value::as_str).map(str::to_string).unwrap_or(name);
                    let (text, is_error) = response_text(fr.get("response"));
                    results.push(tool_result(&cid, &name, text, is_error));
                    stats.tool_results += 1;
                } else {
                    common_part(p, "unknown", &mut content, &mut stats);
                }
            }
            if !content.is_empty() {
                out.push(id.clone(), &ts, json!({"role": "user", "content": content}), &mut stats);
            }
            for (n, r) in results.into_iter().enumerate() {
                out.push(format!("{id}-r{n}"), &ts, Value::Object(r), &mut stats);
            }
        } else {
            flush(&mut pending, &mut answered, &mut out, &last_ts, &mut stats);
            let modern = parts.iter().any(|p| p.get("functionCall").is_some() || p.get("thought").and_then(Value::as_bool) == Some(true));
            if !modern {
                stats.skipped_thoughts += m.get("thoughts").and_then(Value::as_array).map_or(0, Vec::len);
            }
            for p in &parts {
                if let Some(fc) = p.get("functionCall") {
                    push_call(fc, &mut content, &mut calls, &mut stats);
                } else {
                    common_part(p, &model, &mut content, &mut stats);
                }
            }
            let mut legacy_pending: Pending = Vec::new();
            if !modern {
                for tc in m.get("toolCalls").and_then(Value::as_array).into_iter().flatten() {
                    if push_call(tc, &mut content, &mut calls, &mut stats) {
                        if let Some(res) = tc.get("result").filter(|r| !r.is_null()) {
                            let cid = tc["id"].as_str().unwrap_or("").to_string();
                            let name = calls.get(&cid).cloned().unwrap_or_default();
                            legacy_pending.push((cid, name, res.clone(), id.clone()));
                        }
                    }
                }
            }
            if !content.is_empty() {
                out.push(id.clone(), &ts, json!({"role": "assistant", "content": content}), &mut stats);
                pending = legacy_pending;
            }
        }
        last_ts = ts;
    }
    flush(&mut pending, &mut answered, &mut out, &last_ts, &mut stats);

    if stats.messages == 0 {
        return Err("session has no user or gemini content to convert".into());
    }
    let header_ts = if meta_ts.is_empty() {
        out.records.iter().find_map(|r| r.get("timestamp").and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string))
            .ok_or("session has no startTime, lastUpdated or message timestamp")?
    } else {
        meta_ts
    };
    let cwd = meta.get("directories").and_then(Value::as_array)
        .and_then(|d| d.first()).and_then(Value::as_str).unwrap_or("").to_string();

    let mut header = Map::new();
    header.insert("type".into(), "session".into());
    header.insert("version".into(), 3.into());
    header.insert("id".into(), session_id.into());
    header.insert("timestamp".into(), header_ts.into());
    header.insert("cwd".into(), cwd.into());
    if let Some(t) = meta.get("summary").and_then(Value::as_str).filter(|s| !s.trim().is_empty()) {
        header.insert("title".into(), t.into());
    }
    let mut lines = vec![serde_json::to_string(&header).map_err(|e| e.to_string())?];
    for m in &out.records {
        lines.push(serde_json::to_string(m).map_err(|e| e.to_string())?);
    }
    Ok((lines, stats))
}

/// A functionCall (or legacy toolCalls entry) as a toolCall block. False if it has no id.
fn push_call(fc: &Value, content: &mut Vec<Value>, calls: &mut HashMap<String, String>, stats: &mut Stats) -> bool {
    let id = fc.get("id").and_then(Value::as_str).unwrap_or("").to_string();
    if id.is_empty() {
        stats.skipped_parts += 1;
        return false;
    }
    let name = fc.get("name").and_then(Value::as_str).unwrap_or("unknown_tool").to_string();
    let mut args = fc.get("args").cloned().unwrap_or_else(|| json!({}));
    if !args.is_object() {
        args = json!({"input": args});
    }
    calls.insert(id.clone(), name.clone());
    content.push(json!({"type": "toolCall", "id": id, "name": name, "arguments": args}));
    stats.tool_calls += 1;
    true
}

/// Parts shared by both roles: text, thought, inlineData, fileData. A thought part keeps
/// its words as a `<prior_reasoning>` marker; its signature is never copied.
fn common_part(p: &Value, model: &str, content: &mut Vec<Value>, stats: &mut Stats) {
    if p.get("thought").and_then(Value::as_bool) == Some(true) {
        let t = p.get("text").and_then(Value::as_str).unwrap_or("");
        match crate::prior_reasoning::wrap(t, "gemini", model) {
            Some(w) => {
                content.push(json!({"type": "text", "text": w}));
                stats.thoughts_kept += 1;
            }
            None => stats.skipped_thoughts += 1,
        }
    } else if let Some(s) = p.get("text").and_then(Value::as_str) {
        if !s.trim().is_empty() {
            content.push(json!({"type": "text", "text": s}));
            stats.text_parts += 1;
        }
    } else if let Some(d) = p.get("inlineData") {
        let data = d.get("data").and_then(Value::as_str).unwrap_or("");
        let mime = d.get("mimeType").and_then(Value::as_str).unwrap_or("");
        if data.is_empty() {
            stats.skipped_parts += 1;
        } else {
            content.push(json!({"type": "image", "data": data, "mimeType": mime}));
            stats.images += 1;
        }
    } else if p.get("fileData").is_some() {
        stats.skipped_file_parts += 1;
    } else {
        stats.skipped_parts += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HDR: &str = r#"{"sessionId":"gs-1","projectHash":"abc","startTime":"2026-10-01T10:00:00.000Z","lastUpdated":"2026-10-01T10:05:00.000Z","directories":["/tmp/g"],"kind":"main"}"#;

    fn session(lines: &[&str]) -> String {
        let mut s = String::from(HDR);
        s.push('\n');
        for l in lines {
            s.push_str(l);
            s.push('\n');
        }
        s
    }

    fn recs(lines: &[String]) -> Vec<Value> {
        lines.iter().map(|l| serde_json::from_str(l).unwrap()).collect()
    }

    const U1: &str = r#"{"id":"u1","timestamp":"2026-10-01T10:00:01.000Z","type":"user","content":[{"text":"list files"}]}"#;
    const G1: &str = r#"{"id":"g1","timestamp":"2026-10-01T10:00:02.000Z","type":"gemini","content":[{"text":"Listing."},{"functionCall":{"id":"ls__1","name":"list_directory","args":{"path":"."}}}]}"#;
    const R1: &str = r#"{"id":"r1","timestamp":"2026-10-01T10:00:03.000Z","type":"user","content":[{"functionResponse":{"id":"ls__1","name":"list_directory","response":{"output":"a.txt"}}}]}"#;
    const G2: &str = r#"{"id":"g2","timestamp":"2026-10-01T10:00:04.000Z","type":"gemini","content":[{"text":"Done."}]}"#;

    #[test]
    fn text_turns_in_order_with_parent_chain_and_header() {
        let raw = session(&[
            U1,
            r#"{"id":"g0","timestamp":"2026-10-01T10:00:02.000Z","type":"gemini","content":"hi there"}"#,
            r#"{"$set":{"lastUpdated":"2026-10-01T10:06:00.000Z","summary":"Listing files"}}"#,
            r#"{"id":"u2","timestamp":"2026-10-01T10:00:03.000Z","type":"user","content":"second"}"#,
        ]);
        let (lines, stats) = to_journal_lines(&raw).unwrap();
        let r = recs(&lines);
        assert_eq!(r[0]["type"], "session");
        assert_eq!(r[0]["version"], 3);
        assert_eq!(r[0]["id"], "gs-1");
        assert_eq!(r[0]["timestamp"], "2026-10-01T10:00:00.000Z");
        assert_eq!(r[0]["cwd"], "/tmp/g");
        assert_eq!(r[0]["title"], "Listing files");
        assert_eq!(stats.messages, 3);
        let ids: Vec<&str> = r[1..].iter().map(|m| m["id"].as_str().unwrap()).collect();
        assert_eq!(ids, ["u1", "g0", "u2"]);
        assert!(r[1]["parentId"].is_null());
        assert_eq!(r[2]["parentId"], "u1");
        assert_eq!(r[3]["parentId"], "g0");
        assert_eq!(r[1]["message"]["role"], "user");
        assert_eq!(r[1]["message"]["content"][0], json!({"type": "text", "text": "list files"}));
        assert_eq!(r[2]["message"]["role"], "assistant");
        assert_eq!(r[2]["message"]["content"][0]["text"], "hi there");
        assert_eq!(r[3]["message"]["content"][0]["text"], "second");
    }

    #[test]
    fn function_call_becomes_tool_call_block() {
        let (lines, stats) = to_journal_lines(&session(&[U1, G1])).unwrap();
        let r = recs(&lines);
        let c = &r[2]["message"]["content"];
        assert_eq!(c[0]["text"], "Listing.");
        assert_eq!(c[1], json!({"type": "toolCall", "id": "ls__1", "name": "list_directory", "arguments": {"path": "."}}));
        assert_eq!(stats.tool_calls, 1);
    }

    #[test]
    fn function_responses_become_chained_tool_results() {
        let g = r#"{"id":"g1","timestamp":"2026-10-01T10:00:02.000Z","type":"gemini","content":[{"functionCall":{"id":"a","name":"read_file","args":{}}},{"functionCall":{"id":"b","name":"shell","args":{}}}]}"#;
        let resp = r#"{"id":"r1","timestamp":"2026-10-01T10:00:03.000Z","type":"user","content":[{"functionResponse":{"id":"a","name":"read_file","response":{"output":"file body"}}},{"functionResponse":{"id":"b","name":"shell","response":{"error":"exit 1"}}}]}"#;
        let (lines, stats) = to_journal_lines(&session(&[U1, g, resp, G2])).unwrap();
        let r = recs(&lines);
        assert_eq!(stats.tool_results, 2);
        let ok = &r[3];
        assert_eq!(ok["parentId"], "g1");
        assert_eq!(ok["message"]["role"], "toolResult");
        assert_eq!(ok["message"]["toolCallId"], "a");
        assert_eq!(ok["message"]["toolName"], "read_file");
        assert_eq!(ok["message"]["isError"], false);
        assert_eq!(ok["message"]["content"][0], json!({"type": "text", "text": "file body"}));
        let err = &r[4];
        assert_eq!(err["parentId"], ok["id"]);
        assert_eq!(err["message"]["toolCallId"], "b");
        assert_eq!(err["message"]["isError"], true);
        assert_eq!(err["message"]["content"][0]["text"], "exit 1");
        assert_eq!(r[5]["id"], "g2");
        assert_eq!(r[5]["parentId"], err["id"], "next turn links to the last result");
    }

    #[test]
    fn orphan_and_duplicate_responses_are_dropped_and_counted() {
        let orphan = r#"{"id":"r0","timestamp":"2026-10-01T10:00:01.500Z","type":"user","content":[{"functionResponse":{"id":"nope","name":"x","response":{"output":"?"}}}]}"#;
        let dup = r#"{"id":"r2","timestamp":"2026-10-01T10:00:03.500Z","type":"user","content":[{"functionResponse":{"id":"ls__1","name":"list_directory","response":{"output":"again"}}}]}"#;
        let (lines, stats) = to_journal_lines(&session(&[U1, orphan, G1, R1, dup, G2])).unwrap();
        assert_eq!(stats.orphan_tool_results, 1);
        assert_eq!(stats.duplicate_tool_results, 1);
        assert_eq!(stats.tool_results, 1);
        let r = recs(&lines);
        let results: Vec<&Value> = r.iter().filter(|m| m["message"]["role"] == "toolResult").collect();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0]["message"]["content"][0]["text"], "a.txt");
    }

    #[test]
    fn later_record_with_same_id_replaces_in_place() {
        let g1b = r#"{"id":"g0","timestamp":"2026-10-01T10:00:02.000Z","type":"gemini","content":"first draft"}"#;
        let u2 = r#"{"id":"u2","timestamp":"2026-10-01T10:00:03.000Z","type":"user","content":"next"}"#;
        let g1c = r#"{"id":"g0","timestamp":"2026-10-01T10:00:02.000Z","type":"gemini","content":"final"}"#;
        let (lines, stats) = to_journal_lines(&session(&[U1, g1b, u2, g1c])).unwrap();
        let r = recs(&lines);
        assert_eq!(stats.messages, 3);
        let ids: Vec<&str> = r[1..].iter().map(|m| m["id"].as_str().unwrap()).collect();
        assert_eq!(ids, ["u1", "g0", "u2"]);
        assert_eq!(r[2]["message"]["content"][0]["text"], "final");
    }

    #[test]
    fn rewind_removes_the_message_and_everything_after() {
        let u2 = r#"{"id":"u2","timestamp":"2026-10-01T10:00:05.000Z","type":"user","content":"try again"}"#;
        let (lines, _) = to_journal_lines(&session(&[U1, G1, R1, G2, r#"{"$rewindTo":"g1"}"#, u2])).unwrap();
        let r = recs(&lines);
        let ids: Vec<&str> = r[1..].iter().map(|m| m["id"].as_str().unwrap()).collect();
        assert_eq!(ids, ["u1", "u2"]);
        assert_eq!(r[2]["parentId"], "u1");
    }

    #[test]
    fn patch_replaces_content_and_removes_ids() {
        let g0 = r#"{"id":"g0","timestamp":"2026-10-01T10:00:02.000Z","type":"gemini","content":"partial"}"#;
        let u2 = r#"{"id":"u2","timestamp":"2026-10-01T10:00:03.000Z","type":"user","content":"drop me"}"#;
        let raw = session(&[
            U1, g0, u2,
            r#"{"$patch":{"id":"g0","content":[{"text":"consolidated"}],"updates":[]}}"#,
            r#"{"$patch":{"removeIds":["u2"]}}"#,
        ]);
        let (lines, stats) = to_journal_lines(&raw).unwrap();
        let r = recs(&lines);
        assert_eq!(r.len(), 3);
        assert_eq!(r[2]["id"], "g0");
        assert_eq!(r[2]["message"]["content"][0]["text"], "consolidated");
        assert_eq!(stats.ignored_patch_keys, 1);
    }

    #[test]
    fn ui_messages_and_bad_lines_are_skipped_and_counted() {
        let raw = session(&[
            U1,
            r#"{"id":"i1","timestamp":"2026-10-01T10:00:01.100Z","type":"info","content":"saved"}"#,
            r#"{"id":"e1","timestamp":"2026-10-01T10:00:01.200Z","type":"error","content":"quota"}"#,
            r#"{"id":"w1","timestamp":"2026-10-01T10:00:01.300Z","type":"warning","content":"slow"}"#,
            "this is not json {",
            G2,
        ]);
        let (lines, stats) = to_journal_lines(&raw).unwrap();
        assert_eq!(stats.skipped_ui_messages, 3);
        assert_eq!(stats.bad_lines, 1);
        let r = recs(&lines);
        assert_eq!(r.len(), 3);
        assert_eq!(r[2]["parentId"], "u1");
    }

    #[test]
    fn thoughts_are_dropped_and_signatures_never_copied() {
        let g = r#"{"id":"g1","timestamp":"2026-10-01T10:00:02.000Z","type":"gemini","content":[{"text":"pondering","thought":true,"thoughtSignature":"SIG-XYZ"},{"text":"answer"},{"functionCall":{"id":"c","name":"t","args":{}},"thoughtSignature":"SIG-ABC"}]}"#;
        let (lines, stats) = to_journal_lines(&session(&[U1, g])).unwrap();
        assert_eq!(stats.thoughts_kept, 1);
        let all = lines.join("\n");
        assert!(!all.contains("SIG-"), "thoughtSignature leaked: {all}");
        assert!(!all.contains("thoughtSignature"));
        assert!(all.contains("pondering"), "thought words are kept as a marker");
        assert!(all.contains("<prior_reasoning harness=\\\"gemini\\\""));
        let r = recs(&lines);
        assert_eq!(r[2]["message"]["content"][0]["text"].as_str().unwrap().contains("pondering"), true);
        assert_eq!(r[2]["message"]["content"][1]["text"], "answer");
    }

    #[test]
    fn inline_image_is_kept_and_file_data_skipped() {
        let u = r#"{"id":"u1","timestamp":"2026-10-01T10:00:01.000Z","type":"user","content":[{"text":"look"},{"inlineData":{"mimeType":"image/png","data":"iVBORw0KGgo="}},{"fileData":{"fileUri":"gs://x","mimeType":"image/png"}}]}"#;
        let (lines, stats) = to_journal_lines(&session(&[u])).unwrap();
        let r = recs(&lines);
        assert_eq!(r[1]["message"]["content"][1], json!({"type": "image", "data": "iVBORw0KGgo=", "mimeType": "image/png"}));
        assert_eq!(stats.images, 1);
        assert_eq!(stats.skipped_file_parts, 1);
    }

    #[test]
    fn legacy_tool_calls_become_tool_call_blocks_with_synthesized_results() {
        let g = r#"{"id":"g1","timestamp":"2026-10-01T10:00:02.000Z","type":"gemini","content":"Reading.","thoughts":[{"subject":"s","description":"d","timestamp":"t"}],"toolCalls":[{"id":"rf_1","name":"read_file","args":{"path":"a"},"status":"success","result":[{"functionResponse":{"id":"rf_1","name":"read_file","response":{"output":"contents"}}}]},{"id":"sh_1","name":"shell","args":"ls","status":"success"}]}"#;
        let (lines, stats) = to_journal_lines(&session(&[U1, g, G2])).unwrap();
        let r = recs(&lines);
        let c = &r[2]["message"]["content"];
        assert_eq!(c[0]["text"], "Reading.");
        assert_eq!(c[1], json!({"type": "toolCall", "id": "rf_1", "name": "read_file", "arguments": {"path": "a"}}));
        assert_eq!(c[2]["arguments"], json!({"input": "ls"}));
        assert_eq!(stats.skipped_thoughts, 1);
        assert_eq!(stats.synthesized_tool_results, 1);
        assert_eq!(r[3]["message"]["role"], "toolResult");
        assert_eq!(r[3]["message"]["toolCallId"], "rf_1");
        assert_eq!(r[3]["message"]["content"][0]["text"], "contents");
        assert_eq!(r[3]["parentId"], "g1");
        assert_eq!(r[4]["id"], "g2");
        assert_eq!(r[4]["parentId"], r[3]["id"]);
    }

    #[test]
    fn legacy_result_answered_by_a_user_response_is_not_duplicated() {
        let g = r#"{"id":"g1","timestamp":"2026-10-01T10:00:02.000Z","type":"gemini","content":"Listing.","toolCalls":[{"id":"ls__1","name":"list_directory","args":{},"result":[{"functionResponse":{"id":"ls__1","name":"list_directory","response":{"output":"a.txt"}}}]}]}"#;
        let (_, stats) = to_journal_lines(&session(&[U1, g, R1, G2])).unwrap();
        assert_eq!(stats.tool_results, 1);
        assert_eq!(stats.synthesized_tool_results, 0);
    }

    #[test]
    fn journal_is_readable_and_active_path_has_every_message() {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let (lines, _) = to_journal_lines(&session(&[U1, G1, R1, G2])).unwrap();
        let path = std::env::temp_dir().join(format!("hs-fromgemini-{}-{n}.jsonl", std::process::id()));
        std::fs::write(&path, lines.join("\n") + "\n").unwrap();
        let j = crate::journal::Journal::read(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        let active: Vec<String> = j.messages().iter().map(|m| m.id.to_string()).collect();
        let expected: Vec<String> = recs(&lines)[1..].iter().map(|m| m["id"].as_str().unwrap().to_string()).collect();
        assert_eq!(expected.len(), 4);
        assert_eq!(active, expected);
    }
}
