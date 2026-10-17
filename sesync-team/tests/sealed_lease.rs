//! The sealed write is lease-checked, like the plaintext write it replaces: a record is stored
//! only from the machine holding the conversation's lease, under its current version.
use sesync_team::membership::{device_entry, Device, MemberList, Verifier};
use sesync_team::service::{MemoryStore, Service, ServiceError};
use sesync_team::session_keys::{seal_session, DataKey};
use sesync_team::signed_records::seal_record;

const T0: u64 = 1_000_020;

fn setup() -> (Service<MemoryStore>, Device, Device) {
    let admin = Device::generate();
    let writer = Device::generate();
    let mut svc = Service::new(MemoryStore::default());
    svc.create_space("personal", &["writer"]).unwrap();
    svc.pin_admin_key("personal", &admin.public_key()).unwrap();
    svc.store_trust_list("personal", &MemberList::new(1, vec![device_entry(&admin), device_entry(&writer)]).sign(&admin).to_bytes()).unwrap();
    (svc, admin, writer)
}

fn record_for(writer: &Device) -> Vec<u8> {
    let key = DataKey::random();
    let body = seal_session(&key, b"x").unwrap();
    seal_record(writer, 1, &body).unwrap()
}

#[test]
fn the_lease_holder_can_store_a_sealed_record() {
    let (mut svc, _admin, writer) = setup();
    let lease = svc.acquire("personal", "ses_l", "box-w", T0).unwrap();
    assert!(svc.put_sealed_leased("personal", "ses_l", "box-w", lease.version, record_for(&writer)).is_ok());
}

#[test]
fn a_machine_without_the_lease_cannot_store_a_sealed_record() {
    let (mut svc, _admin, writer) = setup();
    let _lease = svc.acquire("personal", "ses_l", "box-w", T0).unwrap();
    let r = svc.put_sealed_leased("personal", "ses_l", "box-other", 1, record_for(&writer));
    assert!(matches!(r, Err(ServiceError::Lease(_))), "another machine is refused: {r:?}");
}

#[test]
fn a_stale_lease_version_is_refused() {
    let (mut svc, _admin, writer) = setup();
    let lease = svc.acquire("personal", "ses_l", "box-w", T0).unwrap();
    let r = svc.put_sealed_leased("personal", "ses_l", "box-w", lease.version + 5, record_for(&writer));
    assert!(matches!(r, Err(ServiceError::Lease(_))), "a version the lease did not grant is refused");
}

#[test]
fn an_unlisted_author_is_refused_even_with_the_lease() {
    let (mut svc, _admin, _writer) = setup();
    let outsider = Device::generate();
    let lease = svc.acquire("personal", "ses_l", "box-w", T0).unwrap();
    let r = svc.put_sealed_leased("personal", "ses_l", "box-w", lease.version, record_for(&outsider));
    assert!(r.is_err(), "the lease does not make an unlisted author acceptable");
    let _ = Verifier::new(Device::generate().public_key());
}
