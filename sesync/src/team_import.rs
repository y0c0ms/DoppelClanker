//! Confirmed import of a team session into a local harness. This is the only place a team
//! session becomes a local file, and it needs an explicit confirmation from the user.

use std::io::Write;
use std::path::Path;

#[derive(Debug, PartialEq, Eq)]
pub enum ImportOutcome { Written }

#[derive(Debug, PartialEq, Eq)]
pub enum ImportError {
    /// The user did not confirm. Nothing is written.
    NotConfirmed,
    /// A local session already exists at the target. It is kept as it is.
    WouldOverwrite,
    /// The file could not be written.
    Io(String),
}

/// Write `bytes` to `target`, but only if the user confirmed and nothing is there yet. The file
/// is created exclusively, so a race with a local write cannot replace it either.
pub fn import_confirmed(target: &Path, bytes: &[u8], confirmed: bool) -> Result<ImportOutcome, ImportError> {
    if !confirmed {
        return Err(ImportError::NotConfirmed);
    }
    if target.exists() {
        return Err(ImportError::WouldOverwrite);
    }
    if let Some(dir) = target.parent() {
        std::fs::create_dir_all(dir).map_err(|e| ImportError::Io(e.to_string()))?;
    }
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(target).map_err(|e| {
        if e.kind() == std::io::ErrorKind::AlreadyExists { ImportError::WouldOverwrite }
        else { ImportError::Io(e.to_string()) }
    })?;
    f.write_all(bytes).map_err(|e| ImportError::Io(e.to_string()))?;
    f.sync_all().ok();
    Ok(ImportOutcome::Written)
}
