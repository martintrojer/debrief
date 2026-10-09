//! The log: one row per commit, newest first:
//! `<marker> <short id> <age> <author> <summary>`.

use std::time::{SystemTime, UNIX_EPOCH};

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::palette;
use crate::app::{App, age, short_id};
use crate::seen::Status;

/// Columns given to the author.
const AUTHOR_W: usize = 12;

/// The marker and its colour for a status.
pub fn marker(s: Status) -> (&'static str, Color) {
    match s {
        Status::New => ("●", Color::Cyan),
        Status::Updated => ("◐", Color::Yellow),
        Status::Seen => (" ", palette::TEXT),
    }
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// `s` cut or padded to `w` characters.
fn fit(s: &str, w: usize) -> String {
    let mut out: String = s.chars().take(w).collect();
    let n = out.chars().count();
    out.extend(std::iter::repeat_n(' ', w - n));
    out
}

pub(super) fn draw(frame: &mut Frame, app: &App, area: Rect) {
    if app.rows().is_empty() {
        frame.render_widget(
            Paragraph::new(" no commits").style(Style::new().fg(palette::OVERLAY)),
            area,
        );
        return;
    }
    let now = now();
    let lines: Vec<Line> = app
        .rows()
        .iter()
        .enumerate()
        .skip(app.scroll())
        .take(area.height as usize)
        .map(|(i, r)| {
            let (m, mc) = marker(r.status);
            let mut base = Style::new().fg(palette::TEXT);
            if i == app.cursor() {
                base = base.bg(palette::SURFACE);
            }
            Line::from(vec![
                Span::styled(format!("{m} "), base.fg(mc)),
                Span::styled(format!("{} ", short_id(&r.info)), base.fg(palette::MAUVE)),
                Span::styled(
                    format!("{:>4} ", age(now, r.info.time)),
                    base.fg(palette::GREEN),
                ),
                Span::styled(
                    format!("{} ", fit(&r.info.author, AUTHOR_W)),
                    base.fg(palette::BLUE),
                ),
                Span::styled(r.info.summary.clone(), base),
            ])
            .style(base)
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), area);
}
