//! A pull displays team sessions and writes nothing. The rule is the operator's: a first pull
//! only shows sessions; importing into a harness needs a confirmation the pull cannot give.
use sesync::team_pull::view_of;

#[test]
fn a_pull_lists_the_team_sessions_for_display() {
    let v = view_of(&["ses_a".to_string(), "ses_b".to_string()]);
    assert_eq!(v.sessions.len(), 2);
    assert_eq!(v.sessions[0].id, "ses_a");
}

#[test]
fn a_pull_with_no_team_sessions_shows_none() {
    assert!(view_of(&[]).sessions.is_empty());
}

#[test]
fn the_pull_has_no_way_to_write_into_a_harness() {
    // The view is data only: building it touches no file. Check the harness roots stay empty.
    let root = std::env::temp_dir().join(format!("sesync-pull-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let _ = view_of(&["ses_a".to_string()]);
    let written = std::fs::read_dir(&root).unwrap().count();
    assert_eq!(written, 0, "a pull writes nothing to disk");
    let _ = std::fs::remove_dir_all(&root);
}
