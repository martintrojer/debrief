//! The commit view's file list: a tree of `▾ dir +n −m` rows and,
//! indented under them, `S name +n −m` file rows (`S` a coloured A/M/D/R/C
//! status letter), or flat `S path +n −m` rows; a border on its edge
//! toward the diff, `▎` on the file holding the diff cursor (or the
//! collapsed directory hiding it). Names too long are clipped in the
//! middle, keeping the extension.
//!
//! Under the tree, the commit list (`[view] sidebar_commits`): a rule,
//! then the log's rows as `<marker> <short id> [session] <summary>`,
//! clipped to the width; the open commit's row on the cursor-row
//! background, the list's cursor bold on it while the list has focus;
//! the log's dim `… more` row ends it while more commits may exist. The
//! log's search and filter matches are marked as in the log, and a
//! path filter shows on the rule as `─ ⌕ path ─`.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

use super::log::{WORKING_MARKER, hl, marker, more_line};
use super::palette;
use crate::app::config::SidebarSide;
use crate::app::file_tree::clip_middle;
use crate::app::sidebar::{GUTTER_COLS, STATUS_COLS, counts_label, lead_col};
use crate::app::{App, Focus, WORKING_ID, short_id};
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

/// The file list's border, on its edge toward the diff; `area` cleared
/// first (over the diff, a peek, nothing of it shows through). Returns
/// the area inside it.
fn bordered(frame: &mut Frame, app: &App, area: Rect) -> Rect {
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
    inner
}

/// `s` cut to `w` columns.
fn clip(s: &str, w: usize) -> String {
    let mut out = String::new();
    let mut used = 0;
    for c in s.chars() {
        let cw = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        if used + cw > w {
            break;
        }
        used += cw;
        out.push(c);
    }
    out
}

/// The commit list: a rule on the top row of `area`, then the rows.
pub(super) fn draw_commits(frame: &mut Frame, app: &App, area: Rect) {
    let inner = bordered(frame, app, area);
    if inner.height == 0 {
        return;
    }
    // The rule names the branch and path filter: `─ ⎇ main  ⌕ src ──`.
    let w = inner.width as usize;
    let rule = if w > 4 {
        let chips = super::log::chips(app, w - 3);
        let used = 3 + chips.iter().map(Span::width).sum::<usize>();
        let mut spans = vec![Span::styled("─ ", Style::new().fg(palette::OVERLAY))];
        spans.extend(chips);
        spans.push(Span::styled(
            format!(" {}", "─".repeat(w.saturating_sub(used))),
            Style::new().fg(palette::OVERLAY),
        ));
        Line::from(spans)
    } else {
        Line::styled("─".repeat(w), Style::new().fg(palette::OVERLAY))
    };
    frame.render_widget(Paragraph::new(rule), Rect { height: 1, ..inner });
    let list = Rect {
        y: inner.y + 1,
        height: inner.height - 1,
        ..inner
    };
    let focused = app.focus() == Focus::Commits;
    let open = app.commit_list_open_row();
    let cursor = app.commit_list_cursor();
    let width = list.width as usize;
    let pats = app.log_patterns();
    let mut lines: Vec<Line> = app
        .rows()
        .iter()
        .enumerate()
        .skip(app.commit_list_offset())
        .take(list.height as usize)
        .map(|(i, r)| {
            let mut base = Style::new().fg(palette::TEXT);
            if open == Some(i) || (focused && i == cursor) {
                base = base.bg(palette::SURFACE);
            }
            if focused && i == cursor {
                base = base.add_modifier(Modifier::BOLD);
            }
            let (mut m, mut mc) = marker(r.status);
            if r.working {
                (m, mc) = (WORKING_MARKER, palette::YELLOW);
            }
            let mut spans = vec![Span::styled(format!("{m} "), base.fg(mc))];
            let mut rest = width.saturating_sub(2);
            let mut push = |text: &str, style: Style, spans: &mut Vec<Span<'static>>| {
                let t = clip(text, rest);
                rest -= t.width();
                spans.extend(hl(t, style, &pats));
            };
            if r.info.commit == WORKING_ID {
                push(&r.info.summary, base.fg(palette::YELLOW), &mut spans);
            } else {
                push(
                    &format!("{} ", short_id(&r.info)),
                    base.fg(palette::MAUVE),
                    &mut spans,
                );
                if let Some(s) = app.session_marker(r) {
                    push(&format!("{s} "), base.fg(palette::TEAL), &mut spans);
                }
                push(&r.info.summary, base, &mut spans);
            }
            // The row's background runs to the border.
            spans.push(Span::styled(" ".repeat(rest), base));
            Line::from(spans).style(base)
        })
        .collect();
    let shown = app.rows().len().saturating_sub(app.commit_list_offset());
    if app.more_row() && shown < list.height as usize {
        lines.push(more_line());
    }
    frame.render_widget(Paragraph::new(lines), list);
}

pub(super) fn draw(frame: &mut Frame, app: &App, area: Rect) {
    let inner = bordered(frame, app, area);
    let (Some(diff), Some(list)) = (app.commit_diff(), app.file_list()) else {
        return;
    };
    let focused = app.focus() == Focus::Files;
    let sel = list.selected;
    let h = inner.height as usize;
    let skip = app.files_skip(h);
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
            let pad_l = lead_col(e, width) - GUTTER_COLS;
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
