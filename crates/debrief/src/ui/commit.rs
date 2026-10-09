//! The commit view's diff pane: a `●` review gutter (while the commit has
//! comments), a `oooo nnnn ±` gutter, then the row text laid out by
//! [`display::cells`], cut at the pane's width and scrolled horizontally
//! with the cursor. Added, removed, hunk-header, cursor and selected rows
//! are coloured across the whole pane; a file header's rule runs to the
//! right edge. Syntax colours, once they arrive, set the foreground of a
//! line's cells (an expanded tab takes its span's colour); the row
//! background, search hits, the cursor and the dimmed `?` of a control
//! character stay on top.

use super::palette;
use crate::app::commit::gutter_width;
use crate::app::display::{self, DisplayCell, Glyph};
use crate::app::highlight::style_at;
use crate::app::review::MARKER;
use crate::app::{App, DiffRow, Focus, Mode, RowKind};
use crate::vcs::LineKind;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

/// Selection background (Catppuccin Mocha surface2).
const SELECTED: Color = Color::Rgb(0x58, 0x5b, 0x70);
const HIT: Color = palette::YELLOW;

fn style_of(kind: RowKind) -> Style {
    let s = Style::new().fg(palette::TEXT);
    match kind {
        RowKind::Message => s,
        RowKind::FileHeader => s.fg(palette::BLUE).add_modifier(Modifier::BOLD),
        RowKind::HunkHeader => s.fg(palette::MAUVE).add_modifier(Modifier::BOLD),
        RowKind::Line(LineKind::Added) => s.fg(palette::GREEN),
        RowKind::Line(LineKind::Removed) => s.fg(palette::RED),
        RowKind::Line(LineKind::Context) => s,
        RowKind::NoNewline | RowKind::Binary | RowKind::Blank => s.fg(palette::OVERLAY),
    }
}

/// The background a row of `kind` gets across the whole pane, if any.
fn row_bg(kind: RowKind) -> Option<Color> {
    match kind {
        RowKind::Line(LineKind::Added) => Some(palette::ADD_BG),
        RowKind::Line(LineKind::Removed) => Some(palette::DEL_BG),
        RowKind::HunkHeader => Some(palette::HUNK_BG),
        _ => None,
    }
}

/// The gutter: old and new line numbers (`w` digits each), and the sign
/// (with its colour) that follows them on Line rows.
fn gutter(app: &App, r: &DiffRow, w: usize) -> (String, Option<(char, Color)>) {
    let line = match (app.commit_diff(), r.kind) {
        (Some(d), RowKind::Line(_)) => r
            .file
            .zip(r.hunk)
            .zip(r.line)
            .and_then(|((f, h), l)| d.files.get(f)?.hunks.get(h)?.lines.get(l)),
        _ => None,
    };
    let num = |n: Option<u32>| n.map_or(String::new(), |n| n.to_string());
    let sign = match r.kind {
        RowKind::Line(LineKind::Added) => ('+', palette::GREEN),
        RowKind::Line(LineKind::Removed) => ('-', palette::RED),
        _ => (' ', palette::OVERLAY),
    };
    match line {
        Some(l) => (
            format!("{:>w$} {:>w$} ", num(l.old), num(l.new)),
            Some(sign),
        ),
        None => (" ".repeat(gutter_width(w)), None),
    }
}

/// The display cells of `text` within columns `[from, from + width)`,
/// each with the columns `[start, end)` it shows there. An expanded tab cut
/// by an edge shows its remaining columns; any other cut cell is dropped.
fn visible(text: &str, from: usize, width: usize) -> Vec<(DisplayCell<'_>, usize, usize)> {
    let to = from + width;
    display::cells(text)
        .into_iter()
        .filter_map(|c| {
            let (start, end) = (c.col.max(from), (c.col + c.width).min(to));
            let whole = start == c.col && end == c.col + c.width;
            (start < end && (whole || c.glyph == Glyph::Tab)).then_some((c, start, end))
        })
        .collect()
}

pub(super) fn draw(frame: &mut Frame, app: &App, area: Rect) {
    let rows = app.commit_rows();
    if rows.is_empty() {
        let msg = if app.commit_loading() {
            " loading…"
        } else {
            " nothing to show"
        };
        frame.render_widget(
            Paragraph::new(msg).style(Style::new().fg(palette::OVERLAY)),
            area,
        );
        return;
    }
    let (scroll, hscroll) = app.commit_scroll();
    let cur = app.commit_cursor();
    // A comment being typed keeps its rows highlighted.
    let sel = app.selection_rows().or(app.comment_rows());
    let hits = app.search_hits();
    let focused = app.focus() == Focus::Diff || app.mode() != Mode::Normal;
    let num_w = app.commit_num_width();
    let marks = app.review_gutter();
    let marked = app.review_marked_rows();
    let text_w = (area.width as usize).saturating_sub(gutter_width(num_w) + marks);
    let mut lines = Vec::new();
    for (y, (i, r)) in rows
        .iter()
        .enumerate()
        .skip(scroll)
        .take(area.height as usize)
        .enumerate()
    {
        let selected = sel.is_some_and(|(lo, hi)| (lo..=hi).contains(&i));
        let bg = if selected {
            Some(SELECTED)
        } else if i == cur.row {
            Some(palette::SURFACE)
        } else {
            row_bg(r.kind)
        };
        // Paint the background across the pane: a Line's style only
        // reaches the cells it draws.
        if let Some(bg) = bg {
            let row = Rect::new(area.x, area.y + y as u16, area.width, 1);
            frame.buffer_mut().set_style(row, Style::new().bg(bg));
        }
        let base = match bg {
            Some(bg) => style_of(r.kind).bg(bg),
            None => style_of(r.kind),
        };
        let mut spans = Vec::new();
        if marks > 0 {
            let m = if marked.binary_search(&i).is_ok() {
                MARKER.to_string()
            } else {
                " ".to_string()
            };
            spans.push(Span::styled(m, base.fg(palette::YELLOW)));
        }
        let (nums, sign) = gutter(app, r, num_w);
        spans.push(Span::styled(nums, base.fg(palette::OVERLAY)));
        if let Some((c, fg)) = sign {
            spans.push(Span::styled(format!("{c} "), base.fg(fg)));
        }
        let syntax = app.commit_spans(r);
        let mut col = hscroll;
        for (c, start, end) in visible(&r.text, hscroll, text_w) {
            if start > col {
                spans.push(Span::styled(" ".repeat(start - col), base));
            }
            let mut s = base;
            if let Some(st) = syntax.and_then(|sp| style_at(sp, c.byte)) {
                s = s.patch(st);
            }
            if hits
                .iter()
                .any(|&(hr, a, b)| hr == i && (a..b).contains(&c.col))
            {
                s = s.bg(HIT).fg(palette::BASE);
            }
            let (text, pad) = match c.glyph {
                Glyph::Text(g) => (g.to_string(), String::new()),
                Glyph::Tab => (" ".to_string(), " ".repeat(end - start - 1)),
                Glyph::Control => {
                    s = s.fg(palette::OVERLAY).add_modifier(Modifier::DIM);
                    ("?".to_string(), String::new())
                }
            };
            let pad_style = s;
            if focused && i == cur.row && c.col == cur.col && sel.is_none() {
                s = s.add_modifier(Modifier::REVERSED);
            }
            spans.push(Span::styled(text, s));
            if !pad.is_empty() {
                spans.push(Span::styled(pad, pad_style));
            }
            col = end;
        }
        if r.kind == RowKind::FileHeader {
            // The header's rule runs to the right edge.
            let end = hscroll + text_w;
            if end > col {
                spans.push(Span::styled("─".repeat(end - col), base));
            }
        }
        // A row with no cells (empty, or only a hidden trailing CR) still
        // shows the cursor.
        if focused && i == cur.row && display::cells(&r.text).is_empty() && sel.is_none() {
            spans.push(Span::styled(" ", base.add_modifier(Modifier::REVERSED)));
        }
        lines.push(Line::from(spans).style(base));
    }
    frame.render_widget(Paragraph::new(lines), area);
}

#[cfg(test)]
mod tests {
    use super::visible;

    fn cols(text: &str, from: usize, width: usize) -> Vec<usize> {
        visible(text, from, width)
            .iter()
            .map(|&(_, start, _)| start)
            .collect()
    }

    #[test]
    fn visible_cuts_by_columns() {
        assert_eq!(cols("ab日cd", 1, 3), [1, 2]);
        assert_eq!(cols("ab日cd", 3, 2), [4], "a cut wide grapheme is dropped");
        assert_eq!(cols("\tx", 2, 1), [2], "a cut tab shows its remainder");
        assert_eq!(cols("\tx", 2, 4), [2, 4]);
        assert_eq!(cols("ab\tc", 0, 4), [0, 1, 2], "cut on the right too");
    }
}
