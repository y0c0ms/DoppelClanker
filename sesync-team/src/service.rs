//! The team service: the rules the HTTP layer enforces, over the tested core modules.
//! Storage is behind the `Store` trait. `MemoryStore` is used by the tests; Postgres
//! implements the same trait for the server. No rule lives in the storage layer.

use crate::backup::{BackupPolicy, Retention};
use crate::lease::{Lease, LeaseError, LeaseTable};
use crate::spaces::{Space, SpaceError};
use crate::totp::{JoinError, JoinGate};
use std::collections::HashMap;

#[derive(Debug, PartialEq, Eq)]
pub enum ServiceError {
    /// The store could not answer. Not a judgement on the request: retry later.
    Unavailable,
    /// The request conflicts with what exists, such as creating a team that already exists.
    Conflict(String),
    NoSpace(String),
    NotMember(String),
    Auth(String),
    Lease(String),
    Crypto(String),
    Policy(String),
}

impl From<LeaseError> for ServiceError {
    fn from(e: LeaseError) -> Self { ServiceError::Lease(format!("{e:?}")) }
}

impl From<SpaceError> for ServiceError {
    fn from(e: SpaceError) -> Self { ServiceError::Crypto(e.to_string()) }
}

impl From<JoinError> for ServiceError {
    fn from(e: JoinError) -> Self { ServiceError::Auth(format!("{e:?}")) }
}

/// Persistence for the service. Implemented in memory for tests and by Postgres for the
/// server. It stores what the service gives it; it makes no decisions.
pub trait Store {
    /// Load a space by value (or None). Owned, so a remote store works the same way.
    fn load_space(&self, name: &str) -> Option<Space>;
    fn save_space(&mut self, space: Space);
    fn record(&self, space: &str, conv: &str) -> Option<Vec<u8>>;
    fn put_record(&mut self, space: &str, conv: &str, sealed: Vec<u8>);
    /// Conversation ids stored in a space, sorted. Ids only; contents stay sealed.
    fn list_conversations(&self, space: &str) -> Vec<String>;
    /// True if the store answers a trivial query. Used by the health check.
    fn ping(&self) -> bool;
    fn load_gate(&self, space: &str, machine: &str) -> Option<JoinGate>;
    fn save_gate(&mut self, space: &str, machine: &str, gate: JoinGate);
    fn load_backup(&self, space: &str) -> Option<BackupPolicy>;
    fn save_backup(&mut self, space: &str, policy: BackupPolicy);
    fn load_token_key(&self, space: &str) -> Option<[u8; 32]>;
    fn load_owner(&self, space: &str, machine: &str) -> Option<String>;
    fn save_owner(&mut self, space: &str, machine: &str, member: &str);
    /// When this machine's enrolment was confirmed by the code. Set once; never moved forward.
    fn load_enrolled_at(&self, space: &str, machine: &str) -> Option<u64>;
    fn save_enrolled_at(&mut self, space: &str, machine: &str, at: u64);
}

#[derive(Default)]
pub struct MemoryStore {
    spaces: HashMap<String, Space>,
    records: HashMap<(String, String), Vec<u8>>,
    gates: HashMap<(String, String), JoinGate>,
    backups: HashMap<String, BackupPolicy>,
    owners: HashMap<(String, String), String>,
    enrolled_at: HashMap<(String, String), u64>,
}

impl Store for MemoryStore {
    fn load_space(&self, name: &str) -> Option<Space> { self.spaces.get(name).and_then(|s| Space::from_bytes(&s.to_bytes())) }
    fn save_space(&mut self, space: Space) { self.spaces.insert(space.name.clone(), space); }
    fn record(&self, space: &str, conv: &str) -> Option<Vec<u8>> {
        self.records.get(&(space.to_string(), conv.to_string())).cloned()
    }
    fn ping(&self) -> bool { true }
    fn list_conversations(&self, space: &str) -> Vec<String> {
        let mut ids: Vec<String> = self.records.keys()
            .filter(|(s, _)| s == space)
            .map(|(_, c)| c.clone())
            .collect();
        ids.sort();
        ids
    }
    fn put_record(&mut self, space: &str, conv: &str, sealed: Vec<u8>) {
        self.records.insert((space.to_string(), conv.to_string()), sealed);
    }
    fn load_gate(&self, space: &str, machine: &str) -> Option<JoinGate> {
        self.gates.get(&(space.to_string(), machine.to_string())).map(|g| g.clone_state())
    }
    fn save_gate(&mut self, space: &str, machine: &str, gate: JoinGate) {
        self.gates.insert((space.to_string(), machine.to_string()), gate);
    }
    fn load_backup(&self, space: &str) -> Option<BackupPolicy> { self.backups.get(space).cloned() }
    fn load_token_key(&self, space: &str) -> Option<[u8; 32]> {
        self.spaces.get(space).and_then(|sp| sp.token_key())
    }
    fn load_owner(&self, space: &str, machine: &str) -> Option<String> {
        self.owners.get(&(space.to_string(), machine.to_string())).cloned()
    }
    fn load_enrolled_at(&self, space: &str, machine: &str) -> Option<u64> {
        self.enrolled_at.get(&(space.to_string(), machine.to_string())).copied()
    }
    fn save_enrolled_at(&mut self, space: &str, machine: &str, at: u64) {
        self.enrolled_at.entry((space.to_string(), machine.to_string())).or_insert(at);
    }
    fn save_owner(&mut self, space: &str, machine: &str, member: &str) {
        self.owners.insert((space.to_string(), machine.to_string()), member.to_string());
    }
    fn save_backup(&mut self, space: &str, policy: BackupPolicy) {
        self.backups.insert(space.to_string(), policy);
    }
}

/// Token lifetime: 24 hours. After that the machine must confirm the authenticator again.
pub const TOKEN_LIFETIME_SECS: u64 = 24 * 60 * 60;

/// A member can renew silently for this long after the code-confirmed enrolment. After that the
/// authenticator code is required again.
pub const RENEW_WINDOW_SECS: u64 = 7 * 24 * 60 * 60;

pub struct Service<S: Store> {
    store: S,
    /// Leases are in-memory by design: they are short-lived and a restart ends them safely
    /// (every holder must re-acquire, which the fencing version then enforces).
    leases: LeaseTable,
}

impl<S: Store> Service<S> {
    pub fn new(store: S) -> Self {
        Service { store, leases: LeaseTable::default() }
    }

    /// Create a space. Refused if it already exists: replacing one would replace its secret and
    /// content keys, which would lock every member out of their data.
    pub fn create_space(&mut self, name: &str, members: &[&str]) -> Result<(), ServiceError> {
        if self.store.load_space(name).is_some() {
            return Err(ServiceError::Conflict(format!("team '{name}' already exists")));
        }
        // A random 160-bit secret per space, stored with the space so it is wrapped at rest.
        let mut secret = vec![0u8; 20];
        rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut secret);
        let mut space = Space::new(name, members);
        space.set_secret(secret);
        let mut tk = [0u8; 32];
        rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut tk);
        space.set_token_key(tk);
        self.store.save_space(space);
        Ok(())
    }

    /// Add a member to an existing space. Inviting someone already in it does nothing.
    pub fn invite(&mut self, space: &str, who: &str) -> Result<(), ServiceError> {
        let mut s = self.store.load_space(space).ok_or_else(|| ServiceError::NoSpace(space.into()))?;
        s.add_member(who);
        self.store.save_space(s);
        Ok(())
    }

    /// Whether `who` is a member of `space`.
    pub fn is_member(&self, space: &str, who: &str) -> bool {
        self.member(space, who).is_ok()
    }

    /// The space's authenticator secret, for enrolling the authenticator app. Only the
    /// operator who creates a space sees it; the server never returns it over HTTP.
    pub fn space_secret(&self, space: &str) -> Option<Vec<u8>> {
        self.store.load_space(space).and_then(|s| s.secret().map(|b| b.to_vec()))
    }

    /// Consume the service and return its store, so a restart can be simulated.
    pub fn into_store(self) -> S {
        self.store
    }

    fn member(&self, space: &str, who: &str) -> Result<(), ServiceError> {
        let s = self.store.load_space(space).ok_or_else(|| ServiceError::NoSpace(space.into()))?;
        s.seal_for_member_check(who).map_err(|_| ServiceError::NotMember(who.into()))
    }

    /// A machine joins a space with the authenticator code for `now`.
    pub fn join(&mut self, space: &str, who: &str, machine: &str, code: &str, now: u64) -> Result<(), ServiceError> {
        self.check_store()?;
        self.member(space, who)?;
        let secret = self.space_secret(space).ok_or_else(|| ServiceError::NoSpace(space.into()))?;
        // Remember which member this machine joined as. Stored, so it survives a restart.
        self.store.save_owner(space, machine, who);
        let mut gate = self.store.load_gate(space, machine).unwrap_or_else(|| JoinGate::new(secret.clone()));
        let r = gate.verify(code, now);
        self.store.save_gate(space, machine, gate);
        r.map_err(ServiceError::from)
    }

    /// Enrol a machine: confirm the authenticator code, then issue a token bound to that
    /// machine. The token is the only credential the HTTP layer accepts for later calls.
    pub fn enrol(&mut self, space: &str, who: &str, machine: &str, code: &str, now: u64)
        -> Result<String, ServiceError>
    {
        self.join(space, who, machine, code, now)?;
        self.store.save_enrolled_at(space, machine, now);
        let key = self.store.load_token_key(space).ok_or_else(|| ServiceError::NoSpace(space.into()))?;
        Ok(crate::auth::issue_token(&key, who, machine, now + TOKEN_LIFETIME_SECS))
    }

    /// Renew a member token without the code. Allowed only if the token is valid for this
    /// machine (or was valid recently enough to prove possession), and the enrolment is still
    /// inside the 7-day window. The new token never outlives the window.
    pub fn renew(&mut self, space: &str, token: &str, machine: &str, now: u64) -> Result<String, ServiceError> {
        let key = self.store.load_token_key(space).ok_or_else(|| ServiceError::NoSpace(space.into()))?;
        let member = crate::auth::check_token_ignoring_expiry(&key, token, machine)
            .map_err(|e| ServiceError::Auth(format!("{e:?}")))?;
        let enrolled = self.store.load_enrolled_at(space, machine)
            .ok_or_else(|| ServiceError::Auth("not enrolled on this machine".into()))?;
        let window_end = enrolled + RENEW_WINDOW_SECS;
        if now >= window_end {
            return Err(ServiceError::Auth("enrolment window passed; the authenticator code is needed".into()));
        }
        let expires = (now + TOKEN_LIFETIME_SECS).min(window_end);
        Ok(crate::auth::issue_token(&key, &member, machine, expires))
    }

    /// Check a token from a machine and return the member it was issued to.
    pub fn authenticate(&self, space: &str, token: &str, machine: &str, now: u64) -> Result<String, ServiceError> {
        self.check_store()?;
        let key = self.store.load_token_key(space).ok_or_else(|| ServiceError::NoSpace(space.into()))?;
        crate::auth::check_token(&key, token, machine, now)
            .map_err(|e| ServiceError::Auth(format!("{e:?}")))
    }

    pub fn acquire(&mut self, space: &str, conv: &str, machine: &str, now: u64) -> Result<Lease, ServiceError> {
        self.check_store()?;
        self.store.load_space(space).ok_or_else(|| ServiceError::NoSpace(space.into()))?;
        Ok(self.leases.acquire(&format!("{space}/{conv}"), machine, now)?)
    }

    pub fn release(&mut self, space: &str, conv: &str, machine: &str) -> Result<(), ServiceError> {
        Ok(self.leases.release(&format!("{space}/{conv}"), machine)?)
    }

    /// Store a sealed record. Only the holder of the live lease, under its current version,
    /// may write. The content is sealed with the space key before it is stored.
    pub fn put_record(&mut self, space: &str, conv: &str, machine: &str, version: u64,
                      plaintext: Vec<u8>, now: u64) -> Result<(), ServiceError> {
        let key = format!("{space}/{conv}");
        self.leases.check_write(&key, version, machine)?;
        let _ = now;
        let owner = self.store.load_owner(space, machine)
            .ok_or_else(|| ServiceError::NotMember(machine.into()))?;
        let mut s = self.store.load_space(space).ok_or_else(|| ServiceError::NoSpace(space.into()))?;
        let sealed = s.seal_for(&owner, &plaintext)?;
        self.store.save_space(s);
        self.store.put_record(space, conv, sealed);
        Ok(())
    }

    pub fn get_record(&self, space: &str, conv: &str, who: &str) -> Result<Vec<u8>, ServiceError> {
        self.check_store()?;
        self.member(space, who)?;
        let sealed = self.store.record(space, conv).ok_or_else(|| ServiceError::NoSpace(conv.into()))?;
        let s = self.store.load_space(space).ok_or_else(|| ServiceError::NoSpace(space.into()))?;
        Ok(s.open_for(who, &sealed)?)
    }

    /// Refuse with Unavailable when the store cannot answer, so an outage is never mistaken
    /// for a missing space or a refused code.
    fn check_store(&self) -> Result<(), ServiceError> {
        if self.store.ping() { Ok(()) } else { Err(ServiceError::Unavailable) }
    }

    /// Whether the store is reachable. The health check reports this, nothing else.
    pub fn healthy(&self) -> bool {
        self.store.ping()
    }

    /// The conversation ids in a space. Only a member may list them.
    pub fn list_conversations(&self, space: &str, who: &str) -> Result<Vec<String>, ServiceError> {
        self.check_store()?;
        self.member(space, who)?;
        Ok(self.store.list_conversations(space))
    }

    /// The stored bytes, for checks that the store never holds plaintext.
    pub fn raw_stored(&self, space: &str, conv: &str) -> Option<Vec<u8>> {
        self.store.record(space, conv)
    }

    /// Save a backup policy. Refused if the policy is not valid.
    pub fn set_backup(&mut self, space: &str, enabled: bool, interval_hours: u32, copies: u32)
        -> Result<(), ServiceError> {
        let policy = BackupPolicy::new(enabled, interval_hours, Retention::Copies(copies))
            .map_err(|e| ServiceError::Policy(format!("{e:?}")))?;
        self.store.load_space(space).ok_or_else(|| ServiceError::NoSpace(space.into()))?;
        self.store.save_backup(space, policy);
        Ok(())
    }
}

