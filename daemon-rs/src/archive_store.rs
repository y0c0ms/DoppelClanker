//! Compressed, verified copies of session files, kept under the session they belong to.
//!
//! Layout: <store>/<parent session id>/<file name>.hsar. The copy is written atomically
//! and read back through the archive's checksum, so a damaged copy is an error, never
//! silently returned. A parent id must be a plain name; it cannot climb out of the store.

use crate::archive::{pack, unpack, ArchiveError};
use std::fs;
use std::path::{Path, PathBuf};

/// Compression level: fast enough for live sync, small enough for transcripts.
const LEVEL: i32 = 6;

fn plain_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 200
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
        && !s.starts_with('.')
}

/// Store a copy of `src` under `store/<parent>/`. Returns the path of the stored copy.
pub fn store_session(store: &Path, parent: &str, src: &Path) -> Result<PathBuf, String> {
    if !plain_name(parent) {
        return Err(format!("refusing parent id that is not a plain name: {parent:?}"));
    }
    let name = src.file_name().and_then(|n| n.to_str()).ok_or("source has no file name")?;
    let dir = store.join(parent);
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let original = fs::read(src).map_err(|e| format!("{}: {e}", src.display()))?;
    let packed = pack(&original, LEVEL);
    let dst = dir.join(format!("{name}.hsar"));
    let tmp = dir.join(format!(".{name}.hsar.tmp"));
    fs::write(&tmp, &packed).map_err(|e| e.to_string())?;
    fs::rename(&tmp, &dst).map_err(|e| e.to_string())?;
    Ok(dst)
}

/// Read a stored copy back. A damaged or foreign copy is an error.
pub fn read_session(copy: &Path) -> Result<Vec<u8>, ArchiveError> {
    let packed = fs::read(copy).map_err(|e| ArchiveError::Io(e.to_string()))?;
    unpack(&packed)
}
