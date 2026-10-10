//! The commit view's file list: a tree of `▾ dir +n −m` rows and,
//! indented under them, `S name +n −m` file rows (`S` a coloured A/M/D/R/C
//! status letter), or flat `S path +n −m` rows; a border on its edge
//! toward the diff, `▎` on the file holding the diff cursor (or the
//! collapsed directory hiding it). Names too long are clipped in the
//! middle, keeping the extension.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

use super::palette;
use crate::app::config::SidebarSide;
use crate::app::file_tree::{clip_middle, indent};
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

/// A directory row's arrow: open / closed (ramble's).
pub const OPEN: &str = "▾";
pub const CLOSED: &str = "▸";

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
    let (Some(diff), Some(list)) = (app.commit_diff(), app.file_list()) else {
        return;
    };
    let focused = app.focus() == Focus::Files;
    let sel = list.selected;
    let h = inner.height as usize;
    let skip = if focused {
        (sel + 1).saturating_sub(h)
    } else {
        list.current.map_or(0, |c| (c + 1).saturating_sub(h))
    };
    let width = inner.width as usize;
    let lines: Vec<Line> = (skip..list.visible.rows.len())
        .take(h)
        .filter_map(|row| Some((row, list.entry(row)?)))
        .map(|(row, e)| {
            let mut base = Style::new().fg(palette::TEXT);
            if focused && row == sel {
                base = base.bg(palette::SURFACE).add_modifier(Modifier::BOLD);
            }
            let mark = if list.current == Some(row) {
                Span::styled(CURRENT, base.fg(palette::PEACH))
            } else {
                Span::styled(" ", base)
            };
            let counts = counts_label(e.add, e.del);
            let lead = match e.file.and_then(|i| diff.files.get(i)) {
                Some(f) => {
                    let status = f.status();
                    Span::styled(
                        format!("{} ", status.letter()),
                        base.fg(status_color(status)),
                    )
                }
                None => {
                    let arrow = if list.is_collapsed(e) { CLOSED } else { OPEN };
                    Span::styled(format!("{arrow} "), base.fg(palette::BLUE))
                }
            };
            let pad_l = indent(
                e.depth,
                width.saturating_sub(counts.width()),
                1 + STATUS_COLS,
            );
            let room = width.saturating_sub(1 + pad_l + STATUS_COLS + counts.width());
            let name = clip_middle(&e.name, room);
            let name_style = if e.is_dir() {
                base.fg(palette::BLUE)
            } else {
                base
            };
            let pad = room.saturating_sub(name.width());
            let (a, d) = counts.trim_start().split_once(' ').unwrap_or(("", ""));
            Line::from(vec![
                mark,
                Span::styled(" ".repeat(pad_l), base),
                lead,
                Span::styled(name, name_style),
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
