//! Rendering, checked on ratatui's TestBackend buffer. The screen must show the
//! session list, the status line, and the rename prompt when renaming.

use harness_sync::app::{App, Ev};
use harness_sync::manage::Roots;
use harness_sync::state::State;
use harness_sync::ui::draw;
use ratatui::{backend::TestBackend, Terminal};
use std::fs;
use std::path::PathBuf;
use std::time::Instant;

fn sandbox(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("hs-render-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

fn app_with_one_session(sb: &PathBuf, title: &str) -> App {
    let roots = Roots {
        omp: sb.join("omp").join("sessions"),
        pi: sb.join("pi").join("sessions"),
        claude: sb.join("claude").join("projects"),
    };
    let dir = roots.omp.join("-tmp");
    fs::create_dir_all(&dir).unwrap();
    fs::create_dir_all(&roots.pi).unwrap();
    fs::create_dir_all(&roots.claude).unwrap();
    let sid = "aaaaaaaa-1111-2222-3333-444444444444";
    fs::write(dir.join(format!("2026-10-01T10-00-00-000Z_{sid}.jsonl")),
        format!("{{\"type\":\"session\",\"version\":3,\"id\":\"{sid}\",\"cwd\":\"/tmp\",\"title\":\"{title}\"}}\n")).unwrap();
    App::new(roots, State::default(), sb.join("locks"), sb.join("no-hslock"))
}

fn screen(app: &App, w: u16, h: u16) -> String {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| draw(f, app)).unwrap();
    let buf = term.backend().buffer().clone();
    let mut out = String::new();
    for y in 0..h {
        for x in 0..w {
            out.push_str(buf[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}

#[test]
fn the_session_title_is_drawn() {
    let sb = sandbox("title");
    let app = app_with_one_session(&sb, "Visible title");
    let s = screen(&app, 80, 12);
    assert!(s.contains("Visible title"), "the list must show the session title:\n{s}");
}

#[test]
fn the_key_help_is_drawn() {
    let sb = sandbox("help");
    let app = app_with_one_session(&sb, "x");
    let s = screen(&app, 100, 12);
    assert!(s.contains("ctrl+d") && s.contains("rename"), "the footer must show the keys:\n{s}");
}

#[test]
fn the_status_line_is_drawn() {
    let sb = sandbox("status");
    let mut app = app_with_one_session(&sb, "x");
    app.handle(Ev::CtrlD, Instant::now());
    let s = screen(&app, 100, 12);
    assert!(s.contains("Press ctrl+d again"), "a pending delete must be visible:\n{s}");
}

#[test]
fn rename_mode_shows_the_input_with_the_current_title() {
    let sb = sandbox("rename");
    let mut app = app_with_one_session(&sb, "Current name");
    app.handle(Ev::Char('r'), Instant::now());
    let s = screen(&app, 100, 12);
    assert!(s.contains("Rename") && s.contains("Current name"), "rename prompt must show the title:\n{s}");
}
