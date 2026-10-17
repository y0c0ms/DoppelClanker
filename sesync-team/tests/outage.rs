//! When the database is down, a valid member must see "service unavailable" (503), never
//! "no such space" (404) or "not a member" (403). Those answers would make a client believe
//! its data is gone when only the database is unreachable.

use sesync_team::http::{self, EnrolBody, LeaseBody};
use sesync_team::service::{MemoryStore, Service, Store};
use sesync_team::totp::code_at;

const T0: u64 = 1_000_020;

/// A store that behaves normally until `down` is set, then answers as an unreachable database.
struct Flaky { inner: MemoryStore, down: std::rc::Rc<std::cell::Cell<bool>> }

impl Store for Flaky {
    fn load_space(&self, n: &str) -> Option<sesync_team::spaces::Space> {
        if self.down.get() { None } else { self.inner.load_space(n) }
    }
    fn save_space(&mut self, s: sesync_team::spaces::Space) { self.inner.save_space(s) }
    fn record(&self, s: &str, c: &str) -> Option<Vec<u8>> { if self.down.get() { None } else { self.inner.record(s, c) } }
    fn put_record(&mut self, s: &str, c: &str, v: Vec<u8>) { self.inner.put_record(s, c, v) }
    fn list_conversations(&self, s: &str) -> Vec<String> { if self.down.get() { vec![] } else { self.inner.list_conversations(s) } }
    fn load_gate(&self, s: &str, m: &str) -> Option<sesync_team::totp::JoinGate> { self.inner.load_gate(s, m) }
    fn save_gate(&mut self, s: &str, m: &str, g: sesync_team::totp::JoinGate) { self.inner.save_gate(s, m, g) }
    fn load_backup(&self, s: &str) -> Option<sesync_team::backup::BackupPolicy> { self.inner.load_backup(s) }
    fn save_backup(&mut self, s: &str, p: sesync_team::backup::BackupPolicy) { self.inner.save_backup(s, p) }
    fn load_token_key(&self, s: &str) -> Option<[u8; 32]> { if self.down.get() { None } else { self.inner.load_token_key(s) } }
    fn load_owner(&self, s: &str, m: &str) -> Option<String> { if self.down.get() { None } else { self.inner.load_owner(s, m) } }
    fn save_owner(&mut self, s: &str, m: &str, w: &str) { self.inner.save_owner(s, m, w) }
    fn ping(&self) -> bool { !self.down.get() }
}

fn setup() -> (Service<Flaky>, std::rc::Rc<std::cell::Cell<bool>>, String) {
    let down = std::rc::Rc::new(std::cell::Cell::new(false));
    let mut s = Service::new(Flaky { inner: MemoryStore::default(), down: down.clone() });
    s.create_space("team-a", &["alice"]).unwrap();
    let code = code_at(&s.space_secret("team-a").unwrap(), T0);
    let token = s.enrol("team-a", "alice", "box-1", &code, T0).unwrap();
    (s, down, format!("Bearer {token}"))
}

#[test]
fn a_listing_during_an_outage_is_503_not_404() {
    let (s, down, auth) = setup();
    down.set(true);
    let (status, json) = http::list_sessions(&s, Some(&auth), "team-a", "box-1", T0 + 1);
    assert_eq!(status, 503, "an outage must not look like a missing space: {json}");
}

#[test]
fn a_lease_during_an_outage_is_503_not_403_or_404() {
    let (s, down, auth) = setup();
    down.set(true);
    let body = LeaseBody { space: "team-a".into(), conversation: "c1".into(), machine: "box-1".into() };
    let (status, _) = http::acquire(&mut Service::new(Flaky{inner: MemoryStore::default(), down: down.clone()}), Some(&auth), &body, T0);
    assert_eq!(status, 503);
    let _ = s;
}

#[test]
fn a_join_during_an_outage_is_503_not_a_wrong_code() {
    let (mut s, down, _) = setup();
    down.set(true);
    let body = EnrolBody { space: "team-a".into(), member: "alice".into(), machine: "box-2".into(), code: "000000".into() };
    let (status, _) = http::enrol(&mut s, &body, T0);
    assert_eq!(status, 503, "a join during an outage is unavailable, not a refused code");
}

#[test]
fn health_reports_503_during_an_outage_and_recovers() {
    let (s, down, _) = setup();
    assert_eq!(http::health(&s).0, 200);
    down.set(true);
    assert_eq!(http::health(&s).0, 503);
    down.set(false);
    assert_eq!(http::health(&s).0, 200, "it recovers when the database returns");
}
