use sesync_team::totp::{base32, otpauth_uri};

#[test]
fn base32_matches_the_rfc4648_vectors() {
    // RFC 4648 test vectors, with the padding removed.
    assert_eq!(base32(b""), "");
    assert_eq!(base32(b"f"), "MY");
    assert_eq!(base32(b"fo"), "MZXQ");
    assert_eq!(base32(b"foo"), "MZXW6");
    assert_eq!(base32(b"foob"), "MZXW6YQ");
    assert_eq!(base32(b"fooba"), "MZXW6YTB");
    assert_eq!(base32(b"foobar"), "MZXW6YTBOI");
}

#[test]
fn the_uri_names_the_space_and_carries_the_secret_once() {
    let uri = otpauth_uri(b"12345678901234567890", "personal");
    assert!(uri.starts_with("otpauth://totp/sesync:personal?secret="), "{uri}");
    assert!(uri.contains("digits=6") && uri.contains("period=30") && uri.contains("algorithm=SHA1"));
}
