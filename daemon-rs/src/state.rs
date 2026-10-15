//! Persisted daemon state. Same JSON layout as daemon/harness_sync.py State, so
//! an existing state.json keeps working across the port.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::io;
use std::path::Path;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Mirror {
    pub target: String,
    pub path: String,
    #[serde(default)]
    pub source_hash: String,
    #[serde(default)]
    pub installed_hash: String,
    #[serde(default)]
    pub diverged: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Source {
    pub fmt: String,
    pub key: String,
    pub last_seen_change: f64,
    pub last_hash: String,
    pub last_stamp: String,
    pub failures: u32,
    pub mirrors: HashMap<String, Mirror>,
}

impl Source {
    pub fn new(fmt: &str, key: &str) -> Self {
        Source { fmt: fmt.into(), key: key.into(), ..Default::default() }
    }
}

#[derive(Debug, Default)]
pub struct State {
    pub sources: HashMap<String, Source>,
    /// Every path or id the daemon created as a mirror. Changes to these are ignored.
    pub mirror_keys: BTreeSet<String>,
}

fn invalid(e: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, e.to_string())
}

impl State {
    /// Missing file means empty state. A corrupt file is an error: silently
    /// dropping it would forget every mirror and risk duplicate writes.
    pub fn load(path: &Path) -> io::Result<State> {
        if !path.exists() {
            return Ok(State::default());
        }
        let raw: Value = serde_json::from_str(&fs::read_to_string(path)?).map_err(invalid)?;
        let mut st = State::default();
        if let Some(srcs) = raw.get("sources").and_then(Value::as_object) {
            for (k, v) in srcs {
                let s = |f: &str| v.get(f).and_then(Value::as_str).unwrap_or("").to_string();
                let mut src = Source {
                    fmt: s("fmt"),
                    key: s("key"),
                    last_seen_change: v.get("last_seen_change").and_then(Value::as_f64).unwrap_or(0.0),
                    last_hash: s("last_hash"),
                    last_stamp: s("last_stamp"),
                    failures: v.get("failures").and_then(Value::as_u64).unwrap_or(0) as u32,
                    mirrors: HashMap::new(),
                };
                if let Some(ms) = v.get("mirrors").and_then(Value::as_object) {
                    for (t, mv) in ms {
                        src.mirrors.insert(t.clone(), serde_json::from_value(mv.clone()).map_err(invalid)?);
                    }
                }
                st.sources.insert(k.clone(), src);
            }
        }
        if let Some(keys) = raw.get("mirror_keys").and_then(Value::as_array) {
            st.mirror_keys = keys.iter().filter_map(Value::as_str).map(str::to_string).collect();
        }
        Ok(st)
    }

    /// Write atomically: a temp file, then rename over the state file.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let sources: BTreeMap<&String, Value> = self.sources.iter().map(|(k, s)| {
            let mirrors: BTreeMap<&String, &Mirror> = s.mirrors.iter().collect();
            (k, serde_json::json!({
                "fmt": s.fmt, "key": s.key, "last_seen_change": s.last_seen_change,
                "last_hash": s.last_hash, "last_stamp": s.last_stamp, "failures": s.failures,
                "mirrors": mirrors,
            }))
        }).collect();
        let data = serde_json::json!({
            "sources": sources,
            "mirror_keys": self.mirror_keys.iter().collect::<Vec<_>>(),
        });
        let tmp = path.with_extension("tmp");
        fs::write(&tmp, serde_json::to_string_pretty(&data).map_err(invalid)?)?;
        fs::rename(&tmp, path)
    }
}
