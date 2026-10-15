//! hsconv: harness-sync converter.
//!
//! The omp/Pi journal is the canonical form. Three outputs:
//!   to-pi      journal -> Pi journal   (projection: drop title slot, inline blobs, new id)
//!   to-omp     journal -> omp journal  (projection: add title slot, new id)
//!   to-opencode journal -> OpenCode import bundle (JSON on stdout or --out)
//!   to-claude  journal -> Claude transcript lines (--out file)
//! `inspect` prints what the reader sees, for differential tests.
//!
//! All ids are deterministic from --id (or the source id), so re-running
//! produces the same mirror and the daemon can replace in place.

mod claude;
mod from_claude;
mod from_opencode;
mod journal;
mod opencode;

use journal::{blobs_dir_for, inline_blobs, journal_filename, Journal};
use serde_json::{Map, Value};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process;

/// ISO-8601 UTC timestamp of a file's modification time, millisecond precision.
/// Falls back to the current time if the mtime cannot be read, never the epoch.
fn file_mtime_iso(path: &Path) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .unwrap_or_else(|| SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default());
    let ms = secs.as_millis() as i64;
    let (days, rem) = (ms.div_euclid(86_400_000), ms.rem_euclid(86_400_000));
    // civil-from-days (Howard Hinnant), proleptic Gregorian, UTC
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    let (h, mi, s, milli) = (rem / 3_600_000, rem / 60_000 % 60, rem / 1000 % 60, rem % 1000);
    format!("{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{s:02}.{milli:03}Z")
}

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1).cloned())
}
fn flag(args: &[String], name: &str) -> bool {
    args.iter().any(|a| a == name)
}

fn usage() -> ! {
    eprintln!(
        "hsconv inspect <journal>\n\
         hsconv to-pi <journal> --out-dir <pi-agent/sessions> [--id UUID] [--title T]\n\
         hsconv to-omp <journal> --out-dir <omp-agent/sessions> --home <HOME> [--id UUID] [--title T]\n\
         hsconv to-opencode <journal> [--out bundle.json] --id SES_ID --title T [--cli-version V] [--no-thinking]\n\
         hsconv from-claude <transcript.jsonl> --out <journal.jsonl>\n\
         hsconv from-opencode <export.json> --out <journal.jsonl>\n\
         hsconv to-claude <journal> --out-dir <~/.claude/projects> [--id UUID] [--cli-version V]\n\
         hsconv ids <seed>            (prints the deterministic opencode and claude ids)\n\
         Output path is printed on stdout for to-pi/to-omp/to-claude."
    );
    process::exit(1)
}

fn read_with_blobs(path: &Path) -> Result<(Journal, usize), String> {
    let mut j = Journal::read(path)?;
    let n = match blobs_dir_for(path) {
        Some(d) if d.is_dir() => inline_blobs(&mut j.records, &d)?,
        _ => 0,
    };
    Ok((j, n))
}

fn rewrite_ids(j: &mut Journal, new_id: &str) {
    for r in j.records.iter_mut() {
        if r.get("type").and_then(Value::as_str) == Some("session") {
            r.insert("id".into(), Value::String(new_id.to_string()));
        }
    }
}

fn set_title(j: &mut Journal, title: &str) {
    if let Some(t) = j.title.as_mut() {
        t.insert("title".into(), Value::String(title.to_string()));
    }
}

fn write_lines(path: &Path, lines: impl Iterator<Item = String>) -> Result<(), String> {
    if let Some(p) = path.parent() {
        fs::create_dir_all(p).map_err(|e| format!("{}: {e}", p.display()))?;
    }
    let tmp = path.with_extension("jsonl.tmp");
    let mut buf = String::new();
    for l in lines {
        buf.push_str(&l);
        buf.push('\n');
    }
    fs::write(&tmp, buf).map_err(|e| format!("{}: {e}", tmp.display()))?;
    fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(())
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str).unwrap_or("");
    let res: Result<(), String> = match cmd {
        "inspect" => {
            let p = PathBuf::from(args.get(1).unwrap_or_else(|| usage()));
            Journal::read(&p).map(|j| {
                let msgs = j.messages();
                let mut roles: std::collections::BTreeMap<&str, usize> = Default::default();
                let mut blocks: std::collections::BTreeMap<String, usize> = Default::default();
                for m in &msgs {
                    *roles.entry(m.role).or_default() += 1;
                    for b in &m.content {
                        let t = match b { Value::String(_) => "string".to_string(), Value::Object(o) => o.get("type").and_then(Value::as_str).unwrap_or("?").to_string(), _ => "?".into() };
                        *blocks.entry(t).or_default() += 1;
                    }
                }
                println!("flavor: {:?}\nsession_id: {}\ncwd: {}\ntitle: {}\nrecords: {}\nactive_messages: {}\nroles: {:?}\nblocks: {:?}",
                    j.flavor(), j.session_id().unwrap_or("-"), j.cwd().unwrap_or("-"), j.title_text(), j.records.len(), msgs.len(), roles, blocks);
            })
        }
        "from-claude" => {
            let p = PathBuf::from(args.get(1).unwrap_or_else(|| usage()));
            let out = arg(&args, "--out").unwrap_or_else(|| usage());
            fs::read_to_string(&p)
                .map_err(|e| format!("{}: {e}", p.display()))
                .and_then(|raw| from_claude::to_journal_lines(&raw))
                .and_then(|(lines, stats)| {
                    let mut text = lines.join("\n");
                    text.push('\n');
                    fs::write(&out, text).map_err(|e| format!("{out}: {e}"))?;
                    eprintln!("from-claude: {} message(s), {} text block(s), skipped {} tool / {} thinking / {} records",
                        stats.messages, stats.text_blocks, stats.skipped_tool_blocks, stats.skipped_thinking_blocks, stats.skipped_records);
                    println!("{out}");
                    Ok(())
                })
        }
        "from-opencode" => {
            let p = PathBuf::from(args.get(1).unwrap_or_else(|| usage()));
            let out = arg(&args, "--out").unwrap_or_else(|| usage());
            fs::read_to_string(&p)
                .map_err(|e| format!("{}: {e}", p.display()))
                .and_then(|raw| from_opencode::parse_export(&raw))
                .and_then(|v| from_opencode::to_journal_lines(&v))
                .and_then(|(lines, stats)| {
                    let mut text = lines.join("\n");
                    text.push('\n');
                    fs::write(&out, text).map_err(|e| format!("{out}: {e}"))?;
                    eprintln!("from-opencode: {} message(s), {} text part(s), skipped {} tool / {} reasoning",
                        stats.messages, stats.text_parts, stats.skipped_tool_parts, stats.skipped_reasoning_parts);
                    println!("{out}");
                    Ok(())
                })
        }
        "ids" => {
            let seed = args.get(1).unwrap_or_else(|| usage());
            println!("opencode {}\nclaude {}", opencode::session_id_for(seed), claude::session_id_for(seed));
            Ok(())
        }
        "to-pi" | "to-omp" => {
            let p = PathBuf::from(args.get(1).unwrap_or_else(|| usage()));
            let out_dir = PathBuf::from(arg(&args, "--out-dir").unwrap_or_else(|| usage()));
            read_with_blobs(&p).and_then(|(mut j, _)| {
                let src_id = j.session_id().unwrap_or("").to_string();
                let new_id = arg(&args, "--id").unwrap_or_else(|| src_id.clone());
                let cwd = j.cwd().unwrap_or("").to_string();
                // No header timestamp: use the source file's mtime, never the epoch
                // (the epoch put 1970 into mirror filenames).
                let ts = match j.timestamp() {
                    Some(t) => t.to_string(),
                    None => file_mtime_iso(&p),
                };
                rewrite_ids(&mut j, &new_id);
                let title = arg(&args, "--title").unwrap_or_else(|| j.title_text());
                let (bucket, head): (String, Option<String>) = if cmd == "to-pi" {
                    (journal::pi_bucket(&cwd), None)
                } else {
                    let home = arg(&args, "--home").or_else(|| env::var("USERPROFILE").ok()).unwrap_or_else(|| usage());
                    (journal::omp_bucket(&cwd, &home), Some(Journal::title_slot_line(&title, &ts)))
                };
                if cmd == "to-pi" {
                    // Pi has no title slot: the picker shows the last `session_info.name`.
                    // Append one so the mirror is labelled (and tagged as a mirror).
                    j.title = None;
                    let last_id = j.records.iter().rev().find_map(|r| r.get("id").and_then(Value::as_str)).map(str::to_string);
                    let mut info = Map::new();
                    info.insert("type".into(), "session_info".into());
                    info.insert("id".into(), Value::String(format!("{:08x}", j.records.len() as u32 ^ 0x5151_a5a5)));
                    info.insert("parentId".into(), last_id.map(Value::String).unwrap_or(Value::Null));
                    info.insert("timestamp".into(), Value::String(ts.clone()));
                    info.insert("name".into(), Value::String(title.clone()));
                    j.records.push(info);
                } else {
                    set_title(&mut j, &title);
                }
                let dest = out_dir.join(bucket).join(journal_filename(&ts, &new_id));
                let lines = head.into_iter().chain(j.records.iter().map(|r| serde_json::to_string(r).unwrap()));
                write_lines(&dest, lines)?;
                println!("{}", dest.display());
                Ok(())
            })
        }
        "to-opencode" => {
            let p = PathBuf::from(args.get(1).unwrap_or_else(|| usage()));
            read_with_blobs(&p).and_then(|(j, _)| {
                let seed = j.session_id().unwrap_or("").to_string();
                let sid = arg(&args, "--id").unwrap_or_else(|| opencode::session_id_for(&seed));
                let title = arg(&args, "--title").unwrap_or_else(|| j.title_text());
                let dir = j.cwd().unwrap_or("").to_string();
                let ver = arg(&args, "--cli-version").unwrap_or_else(|| "1.18.16".into());
                let bundle = opencode::to_bundle(&j, &opencode::Options { session_id: &sid, title: &title, directory: &dir, cli_version: &ver, include_thinking: !flag(&args, "--no-thinking") });
                let text = serde_json::to_string(&bundle).unwrap();
                match arg(&args, "--out") {
                    Some(o) => fs::write(&o, text).map_err(|e| format!("{o}: {e}")).map(|_| println!("{o}")),
                    None => { println!("{text}"); Ok(()) }
                }
            })
        }
        "to-claude" => {
            let p = PathBuf::from(args.get(1).unwrap_or_else(|| usage()));
            let out_dir = PathBuf::from(arg(&args, "--out-dir").unwrap_or_else(|| usage()));
            read_with_blobs(&p).and_then(|(j, _)| {
                let seed = j.session_id().unwrap_or("").to_string();
                let sid = arg(&args, "--id").unwrap_or_else(|| claude::session_id_for(&seed));
                let cwd = j.cwd().unwrap_or("").to_string();
                let ver = arg(&args, "--cli-version").unwrap_or_else(|| "2.1.245".into());
                let lines = claude::to_lines(&j, &claude::Options { session_id: &sid, cwd: &cwd, cli_version: &ver });
                if lines.is_empty() {
                    return Err("no resumable conversation (no user/assistant messages)".into());
                }
                let dest = out_dir.join(claude::bucket(&cwd)).join(format!("{sid}.jsonl"));
                write_lines(&dest, lines.into_iter())?;
                println!("{}", dest.display());
                Ok(())
            })
        }
        _ => usage(),
    };
    if let Err(e) = res {
        eprintln!("hsconv: {e}");
        process::exit(1);
    }
    let _ = Map::<String, Value>::new();
}
