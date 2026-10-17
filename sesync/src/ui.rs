//! Drawing for the manage TUI, with ratatui. Reads the App and writes a frame.
//! No state is kept here: the App owns everything the screen shows.

use crate::app::{App, Mode};
use ratatui::{
    layout::{Constraint, Layout},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{List, ListItem, Paragraph},
    Frame,
};

const KEYS: &str = "ctrl+d delete (twice)  r rename  q quit";

pub fn draw(f: &mut Frame, app: &App) {
    let area = f.area();
    let chunks = Layout::vertical([
        Constraint::Min(1),    // session list
        Constraint::Length(1), // status or rename prompt
        Constraint::Length(1), // key help
    ])
    .split(area);

    let items: Vec<ListItem> = app
        .rows()
        .iter()
        .map(|g| ListItem::new(Line::from(vec![
            Span::styled(format!("{:<6}", g.fmt), Style::default().add_modifier(Modifier::DIM)),
            // A star marks a session the user chose for the team; unmarked rows show a space.
            Span::raw(if app.is_marked(g) { "* " } else { "  " }),
            Span::raw(g.title.clone()),
        ])))
        .collect();
    f.render_widget(List::new(items), chunks[0]);

    let status_line = match app.mode() {
        Mode::Rename => format!("Rename: {}_", app.input()),
        Mode::Browse => app.status().to_string(),
    };
    f.render_widget(Paragraph::new(status_line), chunks[1]);
    f.render_widget(Paragraph::new(KEYS).style(Style::default().add_modifier(Modifier::DIM)), chunks[2]);
}
