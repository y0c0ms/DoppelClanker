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

/// Open a record sealed under `key` with the given associated data. Used by key wrapping.
pub fn open_bound(key: &[u8; 32], aad: &[u8], sealed: &[u8]) -> Result<Vec<u8>, SpaceError> {
    open_with_aad(key, aad, sealed)
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
    secret: Option<Vec<u8>>,
    token_key: Option<[u8; 32]>,
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
            secret: None,
            token_key: None,
        }
    }

    /// The authenticator secret for this space, if one was set.
    pub fn secret(&self) -> Option<&[u8]> {
        self.secret.as_deref()
    }

    pub fn set_secret(&mut self, secret: Vec<u8>) {
        self.secret = Some(secret);
    }

    /// The HMAC key that signs this space's caller tokens.
    pub fn token_key(&self) -> Option<[u8; 32]> {
        self.token_key
    }

    pub fn set_token_key(&mut self, key: [u8; 32]) {
        self.token_key = Some(key);
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

    /// The raw content key for a version. For tests that check the key never reaches a database.
    pub fn key_bytes(&self, version: u64) -> Option<[u8; 32]> {
        self.keys.get(&version).copied()
    }

    /// Serialize the space: name, version, members, and every key version. Used only for
    /// wrapping under the master key; the output must never be stored unwrapped.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        push_str(&mut out, &self.name);
        match &self.secret {
            Some(sec) => {
                out.push(1);
                out.extend_from_slice(&(sec.len() as u32).to_le_bytes());
                out.extend_from_slice(sec);
            }
            None => out.push(0),
        }
        match &self.token_key {
            Some(k) => { out.push(1); out.extend_from_slice(k); }
            None => out.push(0),
        }
        out.extend_from_slice(&self.version.to_le_bytes());
        out.extend_from_slice(&(self.members.len() as u32).to_le_bytes());
        for m in &self.members {
            push_str(&mut out, m);
        }
        let mut versions: Vec<_> = self.keys.iter().collect();
        versions.sort_by_key(|(v, _)| **v);
        out.extend_from_slice(&(versions.len() as u32).to_le_bytes());
        for (v, k) in versions {
            out.extend_from_slice(&v.to_le_bytes());
            out.extend_from_slice(k);
        }
        out
    }

    /// Parse a serialized space. None if the bytes are not a complete space.
    pub fn from_bytes(b: &[u8]) -> Option<Space> {
        let mut c = Cursor { b, i: 0 };
        let name = c.string()?;
        let secret = match c.take(1)?[0] {
            0 => None,
            1 => {
                let n = c.u32()? as usize;
                Some(c.take(n)?.to_vec())
            }
            _ => return None,
        };
        let token_key = match c.take(1)?[0] {
            0 => None,
            1 => {
                let mut k = [0u8; 32];
                k.copy_from_slice(c.take(32)?);
                Some(k)
            }
            _ => return None,
        };
        let version = c.u64()?;
        let n_members = c.u32()? as usize;
        let mut members = BTreeSet::new();
        for _ in 0..n_members {
            members.insert(c.string()?);
        }
        let n_keys = c.u32()? as usize;
        let mut keys = HashMap::new();
        for _ in 0..n_keys {
            let v = c.u64()?;
            let k = c.take(32)?;
            let mut key = [0u8; 32];
            key.copy_from_slice(k);
            keys.insert(v, key);
        }
        if c.i != b.len() { return None; }
        Some(Space { name, members, keys, version, secret, token_key })
    }

    /// Confirm `member` belongs to the space, without sealing or opening anything.
    pub fn seal_for_member_check(&self, member: &str) -> Result<(), SpaceError> {
        if self.members.contains(member) { Ok(()) } else { Err(SpaceError::NotMember(member.into())) }
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

fn push_str(out: &mut Vec<u8>, s: &str) {
    out.extend_from_slice(&(s.len() as u32).to_le_bytes());
    out.extend_from_slice(s.as_bytes());
}

struct Cursor<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.i.checked_add(n)?;
        let s = self.b.get(self.i..end)?;
        self.i = end;
        Some(s)
    }
    fn u32(&mut self) -> Option<u32> {
        let mut a = [0u8; 4];
        a.copy_from_slice(self.take(4)?);
        Some(u32::from_le_bytes(a))
    }
    fn u64(&mut self) -> Option<u64> {
        let mut a = [0u8; 8];
        a.copy_from_slice(self.take(8)?);
        Some(u64::from_le_bytes(a))
    }
    fn string(&mut self) -> Option<String> {
        let n = self.u32()? as usize;
        String::from_utf8(self.take(n)?.to_vec()).ok()
    }
}
