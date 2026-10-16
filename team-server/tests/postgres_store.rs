//! The Postgres store must behave like the in-memory store. It runs only when the test
//! database is named in TEAM_TEST_PG (e.g. a throwaway container). Otherwise it skips.
//! The store must hold ciphertext only, and must never be given a rule to decide.

use harness_sync_team::pg::PgStore;
use harness_sync_team::service::{Service, ServiceError};
use harness_sync_team::totp::code_at;

const SECRET: &[u8] = b"12345678901234567890";
const T0: u64 = 1_000_020;

fn url() -> Option<String> {
    std::env::var("TEAM_TEST_PG").ok()
}

#[test]
fn the_postgres_store_matches_the_service_rules_and_holds_ciphertext() {
    let Some(url) = url() else {
        eprintln!("skipped: set TEAM_TEST_PG to a throwaway Postgres URL");
        return;
    };
    let master = [5u8; 32];
    let store = PgStore::connect(&url, master).expect("connect to the test database");
    store.reset().expect("reset the test schema");
    let mut s = Service::new(store);
    s.create_space("team-pg", &["alice"]).unwrap();
    s.join("team-pg", "alice", "box-1", &code_at(SECRET, T0), T0).unwrap();
    let lease = s.acquire("team-pg", "ses_1", "box-1", T0).unwrap();
    s.put_record("team-pg", "ses_1", "box-1", lease.version, b"plain session text".to_vec(), T0).unwrap();
    assert_eq!(s.get_record("team-pg", "ses_1", "alice").unwrap(), b"plain session text".to_vec());
    let raw = s.raw_stored("team-pg", "ses_1").unwrap();
    assert!(!raw.windows(18).any(|w| w == b"plain session text"), "the database must hold ciphertext");
    assert!(matches!(s.get_record("team-pg", "ses_1", "mallory"), Err(ServiceError::NotMember(_))));
}
