//! The production schema step. It must create a fresh schema, be a no-op the second time, and
//! never touch existing data. Runs only against TEAM_TEST_PG, in its own schema.

use sesync_team::pg::PgStore;
use sesync_team::service::Service;

fn schema_url(url: &str, schema: &str) -> String {
    let sep = if url.contains('?') { '&' } else { '?' };
    format!("{url}{sep}options=-c%20search_path%3D{schema}")
}

fn fresh_schema(url: &str, schema: &str) {
    let mut c = postgres::Client::connect(url, postgres::NoTls).unwrap();
    c.batch_execute(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE; CREATE SCHEMA {schema};")).unwrap();
}

#[test]
fn migrate_creates_a_fresh_schema_and_is_idempotent() {
    let Some(url) = std::env::var("TEAM_TEST_PG").ok() else { return };
    fresh_schema(&url, "t_migrate_fresh");
    let store = PgStore::connect(&schema_url(&url, "t_migrate_fresh"), [4u8; 32]).unwrap();
    store.migrate().expect("first migrate creates the tables");
    store.migrate().expect("second migrate is a no-op");
    let mut s = Service::new(store);
    s.create_space("team-m", &["alice"]).expect("the schema accepts a space");
}

#[test]
fn migrate_never_drops_existing_data() {
    let Some(url) = std::env::var("TEAM_TEST_PG").ok() else { return };
    fresh_schema(&url, "t_migrate_keep");
    let store = PgStore::connect(&schema_url(&url, "t_migrate_keep"), [4u8; 32]).unwrap();
    store.migrate().unwrap();
    let mut s = Service::new(store);
    s.create_space("team-keep", &["alice"]).unwrap();
    // Run the production step again over the live data.
    let again = PgStore::connect(&schema_url(&url, "t_migrate_keep"), [4u8; 32]).unwrap();
    again.migrate().expect("migrate over existing data");
    // A fresh connection, so the check reads what is stored, not what this process holds.
    let reread = PgStore::connect(&schema_url(&url, "t_migrate_keep"), [4u8; 32]).unwrap();
    assert!(Service::new(reread).space_secret("team-keep").is_some(), "the space survives migrate");
}
