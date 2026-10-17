//! Signed records. A record is the author's device key, the member-list version it was sealed
//! under, and the sealed content, all covered by the author's signature. A reader accepts it
//! only when the author is listed in the reader's current list, the version is not older than
//! that list, and the signature verifies.
//!
//! Layout: author(32) || version(8) || sig(64) || content. The signature covers the first
//! 40 bytes and the content, so changing the version or the content breaks it.

use crate::membership::{Device, Verifier};

const AUTHOR_LEN: usize = 32;
const SIG_LEN: usize = 64;
const HEADER: usize = AUTHOR_LEN + 8 + SIG_LEN;

#[derive(Debug, PartialEq, Eq)]
pub enum RecordError {
    Malformed,
    /// The author is not in the reader's current member list.
    UnknownAuthor,
    /// The record was sealed under a list older than the reader holds.
    StaleList { have: u64, got: u64 },
    BadSignature,
}

/// Build a signed record for `content`, written by `author` under list version `version`.
pub fn seal_record(author: &Device, version: u64, content: &[u8]) -> Result<Vec<u8>, RecordError> {
    let mut out = Vec::with_capacity(HEADER + content.len());
    out.extend_from_slice(&author.public_key());
    out.extend_from_slice(&version.to_be_bytes());
    out.extend_from_slice(&[0u8; SIG_LEN]);
    out.extend_from_slice(content);
    let sig = author.sign(&signed_part(&out));
    out[AUTHOR_LEN + 8..HEADER].copy_from_slice(&sig);
    Ok(out)
}

/// The bytes the signature covers: author, version, and content. The signature field is left out.
fn signed_part(rec: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(rec.len());
    v.extend_from_slice(&rec[..AUTHOR_LEN + 8]);
    v.extend_from_slice(&rec[HEADER..]);
    v
}

/// Check a record against the reader's list and return its content.
pub fn open_record(verifier: &Verifier, rec: &[u8]) -> Result<Vec<u8>, RecordError> {
    if rec.len() < HEADER {
        return Err(RecordError::Malformed);
    }
    let author = &rec[..AUTHOR_LEN];
    let version = u64::from_be_bytes(rec[AUTHOR_LEN..AUTHOR_LEN + 8].try_into().map_err(|_| RecordError::Malformed)?);
    let sig = &rec[AUTHOR_LEN + 8..HEADER];

    let (have, listed) = verifier.current_view().ok_or(RecordError::UnknownAuthor)?;
    if version < have {
        return Err(RecordError::StaleList { have, got: version });
    }
    if !listed.iter().any(|e| e.len() >= 32 && &e[..32] == author) {
        return Err(RecordError::UnknownAuthor);
    }
    if !Device::verify(author, &signed_part(rec), sig) {
        return Err(RecordError::BadSignature);
    }
    Ok(rec[HEADER..].to_vec())
}
