//! Conflict display: differing copies are reported for a choice, never merged or overwritten.
use sesync::team_conflict::{compare, Comparison};

#[test]
fn identical_copies_are_the_same() {
    assert_eq!(compare(Some(b"x"), Some(b"x")), Comparison::Same);
}

#[test]
fn a_session_on_one_side_only_is_not_a_conflict() {
    assert_eq!(compare(Some(b"x"), None), Comparison::OnlyLocal);
    assert_eq!(compare(None, Some(b"x")), Comparison::OnlyTeam);
}

#[test]
fn differing_copies_are_a_conflict_with_both_hashes() {
    match compare(Some(b"local edit"), Some(b"team edit")) {
        Comparison::Conflict { local_sha, team_sha } => {
            assert_ne!(local_sha, team_sha);
            assert_eq!(local_sha.len(), 64);
        }
        other => panic!("expected a conflict, got {other:?}"),
    }
}

#[test]
fn a_conflict_report_never_contains_the_contents() {
    let c = compare(Some(b"secret local words"), Some(b"secret team words"));
    let text = format!("{c:?}");
    assert!(!text.contains("secret"), "the report carries hashes only: {text}");
}
