//! Device keys and the admin-signed member list.
//!
//! Each device has an Ed25519 key pair; the private key never leaves the device. The member list
//! names the devices allowed to read and write, is signed by the admin key, and carries a version.
//! A verifier accepts a list only if the admin signed it and its version is not older than the one
//! it already holds. A device signs its own records, so a reader knows who wrote them.

use ring::rand::SystemRandom;
use ring::signature::{Ed25519KeyPair, KeyPair, UnparsedPublicKey, ED25519};

/// A device's signing key. Created on the device and kept there.
pub struct Device {
    pair: Ed25519KeyPair,
    pkcs8: Vec<u8>,
}

impl Device {
    pub fn generate() -> Device {
        let rng = SystemRandom::new();
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&rng).expect("system randomness is available");
        let pair = Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).expect("a fresh key parses");
        Device { pair, pkcs8: pkcs8.as_ref().to_vec() }
    }

    /// Rebuild a device from its stored private key (PKCS#8).
    pub fn from_pkcs8(pkcs8: &[u8]) -> Result<Device, MembershipError> {
        let pair = Ed25519KeyPair::from_pkcs8(pkcs8).map_err(|_| MembershipError::BadKey)?;
        Ok(Device { pair, pkcs8: pkcs8.to_vec() })
    }

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
    pub devices: Vec<Vec<u8>>,
}

impl MemberList {
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
}

impl Verifier {
    pub fn new(admin_public: Vec<u8>) -> Verifier {
        Verifier { admin: admin_public, current: None }
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
        self.current = Some(signed.list.clone());
        Ok(())
    }

    pub fn is_member(&self, device_public: &[u8]) -> bool {
        self.current.as_ref().map_or(false, |l| l.devices.iter().any(|d| d == device_public))
    }
}
