//! TUI state logic, ported from daemon/hs_tui.py. The rules are pinned here
//! without a terminal: a delete needs two presses on the same row within the
//! confirm window; any other key cancels; rename validates before it saves;
//! duplicate and trivial navigation wraps around.

use harness_sync::tui::{Model, Key};
use std::time::{Duration, Instant};

fn model_with(keys: &[&str]) -> Model {
    let mut m = Model::new(keys.iter().map(|k| k.to_string()).collect());
    m.select(0);
    m
}

#[test]
fn one_press_arms_a_delete_and_does_not_delete() {
    let mut m = model_with(&["a", "b"]);
    let t = Instant::now();
    let act = m.press(Key::CtrlD, t);
    assert_eq!(act, None, "a single press must not delete");
    assert!(m.armed().is_some());
}

#[test]
fn a_second_press_on_the_same_row_in_time_deletes() {
    let mut m = model_with(&["a", "b"]);
    let t = Instant::now();
    m.press(Key::CtrlD, t);
    let act = m.press(Key::CtrlD, t + Duration::from_secs(2));
    assert_eq!(act, Some("a".to_string()), "the second press deletes the armed row");
}

#[test]
fn a_second_press_after_the_window_does_not_delete() {
    let mut m = model_with(&["a"]);
    let t = Instant::now();
    m.press(Key::CtrlD, t);
    let act = m.press(Key::CtrlD, t + Duration::from_secs(60));
    assert_eq!(act, None, "a stale arm must expire, not delete");
}

#[test]
fn moving_to_another_row_cancels_the_arm() {
    let mut m = model_with(&["a", "b"]);
    let t = Instant::now();
    m.press(Key::CtrlD, t);
    m.press(Key::Other, t);
    assert!(m.armed().is_none(), "any other key cancels a pending delete");
    assert_eq!(m.press(Key::CtrlD, t), None, "and the next ctrl+d only re-arms");
}

#[test]
fn rename_validates_before_it_is_accepted() {
    let m = model_with(&["a"]);
    assert!(m.check_rename("  ok  ").is_ok());
    assert!(m.check_rename("   ").is_err());
    assert!(m.check_rename("bad\u{7}").is_err());
}

#[test]
fn next_duplicate_wraps_around() {
    let mut m = Model::new(vec!["a".into(), "b".into(), "c".into()]);
    m.set_duplicates(vec![0, 2]);
    m.select(2);
    assert_eq!(m.next_duplicate(), Some(0), "from the last duplicate it wraps to the first");
}

#[test]
fn next_trivial_with_none_is_none() {
    let mut m = model_with(&["a"]);
    m.set_trivial(vec![]);
    assert_eq!(m.next_trivial(), None);
}
