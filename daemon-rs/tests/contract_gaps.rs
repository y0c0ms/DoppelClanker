//! The last rules in the essential modules that no other test reaches: the lock holder
//! contract (holder_of reports who holds a conversation), the O-penCode default-name
//! rule, the O-penCode no-database fast path, and the to-opencode converter guard. Each
//! checks a behaviour a user or a harness relies on.

use harness_sync::install::install_opencode;
use harness_sync::lock::holder_of;
use harness_sync::opencode::{clean_title, sessions};
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn sandbox(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("hs-contract-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

fn hslock() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../lock/target/release/hslock")
}

#[test]
fn holder_of_names_the_live_holder_of_a_conversation() {
    let hs = hslock();
    let sb = sandbox("holder");
    let lock_dir = sb.join("locks");
    fs::create_dir_all(&lock_dir).unwrap();
    let claim = Command::new(&hs).env("HARNESS_SYNC_LOCK_DIR", &lock_dir)
        .args(["claim", "conv-h", "omp", &std::process::id().to_string(), "t"]).output().unwrap();
    assert!(claim.status.success());
    let who = holder_of(&hs, Some(&lock_dir), "conv-h").expect("a held conversation has a holder");
    assert!(who.starts_with("omp "), "the holder is named as 'harness pid', got {who:?}");
}

#[test]
fn holder_of_is_none_for_a_free_conversation_and_when_the_binary_is_missing() {
    let hs = hslock();
    let sb = sandbox("free");
    let lock_dir = sb.join("locks");
    fs::create_dir_all(&lock_dir).unwrap();
    assert_eq!(holder_of(&hs, Some(&lock_dir), "nobody"), None);
    assert_eq!(holder_of(&PathBuf::from("/no/such/hslock"), Some(&lock_dir), "x"), None);
}

#[test]
fn an_opencode_title_that_is_only_the_default_name_is_not_a_title() {
    assert_eq!(clean_title("New session - 2026-10-14T16:52:39.241Z"), "");
    assert_eq!(clean_title("  New session - 2026-10-14T16:52:39Z  "), "");
}

#[test]
fn a_real_title_that_starts_like_the_default_is_kept() {
    assert_eq!(clean_title("New session plan"), "New session plan");
    assert_eq!(clean_title("New session - Q3 plan"), "New session - Q3 plan");
}

#[test]
fn a_missing_opencode_database_is_an_empty_list_and_is_not_created() {
    let sb = sandbox("nodb");
    let db = sb.join("absent").join("opencode.db");
    assert!(sessions(&db).is_empty());
    assert!(!db.exists(), "reading a missing store must not create it");
}

#[test]
fn a_failed_to_opencode_conversion_is_an_error_and_nothing_is_imported() {
    let sb = sandbox("convfail");
    let src = sb.join("src.jsonl");
    fs::write(&src, "{\"type\":\"session\",\"version\":3,\"id\":\"aaaaaaaa-1111-2222-3333-444444444444\",\"timestamp\":\"2026-10-01T10:00:00.000Z\",\"cwd\":\"/tmp\"}\n").unwrap();
    // A converter that fails: install must surface the error and never call import.
    let fake_hsconv = sb.join("failing-hsconv");
    fs::write(&fake_hsconv, "#!/bin/sh\necho 'bad journal' >&2\nexit 3\n").unwrap();
    let marker = sb.join("import-was-called");
    let fake_oc = sb.join("oc");
    fs::write(&fake_oc, format!("#!/bin/sh\ntouch {}\necho 'Imported session'\n", marker.display())).unwrap();
    for p in [&fake_hsconv, &fake_oc] {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(p, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let r = install_opencode(&fake_oc, &fake_hsconv, &src, "/", "ses_convfail00000000000000000", "t", &sb.join("xdg"));
    let err = r.expect_err("a failed conversion must be an error");
    assert!(err.contains("to-opencode failed"), "the guard must be what stops it: {err}");
    assert!(!marker.exists(), "import must not run when the conversion failed");
}
