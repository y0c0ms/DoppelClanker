//! The client's seal and open for team records. It uses the same record format and crypto as the
//! team server (one implementation, shared by path), so a record made here verifies there and back.
//!
//! Seal: the session is encrypted under a fresh data key; that key is wrapped to each listed
//! device; the result is signed by the writer. Open: the record must be signed by a listed author,
//! and the wrap addressed to this device must open. Nothing is returned as text unless both hold.

use sesync_team::session_keys::{open_session, seal_session, unwrap_for, wrap_for, DataKey, Wrap};
use sesync_team::membership::Device;
use sesync_team::signed_records::{open_record, seal_record, RecordError};
use sesync_team::membership::{MemberList, Verifier};

use crate::device_keys::DeviceFile;

#[derive(Debug)]
pub enum SealError {
    UnknownAuthor,
    BadSignature,
    NotForThisDevice,
    Malformed,
    Crypto(String),
}

/// The team's device view for the device's own use: the same type the server uses.
fn as_device(d: &DeviceFile) -> Result<Device, SealError> {
    Device::from_keys(d.signing_pkcs8(), *d.box_secret()).map_err(|_| SealError::Malformed)
}

/// Seal `plain` for the devices in `recipients` and sign it as `writer`.
pub fn seal_for_devices(writer: &DeviceFile, recipients: &[&DeviceFile], plain: &[u8]) -> Result<Vec<u8>, SealError> {
    let key = DataKey::random();
    let body = seal_session(&key, plain).map_err(|e| SealError::Crypto(format!("{e:?}")))?;
    let boxes: Vec<Vec<u8>> = recipients.iter().map(|r| r.public_box().to_vec()).collect();
    let wraps = wrap_for(&key, &boxes).map_err(|e| SealError::Crypto(format!("{e:?}")))?;
    let payload = encode_payload(&wraps, &body);
    let author = as_device(writer)?;
    seal_record(&author, 1, &payload).map_err(|e| SealError::Crypto(format!("{e:?}")))
}

/// Open a record for `reader`. `listed_authors` are the signing keys in the reader's current
/// list; a record from anyone else is refused.
pub fn open_for_device(reader: &DeviceFile, listed_authors: &[Vec<u8>], record: &[u8]) -> Result<Vec<u8>, SealError> {
    // Build a one-version trust view: the authors the reader currently lists.
    let admin = Device::from_keys(reader.signing_pkcs8(), *reader.box_secret()).map_err(|_| SealError::Malformed)?;
    let entries: Vec<Vec<u8>> = listed_authors.iter().map(|k| k.clone()).collect();
    let mut verifier = Verifier::new(admin.public_key());
    let signed = MemberList::new(1, entries).sign(&admin);
    verifier.accept(&signed).map_err(|_| SealError::Malformed)?;
    let payload = open_record(&verifier, record).map_err(|e| match e {
        RecordError::UnknownAuthor => SealError::UnknownAuthor,
        RecordError::BadSignature => SealError::BadSignature,
        RecordError::StaleList { .. } => SealError::UnknownAuthor,
        RecordError::Malformed => SealError::Malformed,
    })?;
    let (wraps, body) = decode_payload(&payload).ok_or(SealError::Malformed)?;
    let me = as_device(reader)?;
    let key = unwrap_for(&me, &wraps).map_err(|_| SealError::NotForThisDevice)?;
    open_session(&key, &body).map_err(|e| SealError::Crypto(format!("{e:?}")))
}

fn encode_payload(wraps: &[Wrap], body: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(wraps.len() as u32).to_be_bytes());
    for w in wraps {
        out.extend_from_slice(&(w.device.len() as u32).to_be_bytes());
        out.extend_from_slice(&w.device);
        out.extend_from_slice(&(w.sealed.len() as u32).to_be_bytes());
        out.extend_from_slice(&w.sealed);
    }
    out.extend_from_slice(body);
    out
}

fn decode_payload(b: &[u8]) -> Option<(Vec<Wrap>, Vec<u8>)> {
    let mut i = 0usize;
    let rd = |i: &mut usize| -> Option<usize> {
        let n = u32::from_be_bytes(b.get(*i..*i + 4)?.try_into().ok()?) as usize;
        *i += 4;
        Some(n)
    };
    let count = rd(&mut i)?;
    let mut wraps = Vec::with_capacity(count.min(4096));
    for _ in 0..count {
        let dl = rd(&mut i)?;
        let device = b.get(i..i + dl)?.to_vec(); i += dl;
        let sl = rd(&mut i)?;
        let sealed = b.get(i..i + sl)?.to_vec(); i += sl;
        wraps.push(Wrap { device, sealed });
    }
    Some((wraps, b.get(i..)?.to_vec()))
}
