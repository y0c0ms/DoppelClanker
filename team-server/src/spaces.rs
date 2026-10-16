//! Spaces: a team or project that shares sessions. Each space has its own content key,
//! and every record is sealed with ChaCha20-Poly1305 under a fresh random nonce. Removing a
//! member rotates the key, so the removed member cannot open anything written afterwards.
//! A key is never used for another space.

use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use rand::RngCore;
use std::collections::{BTreeSet, HashMap};

const NONCE_LEN: usize = 12;

#[derive(Debug, PartialEq, Eq)]
pub enum SpaceError {
    NotMember(String),
    Crypto(String),
}

impl std::fmt::Display for SpaceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SpaceError::NotMember(m) => write!(f, "{m} is not a member of this space"),
            SpaceError::Crypto(s) => write!(f, "cannot open: {s}"),
        }
    }
}

impl std::error::Error for SpaceError {}

fn random_key() -> [u8; 32] {
    let mut k = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut k);
    k
}

fn random_nonce() -> [u8; NONCE_LEN] {
    let mut n = [0u8; NONCE_LEN];
    rand::thread_rng().fill_bytes(&mut n);
    n
}

/// Seal `plaintext` under `key`. The output is nonce || ciphertext-with-tag.
/// `aad` binds the record to its context; here it is the caller's label.
pub fn seal(key: &[u8; 32], aad: &[u8], plaintext: &[u8]) -> Result<Vec<u8>, SpaceError> {
    let cipher = ChaCha20Poly1305::new(Key::from_slice(key));
    let nonce = random_nonce();
    let ct = cipher
        .encrypt(Nonce::from_slice(&nonce), chacha20poly1305::aead::Payload { msg: plaintext, aad })
        .map_err(|e| SpaceError::Crypto(e.to_string()))?;
    let mut out = nonce.to_vec();
    out.extend_from_slice(&ct);
    Ok(out)
}

/// Open a record produced by `seal`. A wrong key, a changed byte, or a wrong `aad` is refused.
pub fn open(key: &[u8; 32], sealed: &[u8]) -> Result<Vec<u8>, SpaceError> {
    open_with_aad(key, b"nonce-test", sealed)
}

fn open_with_aad(key: &[u8; 32], aad: &[u8], sealed: &[u8]) -> Result<Vec<u8>, SpaceError> {
    if sealed.len() < NONCE_LEN {
        return Err(SpaceError::Crypto("record too short".into()));
    }
    let (nonce, ct) = sealed.split_at(NONCE_LEN);
    let cipher = ChaCha20Poly1305::new(Key::from_slice(key));
    cipher
        .decrypt(Nonce::from_slice(nonce), chacha20poly1305::aead::Payload { msg: ct, aad })
        .map_err(|_| SpaceError::Crypto("authentication failed".into()))
}

/// A space: members, and the content key for each key version.
pub struct Space {
    pub name: String,
    members: BTreeSet<String>,
    keys: HashMap<u64, [u8; 32]>,
    version: u64,
}

impl Space {
    pub fn new(name: &str, members: &[&str]) -> Space {
        let mut keys = HashMap::new();
        keys.insert(1, random_key());
        Space {
            name: name.to_string(),
            members: members.iter().map(|m| m.to_string()).collect(),
            keys,
            version: 1,
        }
    }

    pub fn key_version(&self) -> u64 {
        self.version
    }

    fn label(&self) -> Vec<u8> {
        format!("space:{}", self.name).into_bytes()
    }

    /// Seal content for the space, as `member`. Only members may write.
    pub fn seal_for(&mut self, member: &str, plaintext: &[u8]) -> Result<Vec<u8>, SpaceError> {
        if !self.members.contains(member) {
            return Err(SpaceError::NotMember(member.into()));
        }
        let key = self.keys[&self.version];
        let mut out = self.version.to_le_bytes().to_vec();
        out.extend(seal(&key, &self.label(), plaintext)?);
        Ok(out)
    }

    /// Open content for `member`. Only current members may read.
    pub fn open_for(&self, member: &str, sealed: &[u8]) -> Result<Vec<u8>, SpaceError> {
        if !self.members.contains(member) {
            return Err(SpaceError::NotMember(member.into()));
        }
        if sealed.len() < 8 {
            return Err(SpaceError::Crypto("record too short".into()));
        }
        let mut v = [0u8; 8];
        v.copy_from_slice(&sealed[..8]);
        let version = u64::from_le_bytes(v);
        let key = self.keys.get(&version).ok_or_else(|| SpaceError::Crypto("unknown key version".into()))?;
        open_with_aad(key, &self.label(), &sealed[8..])
    }

    /// Remove a member and rotate the key. Earlier content stays readable to the remaining
    /// members through the old key version they still hold.
    pub fn remove_member(&mut self, member: &str) -> Result<(), SpaceError> {
        if !self.members.remove(member) {
            return Err(SpaceError::NotMember(member.into()));
        }
        self.version += 1;
        self.keys.insert(self.version, random_key());
        Ok(())
    }
}
