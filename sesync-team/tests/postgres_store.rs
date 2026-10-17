//! The Postgres store must behave like the in-memory store. It runs only when the test
//! database is named in TEAM_TEST_PG (e.g. a throwaway container). Otherwise it skips.
//! The store must hold ciphertext only, and must never be given a rule to decide.

use sesync_team::pg::PgStore;
use sesync_team::service::{Service, ServiceError};
use sesync_team::totp::code_at;

const T0: u64 = 1_000_020;

fn url() -> Option<String> {
    std::env::var("TEAM_TEST_PG").ok()
}

/// CI sets TEAM_REQUIRE_PG=1, so a missing database is a failure there, never a silent pass.
fn skip_or_fail(reason: &str) {
    if std::env::var("TEAM_REQUIRE_PG").is_ok() {
        panic!("TEAM_REQUIRE_PG is set but the Postgres tests cannot run: {reason}");
    }
    eprintln!("skipped: {reason}");
}

/// A service on a freshly reset schema. Each test gets its own schema, named after the test,
/// so tests can run in parallel without dropping each other's tables.
fn fresh(url: &str, schema: &str) -> Service<PgStore> {
    let store = PgStore::connect(&with_schema(url, schema), [5u8; 32]).expect("connect to the test database");
    store.reset().expect("reset the test schema");
    Service::new(store)
}

/// Point the connection at a per-test schema via the search_path option.
fn with_schema(url: &str, schema: &str) -> String {
    let sep = if url.contains('?') { '&' } else { '?' };
    format!("{url}{sep}options=-c%20search_path%3D{schema}")
}

fn make_schema(url: &str, schema: &str) {
    let mut c = postgres::Client::connect(url, postgres::NoTls).expect("connect for schema setup");
    c.batch_execute(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE; CREATE SCHEMA {schema};"))
        .expect("create test schema");
}

#[test]
fn the_postgres_store_matches_the_service_rules_and_holds_ciphertext() {
    let Some(url) = url() else {
        skip_or_fail("set TEAM_TEST_PG to a throwaway Postgres URL");
        return;
    };
    make_schema(&url, "t_pg_rules");
    let mut s = fresh(&url, "t_pg_rules");
    s.create_space("team-pg", &["alice"]).unwrap();
    let code = code_at(&s.space_secret("team-pg").unwrap(), T0);
    let token = s.enrol("team-pg", "alice", "box-1", &code, T0).unwrap();
    assert_eq!(s.authenticate("team-pg", &token, "box-1", T0 + 1).unwrap(), "alice");
    let lease = s.acquire("team-pg", "ses_1", "box-1", T0).unwrap();
    s.put_record("team-pg", "ses_1", "box-1", lease.version, b"plain session text".to_vec(), T0).unwrap();
    assert_eq!(s.get_record("team-pg", "ses_1", "alice").unwrap(), b"plain session text".to_vec());
    let raw = s.raw_stored("team-pg", "ses_1").unwrap();
    assert!(!raw.windows(18).any(|w| w == b"plain session text"), "the database must hold ciphertext");
    assert!(matches!(s.get_record("team-pg", "ses_1", "mallory"), Err(ServiceError::NotMember(_))));
}

#[test]
fn owners_and_the_token_key_survive_a_restart_on_postgres() {
    let Some(url) = url() else { skip_or_fail("TEAM_TEST_PG unset"); return };
    make_schema(&url, "t_restart");
    let mut s = fresh(&url, "t_restart");
    s.create_space("team-restart", &["alice"]).unwrap();
    let code = code_at(&s.space_secret("team-restart").unwrap(), T0);
    let token = s.enrol("team-restart", "alice", "box-1", &code, T0).unwrap();
    // A brand-new connection, as after a process restart.
    let restarted = Service::new(PgStore::connect(&with_schema(&url, "t_restart"), [5u8; 32]).expect("reconnect"));
    assert_eq!(restarted.authenticate("team-restart", &token, "box-1", T0 + 1).unwrap(), "alice",
        "the token must still verify after a restart");
    assert!(restarted.space_secret("team-restart").is_some(), "the space secret must persist");
}

#[test]
fn a_wrong_master_key_cannot_read_stored_records() {
    let Some(url) = url() else { skip_or_fail("TEAM_TEST_PG unset"); return };
    make_schema(&url, "t_key");
    let mut s = fresh(&url, "t_key");
    s.create_space("team-key", &["alice"]).unwrap();
    let code = code_at(&s.space_secret("team-key").unwrap(), T0);
    let token = s.enrol("team-key", "alice", "box-1", &code, T0).unwrap();
    let lease = s.acquire("team-key", "ses_1", "box-1", T0).unwrap();
    s.put_record("team-key", "ses_1", "box-1", lease.version, b"x".to_vec(), T0).unwrap();
    let _ = token;
    let wrong = Service::new(PgStore::connect(&with_schema(&url, "t_key"), [9u8; 32]).expect("connect with a different key"));
    // A different master key cannot unwrap the space, so its secret and token key are unreadable.
    assert!(wrong.space_secret("team-key").is_none(),
        "a different master key must not unlock the space secret");
    assert!(wrong.authenticate("team-key", &token, "box-1", T0 + 1).is_err(),
        "a different master key must not verify an existing token");
}
