//! Device keys and the admin-signed member list.
//!
//! Each device has an Ed25519 key pair; the private key never leaves the device. The member list
//! names the devices allowed to read and write, is signed by the admin key, and carries a version.
//! A verifier accepts a list only if the admin signed it and its version is not older than the one
//! it already holds. A device signs its own records, so a reader knows who wrote them.

use ring::rand::SystemRandom;
use ring::signature::{Ed25519KeyPair, KeyPair, UnparsedPublicKey, ED25519};

/// A device's keys: an Ed25519 signing key (for records and lists) and an X25519 box key (for
/// receiving wrapped session keys). Both private keys stay on the device.
pub struct Device {
    pair: Ed25519KeyPair,
    pkcs8: Vec<u8>,
    box_secret: crypto_box::SecretKey,
}

impl Device {
    pub fn generate() -> Device {
        let rng = SystemRandom::new();
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&rng).expect("system randomness is available");
        let pair = Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).expect("a fresh key parses");
        let box_secret = crypto_box::SecretKey::generate(&mut crypto_box::aead::OsRng);
        Device { pair, pkcs8: pkcs8.as_ref().to_vec(), box_secret }
    }

    /// The X25519 public key others wrap session keys to.
    pub fn box_public(&self) -> Vec<u8> { self.box_secret.public_key().as_bytes().to_vec() }

    pub(crate) fn box_secret(&self) -> &crypto_box::SecretKey { &self.box_secret }

    /// The X25519 public key others wrap session keys to.

    /// The device's agreement private key, taken once to agree a wrapped key.
    pub(crate) fn agreement_private(&self) -> ring::agreement::EphemeralPrivateKey {
        // Re-derive from the stored seed is not possible for an ephemeral key, so the device keeps
        // its agreement private key in a form that can be used for each unwrap.
        unimplemented!("see agreement_key")
    }

    /// Rebuild a device from its stored signing key and box key. Both are stored on the device.
    pub fn from_keys(pkcs8: &[u8], box_secret: [u8; 32]) -> Result<Device, MembershipError> {
        let pair = Ed25519KeyPair::from_pkcs8(pkcs8).map_err(|_| MembershipError::BadKey)?;
        Ok(Device { pair, pkcs8: pkcs8.to_vec(), box_secret: crypto_box::SecretKey::from(box_secret) })
    }

    pub fn box_secret_bytes(&self) -> [u8; 32] { self.box_secret.to_bytes() }

    pub fn private_pkcs8(&self) -> &[u8] { &self.pkcs8 }

    pub fn public_key(&self) -> Vec<u8> { self.pair.public_key().as_ref().to_vec() }

    pub fn sign(&self, msg: &[u8]) -> Vec<u8> { self.pair.sign(msg).as_ref().to_vec() }

    pub fn verify(public: &[u8], msg: &[u8], sig: &[u8]) -> bool {
        UnparsedPublicKey::new(&ED25519, public).verify(msg, sig).is_ok()
    }
}

/// The member list: a version and the public keys of the devices allowed in the space.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemberList {
    pub version: u64,
    /// Each entry is a device's signing public key followed by its box public key, both
    /// covered by the admin's signature, so the list is the one source of who may read.
    pub devices: Vec<Vec<u8>>,
}

/// One device entry: signing key and box key, concatenated. Both are 32 bytes.
pub fn device_entry(d: &Device) -> Vec<u8> {
    let mut e = d.public_key();
    e.extend_from_slice(&d.box_public());
    e
}

impl MemberList {
    /// Build a list from device entries (see `device_entry`).
    pub fn new(version: u64, devices: Vec<Vec<u8>>) -> MemberList {
        let mut devices = devices;
        devices.sort();
        devices.dedup();
        MemberList { version, devices }
    }

    /// The exact bytes the admin signs: version, then each key length-prefixed in sorted order.
    /// Fixed encoding, so the signature covers precisely these bytes.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&self.version.to_be_bytes());
        out.extend_from_slice(&(self.devices.len() as u32).to_be_bytes());
        for d in &self.devices {
            out.extend_from_slice(&(d.len() as u32).to_be_bytes());
            out.extend_from_slice(d);
        }
        out
    }

    pub fn sign(self, admin: &Device) -> SignedList {
        let sig = admin.sign(&self.encode());
        SignedList { list: self, signature: sig }
    }
}

/// A member list with the admin's signature over its encoding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedList {
    pub list: MemberList,
    pub signature: Vec<u8>,
}

impl SignedList {
    /// Length-prefixed: the encoded list, then the signature.
    pub fn to_bytes(&self) -> Vec<u8> {
        let body = self.list.encode();
        let mut out = Vec::new();
        out.extend_from_slice(&(body.len() as u32).to_be_bytes());
        out.extend_from_slice(&body);
        out.extend_from_slice(&(self.signature.len() as u32).to_be_bytes());
        out.extend_from_slice(&self.signature);
        out
    }

    pub fn from_bytes(b: &[u8]) -> Result<SignedList, MembershipError> {
        let mut i = 0usize;
        let take = |i: &mut usize, n: usize| -> Result<Vec<u8>, MembershipError> {
            let end = i.checked_add(n).ok_or(MembershipError::Malformed)?;
            let s = b.get(*i..end).ok_or(MembershipError::Malformed)?.to_vec();
            *i = end;
            Ok(s)
        };
        let u32_at = |i: &mut usize| -> Result<u32, MembershipError> {
            let v = take(i, 4)?;
            Ok(u32::from_be_bytes([v[0], v[1], v[2], v[3]]))
        };
        let body_len = u32_at(&mut i)? as usize;
        let body = take(&mut i, body_len)?;
        let sig_len = u32_at(&mut i)? as usize;
        let signature = take(&mut i, sig_len)?;
        if i != b.len() { return Err(MembershipError::Malformed); }
        let list = decode(&body)?;
        Ok(SignedList { list, signature })
    }
}

fn decode(b: &[u8]) -> Result<MemberList, MembershipError> {
    let mut i = 0usize;
    let take = |i: &mut usize, n: usize| -> Result<&[u8], MembershipError> {
        let end = i.checked_add(n).ok_or(MembershipError::Malformed)?;
        let s = b.get(*i..end).ok_or(MembershipError::Malformed)?;
        *i = end;
        Ok(s)
    };
    let version = u64::from_be_bytes(take(&mut i, 8)?.try_into().map_err(|_| MembershipError::Malformed)?);
    let count = u32::from_be_bytes(take(&mut i, 4)?.try_into().map_err(|_| MembershipError::Malformed)?) as usize;
    let mut devices = Vec::with_capacity(count.min(1024));
    for _ in 0..count {
        let len = u32::from_be_bytes(take(&mut i, 4)?.try_into().map_err(|_| MembershipError::Malformed)?) as usize;
        devices.push(take(&mut i, len)?.to_vec());
    }
    if i != b.len() { return Err(MembershipError::Malformed); }
    Ok(MemberList { version, devices })
}

#[derive(Debug, PartialEq, Eq)]
pub enum MembershipError {
    BadSignature,
    /// The list is older than one already held, so it is ignored.
    Stale { have: u64, got: u64 },
    Malformed,
    BadKey,
}

/// Holds the admin's public key and the newest list accepted. Only lists the admin signed, and
/// never older than the held version, are taken in.
pub struct Verifier {
    admin: Vec<u8>,
    current: Option<MemberList>,
    pending: Vec<Pending>,
}

impl Verifier {
    pub fn new(admin_public: Vec<u8>) -> Verifier {
        Verifier { admin: admin_public, current: None, pending: Vec::new() }
    }

    pub fn accept(&mut self, signed: &SignedList) -> Result<(), MembershipError> {
        if !Device::verify(&self.admin, &signed.list.encode(), &signed.signature) {
            return Err(MembershipError::BadSignature);
        }
        if let Some(cur) = &self.current {
            if signed.list.version < cur.version {
                return Err(MembershipError::Stale { have: cur.version, got: signed.list.version });
            }
        }
        // A device the admin now lists is confirmed: its join no longer runs on a timer.
        let listed: Vec<Vec<u8>> = signed.list.devices.clone();
        self.pending.retain(|p| !listed.iter().any(|e| e == &p.entry));
        self.current = Some(signed.list.clone());
        Ok(())
    }

    /// The version and member keys of the list currently held, if any.
    pub fn current_view(&self) -> Option<(u64, Vec<Vec<u8>>)> {
        self.current.as_ref().map(|l| (l.version, l.devices.clone()))
    }

    /// The box public keys of the devices in the current list: who a new session is wrapped to.
    pub fn box_keys(&self) -> Vec<Vec<u8>> {
        self.current.as_ref().map_or(Vec::new(), |l| {
            l.devices.iter().filter(|e| e.len() == 64).map(|e| e[32..].to_vec()).collect()
        })
    }

    /// Whether a device is listed, matched by its signing key (the first half of its entry).
    pub fn is_member(&self, device_public: &[u8]) -> bool {
        self.current.as_ref().map_or(false, |l| {
            l.devices.iter().any(|e| e.len() >= 32 && &e[..32] == device_public)
        })
    }
}

/// How long an approved join stands before it expires, unless the admin's signed list includes
/// the device first. Ten minutes: a join is a short hand-over, not a standing grant.
pub const JOIN_WINDOW_SECS: u64 = 10 * 60;

/// A new device asks to join: its entry (signing and box keys), and, once a member approves, the
/// approver's signature over the entry and the approval time.
#[derive(Clone, Debug)]
pub struct Join {
    pub request: Vec<u8>,
    pub approver: Option<Vec<u8>>,
    pub approval: Option<Vec<u8>>,
    /// When the approval was made. The join is valid for JOIN_WINDOW_SECS from here.
    pub approved_at: Option<u64>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum JoinError {
    /// No member has approved the request.
    NotApproved,
    /// The approver is not in the current member list.
    ApproverNotMember,
    /// The approval does not verify over this request and time.
    BadApproval,
    /// The join window has passed.
    Expired,
}

impl Join {
    /// A device asks to join. The request is its own entry, not signed yet.
    pub fn request(device: &Device) -> Join {
        Join { request: device_entry(device), approver: None, approval: None, approved_at: None }
    }

    /// A current member approves this exact request at `now`. The approval signs the request and
    /// the time, so neither can be changed.
    pub fn approve(mut self, member: &Device, now: u64) -> Join {
        let approval = member.sign(&join_message(&self.request, now));
        self.approver = Some(member.public_key());
        self.approval = Some(approval);
        self.approved_at = Some(now);
        self
    }
}

/// What a member signs when it approves: a fixed label, the request entry and the approval time.
fn join_message(request: &[u8], at: u64) -> Vec<u8> {
    let mut m = b"sesync join v1".to_vec();
    m.extend_from_slice(&at.to_be_bytes());
    m.extend_from_slice(request);
    m
}

impl Verifier {
    /// Admit a device if a current member approved its request within the join window at `now`.
    /// A join never removes anyone. An admitted join lapses after JOIN_WINDOW_SECS unless the
    /// admin's signed list includes the device, which confirms it.
    pub fn admit_at(&mut self, join: &Join, now: u64) -> Result<(), JoinError> {
        let (approver, approval, at) = match (&join.approver, &join.approval, join.approved_at) {
            (Some(a), Some(s), Some(t)) => (a, s, t),
            _ => return Err(JoinError::NotApproved),
        };
        if now >= at + JOIN_WINDOW_SECS {
            return Err(JoinError::Expired);
        }
        if !self.is_member(approver) {
            return Err(JoinError::ApproverNotMember);
        }
        if !Device::verify(approver, &join_message(&join.request, at), approval) {
            return Err(JoinError::BadApproval);
        }
        self.pending.push(Pending { entry: join.request.clone(), expires_at: at + JOIN_WINDOW_SECS });
        Ok(())
    }

    /// Admit with no clock: kept for callers that check the window themselves.
    pub fn admit(&mut self, join: &Join) -> Result<(), JoinError> {
        let at = join.approved_at.ok_or(JoinError::NotApproved)?;
        self.admit_at(join, at)
    }

    /// Whether a device is a member at `now`: listed by the admin, or admitted by a join still in
    /// its window. A join confirmed by the admin's list stops being time-limited.
    pub fn is_member_at(&self, device_public: &[u8], now: u64) -> bool {
        if self.is_member(device_public) {
            return true;
        }
        self.pending.iter().any(|p| {
            p.entry.len() >= 32 && &p.entry[..32] == device_public && now < p.expires_at
        })
    }
}

/// A join admitted by a member, waiting for the admin to confirm it in a signed list.
#[derive(Clone, Debug)]
struct Pending {
    entry: Vec<u8>,
    expires_at: u64,
}
