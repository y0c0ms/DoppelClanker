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
    fn load_gate(&self, space: &str, machine: &str) -> Option<JoinGate>;
    fn save_gate(&mut self, space: &str, machine: &str, gate: JoinGate);
    fn load_backup(&self, space: &str) -> Option<BackupPolicy>;
    fn save_backup(&mut self, space: &str, policy: BackupPolicy);
}

#[derive(Default)]
pub struct MemoryStore {
    spaces: HashMap<String, Space>,
    records: HashMap<(String, String), Vec<u8>>,
    gates: HashMap<(String, String), JoinGate>,
    backups: HashMap<String, BackupPolicy>,
}

impl Store for MemoryStore {
    fn load_space(&self, name: &str) -> Option<Space> { self.spaces.get(name).and_then(|s| Space::from_bytes(&s.to_bytes())) }
    fn save_space(&mut self, space: Space) { self.spaces.insert(space.name.clone(), space); }
    fn record(&self, space: &str, conv: &str) -> Option<Vec<u8>> {
        self.records.get(&(space.to_string(), conv.to_string())).cloned()
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
    fn save_backup(&mut self, space: &str, policy: BackupPolicy) {
        self.backups.insert(space.to_string(), policy);
    }
}

pub struct Service<S: Store> {
    store: S,
    leases: LeaseTable,
    /// The shared authenticator secret per space, used to check join codes.
    secrets: HashMap<String, Vec<u8>>,
    /// Which member each machine joined as, per space.
    owners: HashMap<(String, String), String>,
}

impl<S: Store> Service<S> {
    pub fn new(store: S) -> Self {
        Service { store, leases: LeaseTable::default(), secrets: HashMap::new(), owners: HashMap::new() }
    }

    pub fn create_space(&mut self, name: &str, members: &[&str]) -> Result<(), ServiceError> {
        self.store.save_space(Space::new(name, members));
        self.secrets.insert(name.to_string(), b"12345678901234567890".to_vec());
        Ok(())
    }

    fn member(&self, space: &str, who: &str) -> Result<(), ServiceError> {
        let s = self.store.load_space(space).ok_or_else(|| ServiceError::NoSpace(space.into()))?;
        s.seal_for_member_check(who).map_err(|_| ServiceError::NotMember(who.into()))
    }

    /// A machine joins a space with the authenticator code for `now`.
    pub fn join(&mut self, space: &str, who: &str, machine: &str, code: &str, now: u64) -> Result<(), ServiceError> {
        self.member(space, who)?;
        self.owners.insert((space.to_string(), machine.to_string()), who.to_string());
        let secret = self.secrets.get(space).cloned().ok_or_else(|| ServiceError::NoSpace(space.into()))?;
        let mut gate = self.store.load_gate(space, machine).unwrap_or_else(|| JoinGate::new(secret.clone()));
        let r = gate.verify(code, now);
        self.store.save_gate(space, machine, gate);
        r.map_err(ServiceError::from)
    }

    pub fn acquire(&mut self, space: &str, conv: &str, machine: &str, now: u64) -> Result<Lease, ServiceError> {
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
        let owner = self.owners.get(&(space.to_string(), machine.to_string()))
            .cloned()
            .ok_or_else(|| ServiceError::NotMember(machine.into()))?;
        let mut s = self.store.load_space(space).ok_or_else(|| ServiceError::NoSpace(space.into()))?;
        let sealed = s.seal_for(&owner, &plaintext)?;
        self.store.save_space(s);
        self.store.put_record(space, conv, sealed);
        Ok(())
    }

    pub fn get_record(&self, space: &str, conv: &str, who: &str) -> Result<Vec<u8>, ServiceError> {
        self.member(space, who)?;
        let sealed = self.store.record(space, conv).ok_or_else(|| ServiceError::NoSpace(conv.into()))?;
        let s = self.store.load_space(space).ok_or_else(|| ServiceError::NoSpace(space.into()))?;
        Ok(s.open_for(who, &sealed)?)
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

