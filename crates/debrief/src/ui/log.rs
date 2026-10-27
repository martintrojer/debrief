//! The log: one row per commit, newest first:
//! `<marker> <lead><short id> <age> <author> [session] [fold] <summary>`.
//! The lead is 2 columns per fold depth (empty on the spine, so a spine
//! row's short id starts at column 2): a glyph and a space per enclosing
//! octopus group, outermost first (`├`/`╰` for the row's own group, `│`
//! for an enclosing group with more groups after it, blank for an
//! enclosing last group), then 2 spaces per level a glyph does not cover.
//! A merge row's fold is `▸ +n` closed or `▾ +n` open (`n` the commits
//! it hides; `▸` alone until the size is in), after the session marker.
//! A fold group with more commits to load ends with a dim
//! `  <lead>… more` row (no id, age or author). git's working copy
//! is `✎ Working copy  N files` on top; jj's `@` gets the `✎` marker. A
//! commit with a saved review session shows `` (`3`: three unsent
//! comments) before its summary; git's working-copy row ends with it.
//! While more commits may exist, a dim `… more` row ends the list. A
//! path filter's `⌕ path` row tops the log.

use std::time::{SystemTime, UNIX_EPOCH};

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::search::find_all;
use crate::app::{App, MORE_ROW, WORKING_ID, age, is_fold_more, short_id};
use crate::seen::Status;
use crate::theme::Palette;

/// Columns given to the author.
const AUTHOR_W: usize = 12;
/// The working copy's marker.
pub const WORKING_MARKER: &str = "✎";

/// The marker and its colour for a status.
pub fn marker(pal: &Palette, s: Status) -> (&'static str, Color) {
    match s {
        Status::New => ("●", pal.cyan),
        Status::Updated => ("◐", pal.yellow),
        Status::Seen => (" ", pal.text),
    }
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// `text` in `style`, with the matches of `pats` marked as search hits.
pub(super) fn hl(text: String, style: Style, pats: &[&str], pal: &Palette) -> Vec<Span<'static>> {
    let mut hits: Vec<std::ops::Range<usize>> =
        pats.iter().flat_map(|p| find_all(&text, p)).collect();
    if hits.is_empty() {
        return vec![Span::styled(text, style)];
    }
    hits.sort_by_key(|h| h.start);
    let hit = style.bg(pal.yellow).fg(pal.bg);
    let mut out = Vec::new();
    let mut at = 0;
    for h in hits {
        if h.start < at {
            continue;
        }
        if h.start > at {
            out.push(Span::styled(text[at..h.start].to_string(), style));
        }
        out.push(Span::styled(text[h.clone()].to_string(), hit));
        at = h.end;
    }
    if at < text.len() {
        out.push(Span::styled(text[at..].to_string(), style));
    }
    out
}

/// `s` cut or padded to `w` characters.
fn fit(s: &str, w: usize) -> String {
    let mut out: String = s.chars().take(w).collect();
    let n = out.chars().count();
    out.extend(std::iter::repeat_n(' ', w - n));
    out
}

pub(super) fn draw(frame: &mut Frame, app: &App, area: Rect) {
    let pal = &app.theme().palette;
    if app.rows().is_empty() {
        frame.render_widget(
            Paragraph::new(" no commits").style(Style::new().fg(pal.muted)),
            area,
        );
        return;
    }
    let now = now();
    let pats = app.log_patterns();
    let mut lines: Vec<Line> = app
        .rows()
        .iter()
        .enumerate()
        .skip(app.scroll())
        .take(area.height as usize)
        .map(|(i, r)| {
            let (mut m, mut mc) = marker(pal, app.row_status(i));
            if r.working {
                (m, mc) = (WORKING_MARKER, pal.yellow);
            }
            let session = app.session_marker(r);
            let mut base = Style::new().fg(pal.text);
            if i == app.cursor() {
                base = base.bg(pal.cursor_bg);
            }
            if let (true, Some(w)) = (r.info.commit == WORKING_ID, app.working_copy()) {
                let files = match w.files {
                    1 => "1 file".to_string(),
                    n => format!("{n} files"),
                };
                let mut spans = vec![Span::styled(format!("{m} "), base.fg(mc))];
                spans.extend(hl(r.info.summary.clone(), base.fg(pal.yellow), &pats, pal));
                spans.push(Span::styled("  ", base));
                spans.push(Span::styled(files, base.fg(pal.muted)));
                if let Some(s) = &session {
                    spans.push(Span::styled(format!("  {s}"), base.fg(pal.cyan)));
                }
                return Line::from(spans).style(base);
            }
            if is_fold_more(&r.info.commit) {
                // A fold's `… more` row: no commit, no fields.
                let text = format!("  {}{}", r.lead(), r.info.summary);
                return Line::styled(text, base.fg(pal.muted));
            }
            let mut spans = vec![Span::styled(format!("{m} "), base.fg(mc))];
            let lead = r.lead();
            if !lead.is_empty() {
                spans.push(Span::styled(lead, base.fg(pal.muted)));
            }
            spans.extend(hl(
                short_id(&r.info).to_string(),
                base.fg(pal.purple),
                &pats,
                pal,
            ));
            spans.push(Span::styled(
                format!(" {:>4} ", age(now, r.info.time)),
                base.fg(pal.green),
            ));
            spans.extend(hl(
                fit(&r.info.author, AUTHOR_W),
                base.fg(pal.blue),
                &pats,
                pal,
            ));
            spans.push(Span::styled(" ", base));
            if let Some(s) = session {
                spans.push(Span::styled(format!("{s} "), base.fg(pal.cyan)));
            }
            if let Some(f) = r.fold_label() {
                spans.push(Span::styled(format!("{f} "), base.fg(pal.muted)));
            }
            spans.extend(hl(r.info.summary.clone(), base, &pats, pal));
            Line::from(spans).style(base)
        })
        .collect();
    if app.more_row() && app.rows().len() - app.scroll() < area.height as usize {
        lines.push(more_line(pal));
    }
    frame.render_widget(Paragraph::new(lines), area);
    let total = app.rows().len() + usize::from(app.more_row());
    super::scrollbar::draw(frame, app, "log", area, total, app.scroll());
}

/// The path filter's marker before its path.
pub const PATH_MARKER: &str = "⌕";

/// The path filter as one line: `⌕ src/app` (left-elided to `width`
/// columns).
pub fn path_chip(path: &str, width: usize) -> String {
    let room = width.saturating_sub(PATH_MARKER.chars().count() + 1);
    format!("{PATH_MARKER} {}", super::status::elide_left(path, room))
}

/// The branch's marker before its name.
pub const BRANCH_MARKER: &str = "⎇";

/// The chips naming what the log lists, in `width` columns:
/// `⎇ feat/x` and, under a path filter, `  ⌕ src/app`. The path is cut
/// first (from the left), then the branch (from the right).
pub fn chips(app: &App, width: usize) -> Vec<Span<'static>> {
    let pal = &app.theme().palette;
    use unicode_width::UnicodeWidthStr;
    let branch = format!("{BRANCH_MARKER} {}", app.branch_label());
    let branch = super::status::elide_right(&branch, width);
    let mut out = vec![Span::styled(branch.clone(), Style::new().fg(pal.green))];
    let room = width.saturating_sub(branch.width() + 2);
    if let Some(p) = app.path_filter()
        && room > PATH_MARKER.chars().count() + 1
    {
        out.push(Span::raw("  "));
        out.push(Span::styled(
            path_chip(p, room),
            Style::new().fg(pal.orange),
        ));
    }
    out
}

/// The row over the log naming its branch and path filter.
pub(super) fn draw_chip(frame: &mut Frame, app: &App, area: Rect) {
    let pal = &app.theme().palette;
    let mut spans = vec![Span::raw(" ")];
    spans.extend(chips(app, (area.width as usize).saturating_sub(1)));
    let style = Style::new().bg(pal.bg_dark);
    frame.render_widget(Paragraph::new(Line::from(spans)).style(style), area);
}

/// The dim row ending the log and the commit list while more commits may
/// exist.
pub fn more_line(pal: &Palette) -> Line<'static> {
    Line::styled(format!("  {MORE_ROW}"), Style::new().fg(pal.muted))
}
