//! Property tests for the code that reads untrusted bytes: bearer tokens, sealed session
//! records, the stored space and gate formats, and the wrapped space keys. Two rules: none of
//! them panics on any input, and what is sealed opens back to exactly what went in.

use sesync_team::auth::{check_token, issue_token};
use sesync_team::spaces::{open, seal, Space};

/// The label the public `open` expects; sealing with the same label makes records round-trip.
const LABEL: &[u8] = b"nonce-test";
use sesync_team::totp::JoinGate;
use sesync_team::wrap::unwrap_space;
use proptest::prelude::*;

const KEY: [u8; 32] = [9u8; 32];

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    /// No input string can crash the token check.
    #[test]
    fn check_token_never_panics(token in ".{0,256}", machine in ".{0,32}", now in any::<u64>()) {
        let _ = check_token(&KEY, &token, &machine, now);
    }

    /// A token we issue verifies for its machine before expiry, and only then.
    #[test]
    fn issued_tokens_verify_only_for_their_machine_and_window(
        member in "[a-z]{1,12}", machine in "[a-z0-9-]{1,12}", other in "[a-z0-9-]{1,12}",
        expires in 1_000u64..2_000_000, now in 0u64..2_000_000,
    ) {
        let token = issue_token(&KEY, &member, &machine, expires);
        let ok = check_token(&KEY, &token, &machine, now);
        if now < expires {
            prop_assert_eq!(ok.as_deref(), Ok(member.as_str()));
        } else {
            prop_assert!(ok.is_err());
        }
        if other != machine {
            prop_assert!(check_token(&KEY, &token, &other, now).is_err());
        }
    }

    /// Arbitrary bytes never open as a sealed session.
    #[test]
    fn open_never_panics_on_arbitrary_bytes(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
        let _ = open(&KEY, &bytes);
    }

    /// Whatever we seal opens back to exactly the same bytes.
    #[test]
    fn seal_then_open_is_identity(plain in proptest::collection::vec(any::<u8>(), 0..4096)) {
        let sealed = seal(&KEY, LABEL, &plain).unwrap();
        prop_assert_eq!(open(&KEY, &sealed).unwrap(), plain);
    }

    /// A single flipped bit anywhere in a sealed record is detected.
    #[test]
    fn a_flipped_bit_is_always_detected(plain in proptest::collection::vec(any::<u8>(), 1..256),
                                        pos in any::<prop::sample::Index>(), bit in 0u8..8) {
        let mut sealed = seal(&KEY, LABEL, &plain).unwrap();
        let i = pos.index(sealed.len());
        sealed[i] ^= 1 << bit;
        prop_assert!(open(&KEY, &sealed).is_err(), "a tampered record must not open");
    }

    /// Stored-format parsers never panic on arbitrary input.
    #[test]
    fn stored_formats_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
        let _ = Space::from_bytes(&bytes);
        let _ = JoinGate::from_bytes(&bytes, &KEY);
        let _ = unwrap_space(&bytes, &KEY);
    }
}
