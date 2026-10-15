//! TUI state, ported from daemon/hs_tui.py. Pure state with no terminal, so the
//! rules are tested directly. The ratatui front end drives this model.

use crate::manage::validate_title;
use std::time::{Duration, Instant};

/// Seconds a delete stays armed. Matches CONFIRM_SECONDS in hs_tui.py.
pub const CONFIRM_WINDOW: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Key {
    CtrlD,
    Other,
}

pub struct Model {
    rows: Vec<String>,
    selected: usize,
    duplicates: Vec<usize>,
    trivial: Vec<usize>,
    armed: Option<(String, Instant)>,
}

impl Model {
    pub fn new(rows: Vec<String>) -> Model {
        Model { rows, selected: 0, duplicates: Vec::new(), trivial: Vec::new(), armed: None }
    }

    pub fn select(&mut self, i: usize) {
        self.selected = i.min(self.rows.len().saturating_sub(1));
    }

    pub fn set_duplicates(&mut self, d: Vec<usize>) {
        self.duplicates = d;
    }

    pub fn set_trivial(&mut self, t: Vec<usize>) {
        self.trivial = t;
    }

    pub fn armed(&self) -> Option<&(String, Instant)> {
        self.armed.as_ref()
    }

    /// A key press. Returns the row to delete on a confirmed second ctrl+d, else None.
    /// The first ctrl+d arms the selected row; any other key cancels the arm.
    pub fn press(&mut self, key: Key, now: Instant) -> Option<String> {
        let row = self.rows.get(self.selected)?.clone();
        match key {
            Key::CtrlD => {
                if let Some((armed_row, at)) = self.armed.take() {
                    if armed_row == row && now.duration_since(at) <= CONFIRM_WINDOW {
                        return Some(row);
                    }
                }
                self.armed = Some((row, now));
                None
            }
            Key::Other => {
                self.armed = None;
                None
            }
        }
    }

    /// A new name is valid when it passes the same check as the daemon's rename.
    pub fn check_rename(&self, title: &str) -> Result<String, String> {
        validate_title(title)
    }

    /// Next duplicate after the selected row, wrapping to the first.
    pub fn next_duplicate(&mut self) -> Option<usize> {
        next_after(&self.duplicates, self.selected)
    }

    pub fn next_trivial(&mut self) -> Option<usize> {
        next_after(&self.trivial, self.selected)
    }
}

fn next_after(list: &[usize], current: usize) -> Option<usize> {
    if list.is_empty() {
        return None;
    }
    list.iter().copied().find(|&i| i > current).or_else(|| list.first().copied())
}
