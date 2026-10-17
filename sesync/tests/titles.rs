//! Title rules, ported from daemon/tests/test_titles.py. These are the spec.
//! A mirror name must never carry a date that is not the user's.

use sesync::titles::{clean_opencode_title, claude_title_from_lines, omp_title_from_lines};

#[test]
fn default_timestamp_title_is_untitled() {
    assert_eq!(clean_opencode_title(Some("New session - 2026-10-14T16:52:39.241Z")), "");
}

#[test]
fn default_title_with_other_precision_is_untitled() {
    assert_eq!(clean_opencode_title(Some("New session - 2026-09-21T16:29:28.123Z")), "");
}

#[test]
fn real_title_is_kept() {
    assert_eq!(clean_opencode_title(Some("Research harness stores")), "Research harness stores");
}

#[test]
fn title_that_mentions_a_date_is_kept() {
    // Only the exact O-penCode default pattern is stripped.
    assert_eq!(clean_opencode_title(Some("Plan for 2026-10-14")), "Plan for 2026-10-14");
}

#[test]
fn missing_title_is_empty() {
    assert_eq!(clean_opencode_title(None), "");
    assert_eq!(clean_opencode_title(Some("   ")), "");
}

#[test]
fn claude_ai_title_beats_first_prompt() {
    let lines = [
        r#"{"type":"user","isMeta":false,"message":{"role":"user","content":"This session is being continued from a previous conversation"}}"#,
        r#"{"type":"ai-title","aiTitle":"Configure build pipeline servers"}"#,
        r#"{"type":"user","isMeta":false,"message":{"role":"user","content":"some first prompt"}}"#,
    ];
    let (title, has_user) = claude_title_from_lines(&lines);
    assert_eq!(title, "Configure build pipeline servers");
    assert!(has_user);
}

#[test]
fn claude_custom_title_beats_ai_title() {
    // /rename and -n write custom-title; Claude prefers it over ai-title.
    let lines = [
        r#"{"type":"ai-title","aiTitle":"Generated"}"#,
        r#"{"type":"custom-title","customTitle":"User chose this"}"#,
        r#"{"type":"user","isMeta":false,"message":{"role":"user","content":"prompt"}}"#,
    ];
    assert_eq!(claude_title_from_lines(&lines).0, "User chose this");
}

#[test]
fn claude_continuation_banner_is_never_the_title() {
    let lines = [
        r#"{"type":"user","isMeta":false,"message":{"role":"user","content":"This session is being continued from a previous conversation"}}"#,
        r#"{"type":"user","isMeta":false,"message":{"role":"user","content":"Real question here"}}"#,
    ];
    assert_eq!(claude_title_from_lines(&lines).0, "Real question here");
}

#[test]
fn omp_title_record_beats_header_title() {
    let lines = [
        r#"{"type":"session","version":3,"id":"x","cwd":"/tmp","title":"Old header"}"#,
        r#"{"type":"title","v":1,"title":"Renamed"}"#,
    ];
    assert_eq!(omp_title_from_lines(&lines).0, "Renamed");
}
