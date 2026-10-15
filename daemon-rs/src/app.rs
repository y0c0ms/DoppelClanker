//! TUI application state. Ported from daemon/hs_tui.py ManageApp. Key events go
//! in, state and file operations come out. No terminal code here, so it is tested
//! with real files; the ratatui front end only draws and forwards keys.

use crate::lock::ClaimOutcome;
use crate::manage::{self, Group, Roots};
use crate::state::State;
use crate::tui::{Key, Model};
use std::path::PathBuf;
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Mode {
    Browse,
    Rename,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Ev {
    CtrlD,
    Char(char),
    Backspace,
    Enter,
    Esc,
}

pub struct App {
    roots: Roots,
    state: State,
    lock_dir: PathBuf,
    hslock: PathBuf,
    groups: Vec<Group>,
    model: Model,
    mode: Mode,
    input: String,
    status: String,
    quit: bool,
}

impl App {
    pub fn new(roots: Roots, state: State, lock_dir: PathBuf, hslock: PathBuf) -> App {
        let groups = manage::list_groups(&roots, &state);
        let titles = groups.iter().map(|g| g.title.clone()).collect();
        App {
            model: Model::new(titles),
            roots, state, lock_dir, hslock, groups,
            mode: Mode::Browse,
            input: String::new(),
            status: String::new(),
            quit: false,
        }
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn status(&self) -> &str {
        &self.status
    }

    pub fn should_quit(&self) -> bool {
        self.quit
    }

    pub fn rows(&self) -> &[Group] {
        &self.groups
    }

    pub fn input(&self) -> &str {
        &self.input
    }

    fn selected(&self) -> Option<Group> {
        self.groups.get(self.model.selected()).cloned()
    }

    fn reload(&mut self) {
        self.groups = manage::list_groups(&self.roots, &self.state);
        self.model = Model::new(self.groups.iter().map(|g| g.title.clone()).collect());
    }

    pub fn handle(&mut self, ev: Ev, now: Instant) {
        match self.mode {
            Mode::Rename => self.handle_rename(ev),
            Mode::Browse => self.handle_browse(ev, now),
        }
    }

    fn handle_browse(&mut self, ev: Ev, now: Instant) {
        match ev {
            Ev::CtrlD => {
                let Some(g) = self.selected() else {
                    self.status = "nothing selected".into();
                    return;
                };
                if let Some(row) = self.model.press(Key::CtrlD, now) {
                    let _ = row;
                    self.delete_selected(&g);
                } else {
                    self.status = format!("Press ctrl+d again to DELETE '{}'. Any other key cancels.", g.title);
                }
            }
            Ev::Char('r') => {
                let Some(g) = self.selected() else { return };
                self.model.press(Key::Other, now);
                self.input = g.title.clone();
                self.mode = Mode::Rename;
                self.status = "Enter a new name; Esc cancels.".into();
            }
            Ev::Char('q') => {
                self.quit = true;
            }
            _ => {
                if self.model.armed().is_some() {
                    self.model.press(Key::Other, now);
                    self.status = "delete cancelled".into();
                }
            }
        }
    }

    fn handle_rename(&mut self, ev: Ev) {
        match ev {
            Ev::Char(c) => self.input.push(c),
            Ev::Backspace => {
                self.input.pop();
            }
            Ev::Esc => {
                self.mode = Mode::Browse;
                self.input.clear();
                self.status = "rename cancelled".into();
            }
            Ev::Enter => {
                let Some(g) = self.selected() else {
                    self.mode = Mode::Browse;
                    return;
                };
                let title = self.input.clone();
                match manage::rename_group(&self.roots, &g, &title, &self.lock_dir, &self.hslock) {
                    Ok(saved) => {
                        self.mode = Mode::Browse;
                        self.input.clear();
                        self.status = format!("renamed to '{saved}'");
                        self.reload();
                    }
                    Err(e) => {
                        // Invalid titles and refusals keep the input open so the user can fix it.
                        self.status = format!("refused: {e}");
                    }
                }
            }
            _ => {}
        }
    }

    fn delete_selected(&mut self, g: &Group) {
        match manage::delete_group(&self.roots, g, &self.lock_dir, &self.hslock) {
            Ok(done) => {
                self.status = format!("deleted {} item(s) for '{}'", done.len(), g.title);
                self.reload();
            }
            Err(e) => self.status = format!("refused: {e}"),
        }
    }
}

#[allow(dead_code)]
fn _unused(_: ClaimOutcome<()>) {}
