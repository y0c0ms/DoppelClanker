//! Orphans and mirror listing, ported from daemon/hs_manage.py list_groups. A source
//! recorded in state whose file is gone stays listed as an orphan, so its leftover
//! mirrors can still be cleaned up. Renaming an orphan is refused: there is nothing to
//! rename. A mirror file is never listed as a source.

use harness_sync::manage::{list_groups, rename_group, Roots};
use harness_sync::state::{Mirror, Source, State};
use std::fs;
use std::path::PathBuf;

fn sandbox(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("hs-orphan-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

fn roots(sb: &PathBuf) -> Roots {
    Roots {
        omp: sb.join("omp").join("sessions"),
        pi: sb.join("pi").join("sessions"),
        claude: sb.join("claude").join("projects"),
    }
}

#[test]
fn a_source_in_state_with_no_file_is_listed_as_an_orphan_with_its_mirror() {
    let sb = sandbox("orphan");
    let r = roots(&sb);
    fs::create_dir_all(&r.omp).unwrap();
    let gone = r.omp.join("-tmp").join("2026-10-01T10-00-00-000Z_aaaaaaaa-1111-2222-3333-444444444444.jsonl");
    let mirror = r.pi.join("-tmp").join("mirror.jsonl");
    let mut state = State::default();
    let mut src = Source::new("omp", &gone.display().to_string());
    src.mirrors.insert("pi".into(), Mirror {
        target: "pi".into(), path: mirror.display().to_string(),
        source_hash: String::new(), installed_hash: String::new(), diverged: false,
    });
    state.sources.insert(gone.display().to_string(), src);

    let groups = list_groups(&r, &state);
    let g = groups.iter().find(|g| g.key == gone.display().to_string())
        .expect("an orphan must stay listed so its mirror can be cleaned");
    assert!(g.orphan, "a missing source must be flagged as an orphan");
    assert_eq!(g.mirrors.len(), 1, "its recorded mirror must be listed with it");
}

#[test]
fn renaming_an_orphan_is_refused_because_there_is_nothing_to_rename() {
    let sb = sandbox("rename-orphan");
    let r = roots(&sb);
    fs::create_dir_all(&r.omp).unwrap();
    let gone = r.omp.join("-tmp").join("2026-10-01T10-00-00-000Z_aaaaaaaa-1111-2222-3333-444444444444.jsonl");
    let mut state = State::default();
    state.sources.insert(gone.display().to_string(), Source::new("omp", &gone.display().to_string()));
    let g = list_groups(&r, &state).into_iter().find(|g| g.orphan).expect("orphan listed");
    let res = rename_group(&r, &g, "New", &sb.join("locks"), &sb.join("no-hslock"));
    assert!(res.is_err(), "renaming a missing file must be refused");
    assert!(!gone.exists(), "a refused rename must not create the file");
}

#[test]
fn a_mirror_path_is_never_listed_as_a_source() {
    let sb = sandbox("mirror");
    let r = roots(&sb);
    let dir = r.pi.join("-tmp");
    fs::create_dir_all(&dir).unwrap();
    let mirror = dir.join("2026-10-01T10-00-00-000Z_bbbbbbbb-1111-2222-3333-444444444444.jsonl");
    fs::write(&mirror, "{\"type\":\"session\"}\n").unwrap();
    let mut state = State::default();
    state.mirror_keys.insert(mirror.display().to_string());
    let groups = list_groups(&r, &state);
    assert!(groups.iter().all(|g| g.key != mirror.display().to_string()),
        "a file the daemon created as a mirror must not be a source");
}

#[test]
fn a_live_session_is_listed_and_not_an_orphan() {
    let sb = sandbox("live");
    let r = roots(&sb);
    let dir = r.omp.join("-tmp");
    fs::create_dir_all(&dir).unwrap();
    let p = dir.join("2026-10-01T10-00-00-000Z_cccccccc-1111-2222-3333-444444444444.jsonl");
    fs::write(&p, "{\"type\":\"session\",\"title\":\"Live\"}\n").unwrap();
    let groups = list_groups(&r, &State::default());
    let g = groups.iter().find(|g| g.key == p.display().to_string()).expect("live session listed");
    assert!(!g.orphan);
    assert_eq!(g.title, "Live");
}
