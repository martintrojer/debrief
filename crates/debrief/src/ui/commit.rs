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
/// The hatch of a filler row.
const FILLER: &str = "╱";

pub(super) fn style_of(kind: RowKind) -> Style {
    let s = Style::new().fg(palette::TEXT);
    match kind {
        RowKind::Message => s,
        RowKind::FileHeader => s.fg(palette::BLUE).add_modifier(Modifier::BOLD),
        RowKind::HunkHeader => s.fg(palette::MAUVE).add_modifier(Modifier::BOLD),
        RowKind::Line(LineKind::Added) => s.fg(palette::GREEN),
        RowKind::Line(LineKind::Removed) => s.fg(palette::RED),
        RowKind::Line(LineKind::Context) => s,
        RowKind::NoNewline | RowKind::Binary | RowKind::Blank | RowKind::Filler => {
            s.fg(palette::OVERLAY)
        }
    }
}

/// The background a row of `kind` gets across the whole pane, if any.
pub(super) fn row_bg(kind: RowKind) -> Option<Color> {
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
    let line = diff_line(app, r);
    let num = |n: Option<u32>| n.map_or(String::new(), |n| n.to_string());
    let sign = sign_of(r.kind);
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

/// The empty pane: loading, or nothing to show.
fn draw_empty(frame: &mut Frame, app: &App, area: Rect) {
    let msg = if app.commit_loading() {
        " loading…"
    } else {
        " nothing to show"
    };
    frame.render_widget(
        Paragraph::new(msg).style(Style::new().fg(palette::OVERLAY)),
        area,
    );
}

pub(super) fn draw(frame: &mut Frame, app: &App, area: Rect) {
    let rows = app.commit_rows();
    if rows.is_empty() {
        return draw_empty(frame, app, area);
    }
    if app.commit_split() {
        return super::split::draw(frame, app, area);
    }
    let num_w = app.commit_num_width();
    let cur = app.commit_cursor();
    let focused = app.focus() == Focus::Diff || app.mode() != Mode::Normal;
    let pane = Pane {
        rows,
        hscroll: app.commit_scroll().1,
        hits: app.search_hits(),
        cursor: Some(cur.row),
        caret: focused.then_some(cur),
        // A comment being typed keeps its rows highlighted.
        sel: app.selection_rows().or(app.comment_rows()),
        marks: app.review_gutter(),
        marked: app.review_marked_rows(),
        gutter: Box::new(|r: &DiffRow| gutter(app, r, num_w)),
        gutter_w: gutter_width(num_w),
        side: None,
    };
    let scroll = app.commit_scroll().0;
    for (y, (i, r)) in rows
        .iter()
        .enumerate()
        .skip(scroll)
        .take(area.height as usize)
        .enumerate()
    {
        let rect = Rect::new(area.x, area.y + y as u16, area.width, 1);
        draw_row(frame, app, &pane, i, r, rect);
    }
}

/// The diff line a row shows, if it is one.
pub(super) fn diff_line<'a>(app: &'a App, r: &DiffRow) -> Option<&'a crate::vcs::DiffLine> {
    let d = app.commit_diff()?;
    if !matches!(r.kind, RowKind::Line(_)) {
        return None;
    }
    let ((f, h), l) = r.file.zip(r.hunk).zip(r.line)?;
    d.files.get(f)?.hunks.get(h)?.lines.get(l)
}

/// The sign (and its colour) of a Line row.
pub(super) fn sign_of(kind: RowKind) -> (char, Color) {
    match kind {
        RowKind::Line(LineKind::Added) => ('+', palette::GREEN),
        RowKind::Line(LineKind::Removed) => ('-', palette::RED),
        _ => (' ', palette::OVERLAY),
    }
}

/// The gutter text of a row, and the sign that follows it.
pub(super) type Gutter<'a> = Box<dyn Fn(&DiffRow) -> (String, Option<(char, Color)>) + 'a>;

/// One column of rows as drawn: the unified pane, or one side.
pub(super) struct Pane<'a> {
    pub rows: &'a [DiffRow],
    pub hscroll: usize,
    /// (row, first column, end column).
    pub hits: Vec<(usize, usize, usize)>,
    /// The row that gets the cursor-row background.
    pub cursor: Option<usize>,
    /// Where the cursor cell is drawn (focused, on this pane).
    pub caret: Option<crate::app::Cursor>,
    pub sel: Option<(usize, usize)>,
    /// Columns of the marker gutter.
    pub marks: usize,
    pub marked: Vec<usize>,
    pub gutter: Gutter<'a>,
    /// Columns of `gutter` and the sign.
    pub gutter_w: usize,
    /// The side drawn in side by side (whose syntax colours).
    pub side: Option<debrief_review::Side>,
}

/// Draw row `i` of `p` in the one-row `rect`.
pub(super) fn draw_row(frame: &mut Frame, app: &App, p: &Pane, i: usize, r: &DiffRow, rect: Rect) {
    let text_w = (rect.width as usize).saturating_sub(p.gutter_w + p.marks);
    let selected = p.sel.is_some_and(|(lo, hi)| (lo..=hi).contains(&i));
    let bg = if selected {
        Some(SELECTED)
    } else if p.cursor == Some(i) {
        Some(palette::SURFACE)
    } else {
        row_bg(r.kind)
    };
    // Paint the background across the pane: a Line's style only reaches
    // the cells it draws.
    if let Some(bg) = bg {
        frame.buffer_mut().set_style(rect, Style::new().bg(bg));
    }
    let base = match bg {
        Some(bg) => style_of(r.kind).bg(bg),
        None => style_of(r.kind),
    };
    let caret = p.caret.filter(|c| c.row == i && p.sel.is_none());
    let mut spans = Vec::new();
    if p.marks > 0 {
        let m = if p.marked.binary_search(&i).is_ok() {
            MARKER.to_string()
        } else {
            " ".to_string()
        };
        spans.push(Span::styled(m, base.fg(palette::YELLOW)));
    }
    let (nums, sign) = (p.gutter)(r);
    spans.push(Span::styled(nums, base.fg(palette::OVERLAY)));
    if let Some((c, fg)) = sign {
        spans.push(Span::styled(format!("{c} "), base.fg(fg)));
    }
    if r.kind == RowKind::Filler {
        // The shorter side of a change block: a dim hatch.
        let s = base.fg(palette::OVERLAY).add_modifier(Modifier::DIM);
        let mut n = text_w;
        if caret.is_some() {
            spans.push(Span::styled(" ", base.add_modifier(Modifier::REVERSED)));
            n = n.saturating_sub(1);
        }
        spans.push(Span::styled(FILLER.repeat(n), s));
        frame.render_widget(Paragraph::new(Line::from(spans).style(base)), rect);
        return;
    }
    let syntax = app.side_spans(r, p.side);
    let mut col = p.hscroll;
    for (c, start, end) in visible(&r.text, p.hscroll, text_w) {
        if start > col {
            spans.push(Span::styled(" ".repeat(start - col), base));
        }
        let mut s = base;
        if let Some(st) = syntax.and_then(|sp| style_at(sp, c.byte)) {
            s = s.patch(st);
        }
        if p.hits
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
        if caret.is_some_and(|cur| c.col == cur.col) {
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
        let end = p.hscroll + text_w;
        if end > col {
            spans.push(Span::styled("─".repeat(end - col), base));
        }
    }
    // A row with no cells (empty, or only a hidden trailing CR) still
    // shows the cursor.
    if caret.is_some() && display::cells(&r.text).is_empty() {
        spans.push(Span::styled(" ", base.add_modifier(Modifier::REVERSED)));
    }
    frame.render_widget(Paragraph::new(Line::from(spans).style(base)), rect);
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
