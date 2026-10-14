//! Journal -> Claude Code project transcript (`~/.claude/projects/<bucket>/<uuid>.jsonl`).
//!
//! Shapes copied from a session-migrate mirror that Claude Code 2.1.245
//! resumed and recalled correctly (2026-10-12). Every record carries the
//! same envelope; the conversation is a `uuid`/`parentUuid` chain:
//!   user       -> {type:"user", message:{role:"user", content: string | [blocks]}}
//!   assistant  -> {type:"assistant", message:{id, type:"message", role, model,
//!                  content:[text | tool_use], stop_reason, stop_sequence, usage}}
//!   toolResult -> a user record whose content is [{type:"tool_result", tool_use_id, content, is_error}]
//!
//! Thinking is NOT carried: Claude signs thinking blocks per conversation and
//! rejects foreign ones on resume. This is the one edge where it is lost.
//!
//! Ids are uuid5 of (session seed, record id) so a re-run rewrites the same
//! uuids and the daemon's replace-in-place keeps working.

use crate::journal::{iso_from_ms, Journal};
use serde_json::{json, Map, Value};
use uuid::Uuid;

pub struct Options<'a> {
    pub session_id: &'a str,
    pub cwd: &'a str,
    pub cli_version: &'a str,
}

const NS: Uuid = Uuid::from_bytes([0x5e, 0x55, 0xa0, 0xb1, 0x7c, 0x3e, 0x4d, 0x2a, 0x9f, 0x1b, 0x0a, 0x12, 0xab, 0x34, 0xcd, 0x56]);

pub fn session_id_for(seed: &str) -> String {
    Uuid::new_v5(&NS, format!("{seed}|claude").as_bytes()).to_string()
}

pub fn to_lines(j: &Journal, o: &Options) -> Vec<String> {
    let msgs = j.messages();
    let mut out = Vec::new();
    let mut parent: Option<String> = None;
    let mut model = String::from("unknown");
    let mut n = 0u64;
    let mut uid = |rid: &str| {
        n += 1;
        Uuid::new_v5(&NS, format!("{}|{}|{}", o.session_id, rid, n).as_bytes()).to_string()
    };

    for m in &msgs {
        let ts = iso_from_ms(m.timestamp_ms);
        let (typ, message): (&str, Value) = match m.role {
            "user" => {
                let mut blocks: Vec<Value> = Vec::new();
                let mut only_text: Option<String> = None;
                for b in &m.content {
                    match b {
                        Value::String(s) => only_text = Some(s.clone()),
                        Value::Object(ob) => match ob.get("type").and_then(Value::as_str) {
                            Some("text") => blocks.push(json!({"type": "text", "text": ob.get("text").cloned().unwrap_or(Value::String(String::new()))})),
                            Some("image") => {
                                let data = ob.get("data").and_then(Value::as_str).unwrap_or("");
                                if !data.starts_with("blob:") && !data.is_empty() {
                                    blocks.push(json!({"type": "image", "source": {"type": "base64", "media_type": ob.get("mimeType").cloned().unwrap_or("image/png".into()), "data": data}}));
                                }
                            }
                            _ => {}
                        },
                        _ => {}
                    }
                }
                let content = match (only_text, blocks.len()) {
                    (Some(t), 0) => Value::String(t),
                    (_, 1) if blocks[0].get("type").and_then(Value::as_str) == Some("text") => blocks[0]["text"].clone(),
                    _ => Value::Array(blocks),
                };
                if content == Value::String(String::new()) || content == Value::Array(vec![]) {
                    continue;
                }
                ("user", json!({"role": "user", "content": content}))
            }
            "assistant" => {
                if let Some(mo) = m.message.get("model").and_then(Value::as_str) {
                    model = mo.to_string();
                }
                let mut blocks: Vec<Value> = Vec::new();
                for b in &m.content {
                    let Value::Object(ob) = b else { continue };
                    match ob.get("type").and_then(Value::as_str) {
                        Some("text") => {
                            let t = ob.get("text").and_then(Value::as_str).unwrap_or("");
                            if !t.is_empty() {
                                blocks.push(json!({"type": "text", "text": t}));
                            }
                        }
                        Some("toolCall") => blocks.push(json!({
                            "type": "tool_use",
                            "id": ob.get("id").cloned().unwrap_or(Value::String("call".into())),
                            "name": ob.get("name").cloned().unwrap_or(Value::String("tool".into())),
                            "input": ob.get("arguments").cloned().unwrap_or(Value::Object(Map::new()))
                        })),
                        _ => {} // thinking dropped by design
                    }
                }
                if blocks.is_empty() {
                    continue;
                }
                let stop = if blocks.iter().any(|b| b["type"] == "tool_use") { "tool_use" } else { "end_turn" };
                let mid = format!("msg_hsconv_{}", &Uuid::new_v5(&NS, m.id.as_bytes()).simple().to_string()[..24]);
                ("assistant", json!({
                    "id": mid, "type": "message", "role": "assistant", "model": model,
                    "content": blocks, "stop_reason": stop, "stop_sequence": null,
                    "usage": {"input_tokens": 0, "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0, "output_tokens": 0}
                }))
            }
            "toolResult" => {
                let cid = m.message.get("toolCallId").and_then(Value::as_str).unwrap_or("");
                let is_err = m.message.get("isError").and_then(Value::as_bool).unwrap_or(false);
                let text = m.content.iter().filter_map(|c| match c {
                    Value::String(s) => Some(s.as_str()),
                    Value::Object(x) => x.get("text").and_then(Value::as_str),
                    _ => None,
                }).collect::<Vec<_>>().join("\n");
                ("user", json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": cid, "content": text, "is_error": is_err}]}))
            }
            _ => continue,
        };
        let uuid = uid(m.id);
        let rec = json!({
            "parentUuid": parent, "isSidechain": false, "userType": "external",
            "cwd": o.cwd, "sessionId": o.session_id, "version": o.cli_version, "gitBranch": "",
            "uuid": uuid, "timestamp": ts, "type": typ, "message": message
        });
        out.push(serde_json::to_string(&rec).unwrap());
        parent = Some(uuid);
    }
    out
}

/// Claude's bucket: the absolute cwd with every non-alphanumeric char as '-'.
/// Verified: <drive>:\<home>\<project> -> C--<home>-<project>
pub fn bucket(cwd: &str) -> String {
    cwd.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect()
}
