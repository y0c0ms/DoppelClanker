//! Mirror install. Ported from daemon/harness_sync.py install_jsonl and
//! _install_jsonl_hsconv: the converter writes the mirror, and the file is
//! replaced atomically so a reader never sees a half-written mirror.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub enum InstallTarget {
    /// Pi journal store: `<agent_dir>/sessions/<bucket>/<file>.jsonl`.
    Pi { agent_dir: PathBuf },
    /// omp journal store: `<agent_dir>/sessions/<bucket>/<file>.jsonl`.
    Omp { agent_dir: PathBuf, home: PathBuf },
    /// Claude Code store: `<claude_dir>/projects/<bucket>/<uuid>.jsonl`.
    Claude { claude_dir: PathBuf },
}

/// Convert `src` (an omp/Pi journal) into the target store, with a deterministic
/// mirror id, and return the installed path. Any converter failure is an error,
/// and nothing is left in the store.
pub fn install_journal(
    hsconv: &Path,
    src: &Path,
    target: InstallTarget,
    cwd: &str,
    mirror_id: &str,
    title: &str,
) -> Result<PathBuf, String> {
    let _ = cwd;
    let scratch = std::env::temp_dir().join(format!("hs-install-scratch-{}-{}", std::process::id(), mirror_id));
    let _ = fs::remove_dir_all(&scratch);
    fs::create_dir_all(&scratch).map_err(|e| e.to_string())?;

    let (sub, verb, store, extra): (&str, &str, &Path, Vec<String>) = match &target {
        InstallTarget::Pi { agent_dir } => ("sessions", "to-pi", agent_dir, vec![]),
        InstallTarget::Omp { agent_dir, home } => (
            "sessions", "to-omp", agent_dir,
            vec!["--home".into(), home.display().to_string()],
        ),
        InstallTarget::Claude { claude_dir } => ("projects", "to-claude", claude_dir, vec![]),
    };
    let out_dir = scratch.join(sub);
    let mut cmd = Command::new(hsconv);
    cmd.arg(verb).arg(src).arg("--out-dir").arg(&out_dir).args(&extra);
    // to-claude takes no title; every other target gets --id and --title.
    if verb == "to-claude" {
        cmd.args(["--id", mirror_id]);
    } else {
        cmd.args(["--id", mirror_id, "--title", title]);
    }
    let out = cmd.output().map_err(|e| format!("cannot run hsconv: {e}"))?;
    if !out.status.success() {
        let _ = fs::remove_dir_all(&scratch);
        return Err(format!("hsconv {verb} failed: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }

    let produced = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let produced = PathBuf::from(produced.lines().last().unwrap_or("").trim());
    if !produced.is_file() {
        let _ = fs::remove_dir_all(&scratch);
        return Err(format!("hsconv printed no output file: {}", produced.display()));
    }

    let name = produced.file_name().ok_or("no file name")?.to_owned();
    let bucket = produced.parent().and_then(Path::file_name).ok_or("no bucket")?.to_owned();
    let dest_dir = store.join(sub).join(&bucket);
    fs::create_dir_all(&dest_dir).map_err(|e| e.to_string())?;
    // Remove an older mirror with the same id, whatever its timestamp prefix.
    if let Ok(rd) = fs::read_dir(&dest_dir) {
        for e in rd.flatten() {
            let n = e.file_name();
            if n.to_string_lossy().ends_with(&format!("{mirror_id}.jsonl")) {
                let _ = fs::remove_file(e.path());
            }
        }
    }
    let dest = dest_dir.join(&name);
    let tmp = dest.with_extension("jsonl.tmp");
    fs::copy(&produced, &tmp).map_err(|e| e.to_string())?;
    fs::rename(&tmp, &dest).map_err(|e| e.to_string())?;
    let _ = fs::remove_dir_all(&scratch);
    Ok(dest)
}
