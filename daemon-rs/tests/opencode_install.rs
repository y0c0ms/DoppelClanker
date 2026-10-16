//! O-penCode mirror install, ported from daemon/harness_sync.py install_opencode.
//! Converts with the real hsconv to-opencode, then imports with `opencode import`.
//! Every test runs the real CLI with XDG_* pointed at a sandbox, so the real store
//! is never written. Skipped when the CLI is not installed.

use harness_sync::install::install_opencode;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn hsconv() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../hsconv/target/release/hsconv")
}

fn sandbox(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("hs-ocinst-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

fn opencode_cli() -> Option<PathBuf> {
    // Real harness CLIs run only when HS_E2E=1. Plain `cargo test` never starts them,
    // so a local run cannot touch a live session store or call a model.
    if std::env::var("HS_E2E").map_or(true, |v| v != "1") {
        return None;
    }
    std::env::var_os("PATH").and_then(|p| {
        std::env::split_paths(&p).map(|d| d.join("opencode")).find(|c| c.is_file())
    })
}

/// Point every XDG store at the sandbox, so the import lands there and nowhere else.
fn isolate<'a>(cmd: &'a mut Command, sb: &PathBuf) -> &'a mut Command {
    cmd.env("XDG_DATA_HOME", sb.join("data"))
        .env("XDG_CONFIG_HOME", sb.join("cfg"))
        .env("XDG_STATE_HOME", sb.join("state"))
        .env("XDG_CACHE_HOME", sb.join("cache"))
}

fn omp_journal(dir: &PathBuf) -> PathBuf {
    let p = dir.join("src.jsonl");
    fs::write(&p, "{\"type\":\"session\",\"version\":3,\"id\":\"aaaaaaaa-1111-2222-3333-444444444444\",\"timestamp\":\"2026-10-01T10:00:00.000Z\",\"cwd\":\"/tmp/proj\"}\n\
{\"type\":\"message\",\"id\":\"m1\",\"parentId\":null,\"timestamp\":\"2026-10-01T10:00:01.000Z\",\"message\":{\"role\":\"user\",\"content\":\"hello opencode mirror\"}}\n").unwrap();
    p
}

#[test]
fn a_mirror_is_imported_into_the_sandbox_store_and_listed() {
    let Some(cli) = opencode_cli() else { eprintln!("skipped: opencode CLI not installed"); return; };
    let sb = sandbox("import");
    let src = omp_journal(&sb);
    let id = "ses_installtest0000000000000000";
    let sid = install_opencode(&cli, &hsconv(), &src, "/tmp/proj", id, "Install test", &sb.join("xdg-env"))
        .expect("install must succeed");
    assert_eq!(sid, id);
    let mut list = Command::new(&cli);
    list.args(["session", "list"]);
    isolate(&mut list, &sb.join("xdg-env"));
    let out = String::from_utf8_lossy(&list.output().unwrap().stdout).into_owned();
    assert!(out.contains("Install test"), "the mirror must be listed by opencode: {out}");
}

#[test]
fn a_failed_import_is_an_error() {
    let sb = sandbox("failimport");
    let src = omp_journal(&sb);
    let fake = sb.join("fake-opencode");
    fs::write(&fake, "#!/bin/sh\nif [ \"$1\" = import ]; then echo 'bad bundle' >&2; exit 4; fi\nexit 0\n").unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let r = install_opencode(&fake, &hsconv(), &src, "/tmp", "ses_failimport00000000000000000", "t", &sb.join("xdg-env"));
    assert!(r.is_err(), "a failed import must surface as an error");
}

#[test]
fn an_import_that_does_not_say_imported_is_an_error() {
    // The Python daemon requires the 'Imported session' line. Exit 0 with other output
    // is not a successful import.
    let sb = sandbox("noline");
    let src = omp_journal(&sb);
    let fake = sb.join("quiet-opencode");
    fs::write(&fake, "#!/bin/sh\necho 'something else'\nexit 0\n").unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let r = install_opencode(&fake, &hsconv(), &src, "/tmp", "ses_noline0000000000000000000", "t", &sb.join("xdg-env"));
    assert!(r.is_err(), "no 'Imported session' line means the import is not confirmed");
}
