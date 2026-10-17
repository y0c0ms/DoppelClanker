//! Signed records. A record carries the member-list version it was sealed under and is signed by
//! its author's device key. A reader accepts a record only when the signature verifies with a
//! listed device, and the record's list version is not older than the one the reader holds.
use sesync_team::membership::{Device, MemberList, Verifier};
use sesync_team::signed_records::{open_record, seal_record, RecordError};

fn setup() -> (Device, Device, Device) {
    (Device::generate(), Device::generate(), Device::generate()) // admin, writer, reader
}

fn list_of(v: u64, admin: &Device, devices: &[&Device]) -> sesync_team::membership::SignedList {
    MemberList::new(v, devices.iter().map(|d| sesync_team::membership::device_entry(d)).collect()).sign(admin)
}

#[test]
fn a_record_from_a_listed_writer_opens_for_a_reader() {
    let (admin, writer, reader) = setup();
    let signed = list_of(1, &admin, &[&writer, &reader]);
    let mut v = Verifier::new(admin.public_key());
    v.accept(&signed).unwrap();
    let rec = seal_record(&writer, 1, b"session text").unwrap();
    assert_eq!(open_record(&v, &rec).unwrap(), b"session text");
}

#[test]
fn a_record_from_an_unlisted_writer_is_rejected() {
    let (admin, writer, reader) = setup();
    let outsider = Device::generate();
    let mut v = Verifier::new(admin.public_key());
    v.accept(&list_of(1, &admin, &[&writer, &reader])).unwrap();
    let rec = seal_record(&outsider, 1, b"forged").unwrap();
    assert_eq!(open_record(&v, &rec), Err(RecordError::UnknownAuthor));
}

#[test]
fn a_record_sealed_under_an_older_list_is_rejected() {
    let (admin, writer, reader) = setup();
    let mut v = Verifier::new(admin.public_key());
    v.accept(&list_of(2, &admin, &[&writer, &reader])).unwrap();
    let stale = seal_record(&writer, 1, b"old").unwrap();
    assert_eq!(open_record(&v, &stale), Err(RecordError::StaleList { have: 2, got: 1 }));
}

#[test]
fn a_record_whose_author_was_revoked_is_rejected_after_the_newer_list() {
    let (admin, writer, reader) = setup();
    let mut v = Verifier::new(admin.public_key());
    v.accept(&list_of(1, &admin, &[&writer, &reader])).unwrap();
    let before = seal_record(&writer, 1, b"before revoke").unwrap();
    assert!(open_record(&v, &before).is_ok());
    v.accept(&list_of(2, &admin, &[&reader])).unwrap();
    let after = seal_record(&writer, 2, b"after revoke").unwrap();
    assert_eq!(open_record(&v, &after), Err(RecordError::UnknownAuthor),
        "a revoked writer cannot be accepted under the newer list");
}

#[test]
fn a_tampered_record_fails_its_signature() {
    let (admin, writer, reader) = setup();
    let mut v = Verifier::new(admin.public_key());
    v.accept(&list_of(1, &admin, &[&writer, &reader])).unwrap();
    let mut rec = seal_record(&writer, 1, b"payload").unwrap();
    let last = rec.len() - 1;
    rec[last] ^= 1;
    assert!(matches!(open_record(&v, &rec), Err(RecordError::BadSignature) | Err(RecordError::Malformed)));
}
