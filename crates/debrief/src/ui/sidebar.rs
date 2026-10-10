//! The commit view's file list: one row per file, `S path +n −m` (`S` a
//! coloured A/M/D/R/C status letter), a border on its edge toward the
//! diff, `▎` on the file holding the diff cursor.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

use super::palette;
use crate::app::commit::{file_counts, file_path};
use crate::app::config::SidebarSide;
use crate::app::sidebar::STATUS_COLS;
use crate::app::sidebar::counts_label;
use crate::app::{App, Focus};
use crate::vcs::FileStatus;

/// The current-file marker, drawn in the left gutter.
pub const CURRENT: &str = "▎";

/// A status letter's colour (tuicr's: A green, M yellow, D red, R pink;
/// C teal).
pub fn status_color(s: FileStatus) -> Color {
    match s {
        FileStatus::Added => palette::GREEN,
        FileStatus::Modified => palette::YELLOW,
        FileStatus::Deleted => palette::RED,
        FileStatus::Renamed => palette::PINK,
        FileStatus::Copied => palette::TEAL,
    }
}

/// `s` cut from the left behind `…` to at most `max` columns.
fn clip_start(s: &str, max: usize) -> String {
    if s.width() <= max {
        return s.to_string();
    }
    let mut out: Vec<char> = Vec::new();
    let mut w = 1;
    for c in s.chars().rev() {
        let cw = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        if w + cw > max {
            break;
        }
        w += cw;
        out.push(c);
    }
    out.reverse();
    format!("…{}", out.into_iter().collect::<String>())
}

pub(super) fn draw(frame: &mut Frame, app: &App, area: Rect) {
    // Over the diff (a peek), nothing of it shows through.
    frame.render_widget(Clear, area);
    let border = match app.config().view.sidebar_side {
        SidebarSide::Left => Borders::RIGHT,
        SidebarSide::Right => Borders::LEFT,
    };
    let block = Block::new()
        .borders(border)
        .border_style(Style::new().fg(palette::OVERLAY));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let Some(diff) = app.commit_diff() else {
        return;
    };
    let focused = app.focus() == Focus::Files;
    let current = app.current_file();
    let sel = app.sidebar_selected();
    let h = inner.height as usize;
    let skip = if focused {
        (sel + 1).saturating_sub(h)
    } else {
        current.map_or(0, |c| (c + 1).saturating_sub(h))
    };
    let width = inner.width as usize;
    let lines: Vec<Line> = diff
        .files
        .iter()
        .enumerate()
        .skip(skip)
        .take(h)
        .map(|(i, f)| {
            let mut base = Style::new().fg(palette::TEXT);
            if focused && i == sel {
                base = base.bg(palette::SURFACE).add_modifier(Modifier::BOLD);
            }
            let mark = if current == Some(i) {
                Span::styled(CURRENT, base.fg(palette::PEACH))
            } else {
                Span::styled(" ", base)
            };
            let (add, del) = file_counts(f);
            let counts = counts_label(add, del);
            let status = f.status();
            let letter = Span::styled(
                format!("{} ", status.letter()),
                base.fg(status_color(status)),
            );
            let room = width.saturating_sub(1 + STATUS_COLS + counts.width());
            let path = clip_start(file_path(f), room);
            let pad = room.saturating_sub(path.width());
            let (a, d) = counts.trim_start().split_once(' ').unwrap_or(("", ""));
            Line::from(vec![
                mark,
                letter,
                Span::styled(path, base),
                Span::styled(" ".repeat(pad + 1), base),
                Span::styled(a.to_string(), base.fg(palette::GREEN)),
                Span::styled(" ", base),
                Span::styled(d.to_string(), base.fg(palette::RED)),
            ])
            .style(base)
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

#[cfg(test)]
mod tests {
    use super::clip_start;

    #[test]
    fn clips_from_the_left() {
        assert_eq!(clip_start("src/a.rs", 20), "src/a.rs");
        assert_eq!(clip_start("src/long/a.rs", 6), "…/a.rs");
    }
}
