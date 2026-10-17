//! Helper for the local flow check: prints only the current 6-digit code for a team, so an
//! operator-style join can be exercised without ever printing the team secret. Skips unless
//! TEAM_FLOW_PG and TEAM_FLOW_SPACE are set.
use sesync_team::pg::PgStore;
use sesync_team::service::Service;
use sesync_team::totp::code_at;

#[test]
fn print_the_current_code_only() {
    let (Some(url), Some(space)) = (std::env::var("TEAM_FLOW_PG").ok(), std::env::var("TEAM_FLOW_SPACE").ok())
        else { return };
    let store = PgStore::connect(&url, [0u8; 32]);
    let _ = store;
    let key = std::fs::read(std::env::var("TEAM_FLOW_KEY").unwrap()).unwrap();
    let master: [u8; 32] = key.as_slice().try_into().unwrap();
    let svc = Service::new(PgStore::connect(&url, master).unwrap());
    let secret = svc.space_secret(&space).expect("team exists");
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    println!("CODE={}", code_at(&secret, now));
}
