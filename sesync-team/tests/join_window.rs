//! A join window. An approved join is valid for at most 10 minutes. After that it expires and the
//! device is not a member, unless the admin's signed list has included it by then.
use sesync_team::membership::{device_entry, Device, Join, JoinError, MemberList, Verifier, JOIN_WINDOW_SECS};

const T0: u64 = 1_000_000;

fn setup() -> (Device, Device, Device) {
    (Device::generate(), Device::generate(), Device::generate())
}

fn base(admin: &Device, member: &Device) -> Verifier {
    let mut v = Verifier::new(admin.public_key());
    v.accept(&MemberList::new(1, vec![device_entry(admin), device_entry(member)]).sign(admin)).unwrap();
    v
}

#[test]
fn the_window_is_ten_minutes() {
    assert_eq!(JOIN_WINDOW_SECS, 10 * 60);
}

#[test]
fn an_approved_join_is_admitted_within_the_window() {
    let (admin, member, newcomer) = setup();
    let mut v = base(&admin, &member);
    let join = Join::request(&newcomer).approve(&member, T0);
    v.admit_at(&join, T0 + 9 * 60).expect("inside the window");
    assert!(v.is_member_at(&newcomer.public_key(), T0 + 9 * 60));
}

#[test]
fn an_approved_join_expires_after_ten_minutes() {
    let (admin, member, newcomer) = setup();
    let mut v = base(&admin, &member);
    let join = Join::request(&newcomer).approve(&member, T0);
    assert_eq!(v.admit_at(&join, T0 + 10 * 60 + 1), Err(JoinError::Expired));
    assert!(!v.is_member_at(&newcomer.public_key(), T0 + 10 * 60 + 1));
}

#[test]
fn a_join_admitted_in_time_expires_unless_the_admin_confirms_it() {
    let (admin, member, newcomer) = setup();
    let mut v = base(&admin, &member);
    let join = Join::request(&newcomer).approve(&member, T0);
    v.admit_at(&join, T0 + 60).unwrap();
    // The window lapses before the admin signs a list that includes the device.
    assert!(!v.is_member_at(&newcomer.public_key(), T0 + 10 * 60 + 1), "unconfirmed, it lapses");
}

#[test]
fn an_admin_signed_list_that_includes_the_device_confirms_it() {
    let (admin, member, newcomer) = setup();
    let mut v = base(&admin, &member);
    let join = Join::request(&newcomer).approve(&member, T0);
    v.admit_at(&join, T0 + 60).unwrap();
    v.accept(&MemberList::new(2, vec![device_entry(&admin), device_entry(&member), device_entry(&newcomer)])
        .sign(&admin)).unwrap();
    assert!(v.is_member_at(&newcomer.public_key(), T0 + 60 * 60), "confirmed by the admin: no longer on a timer");
}

#[test]
fn a_join_is_not_admitted_after_its_window_even_with_a_valid_approval() {
    let (admin, member, newcomer) = setup();
    let mut v = base(&admin, &member);
    let join = Join::request(&newcomer).approve(&member, T0);
    assert_eq!(v.admit_at(&join, T0 + 11 * 60), Err(JoinError::Expired));
}

#[test]
fn a_confirmed_join_does_not_come_back_once_the_admin_drops_the_device() {
    // Confirmed by the admin, then removed by a later list: the old pending join must not revive it.
    let (admin, member, newcomer) = setup();
    let mut v = base(&admin, &member);
    v.admit_at(&Join::request(&newcomer).approve(&member, T0), T0 + 60).unwrap();
    v.accept(&MemberList::new(2, vec![device_entry(&admin), device_entry(&member), device_entry(&newcomer)])
        .sign(&admin)).unwrap();
    v.accept(&MemberList::new(3, vec![device_entry(&admin), device_entry(&member)]).sign(&admin)).unwrap();
    assert!(!v.is_member_at(&newcomer.public_key(), T0 + 60), "dropped by the admin, so not a member");
}
