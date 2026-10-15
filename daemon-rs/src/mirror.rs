//! Deterministic mirror ids and content hashes. Ported from daemon/harness_sync.py.
//! These must match the Python values exactly: a different id creates a second
//! copy of every mirror instead of replacing the existing one.

use sha2::{Digest, Sha256};
use uuid::Uuid;

/// Same namespace as the Python daemon (`NAMESPACE`).
const NAMESPACE: &str = "5e55a0b1-7c3e-4d2a-9f1b-0a12ab34cd56";

/// uuid5 of "<source_key>|<target>", the id of a jsonl mirror.
pub fn mirror_uuid(source_key: &str, target: &str) -> String {
    let ns = Uuid::parse_str(NAMESPACE).expect("constant namespace");
    Uuid::new_v5(&ns, format!("{source_key}|{target}").as_bytes()).to_string()
}

const ALPHABET: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// "ses_" + 12 hex of sha256("<key>|opencode") + 14 base62 characters, the shape
/// OpenCode uses for session ids. Mirrors Python's byte pairs at [12..40) step 2.
pub fn mirror_opencode_id(source_key: &str) -> String {
    let digest = Sha256::digest(format!("{source_key}|opencode").as_bytes());
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    let mut tail = String::new();
    for i in (12..40).step_by(2) {
        let v = u8::from_str_radix(&hex[i..i + 2], 16).expect("hex") as usize;
        tail.push(ALPHABET[v % 62] as char);
    }
    format!("ses_{}{}", &hex[..12], tail)
}

/// Lowercase sha256 hex of a byte slice (the Python file_hash, for one buffer).
pub fn file_hash_bytes(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}
