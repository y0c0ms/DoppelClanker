//! The client's seal and open, the code the sync loop calls. A session is sealed for the devices
//! in the member list, signed by the writer, and opened by a listed reader. The list's version
//! and the author are checked, and a session that does not verify is never returned as text.
use sesync::device_keys::load_or_create;
use sesync::sealed::{open_for_device, seal_for_devices, SealError};

fn dir(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("sesync-sealed-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

#[test]
fn a_session_seals_and_a_listed_reader_opens_the_exact_text() {
    let w = dir("w"); let r = dir("r");
    let writer = load_or_create(&w).unwrap();
    let reader = load_or_create(&r).unwrap();
    let rec = seal_for_devices(&writer, &[&writer, &reader], b"the session text").unwrap();
    assert_eq!(open_for_device(&reader, &[writer.public_signing().to_vec()], &rec).unwrap(), b"the session text");
    let _ = std::fs::remove_dir_all(&w); let _ = std::fs::remove_dir_all(&r);
}

#[test]
fn a_session_from_an_unlisted_writer_is_not_opened() {
    let w = dir("uw"); let r = dir("ur");
    let outsider = load_or_create(&w).unwrap();
    let reader = load_or_create(&r).unwrap();
    let rec = seal_for_devices(&outsider, &[&reader], b"x").unwrap();
    let r2 = open_for_device(&reader, &[], &rec);
    assert!(matches!(r2, Err(SealError::UnknownAuthor)), "{r2:?}");
    let _ = std::fs::remove_dir_all(&w); let _ = std::fs::remove_dir_all(&r);
}

#[test]
fn a_device_not_in_the_session_wraps_cannot_open_it() {
    let w = dir("nw"); let r = dir("nr"); let late = dir("nl");
    let writer = load_or_create(&w).unwrap();
    let reader = load_or_create(&r).unwrap();
    let outsider = load_or_create(&late).unwrap();
    let rec = seal_for_devices(&writer, &[&reader], b"private").unwrap();
    assert!(open_for_device(&outsider, &[writer.public_signing().to_vec()], &rec).is_err());
    for d in [&w, &r, &late] { let _ = std::fs::remove_dir_all(d); }
}
