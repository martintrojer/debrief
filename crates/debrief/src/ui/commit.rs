//! The commit view's diff pane: a `●` review gutter (while the commit has
//! comments), a `oooo nnnn ±` gutter, then the row text,
//! cut at the pane's width and scrolled horizontally with the cursor.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use super::palette;
use crate::app::commit::gutter_width;
use crate::app::review::MARKER;
use crate::app::{App, DiffRow, Focus, Mode, RowKind};
use crate::vcs::LineKind;

const RED: Color = Color::Rgb(0xf3, 0x8b, 0xa8);
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
        RowKind::Line(LineKind::Removed) => s.fg(RED),
        RowKind::Line(LineKind::Context) => s,
        RowKind::NoNewline | RowKind::Binary | RowKind::Blank => s.fg(palette::OVERLAY),
    }
}

/// The gutter: old and new line numbers (`w` digits each) and the sign.
fn gutter(app: &App, r: &DiffRow, w: usize) -> String {
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
        RowKind::Line(LineKind::Added) => '+',
        RowKind::Line(LineKind::Removed) => '-',
        _ => ' ',
    };
    match line {
        Some(l) => format!("{:>w$} {:>w$} {sign} ", num(l.old), num(l.new)),
        None => " ".repeat(gutter_width(w)),
    }
}

/// The graphemes of `text` in display columns `[from, from + width)`,
/// as (column, grapheme) pairs; a wide grapheme cut by an edge is dropped.
fn visible(text: &str, from: usize, width: usize) -> Vec<(usize, &str)> {
    let mut col = 0;
    let mut out = Vec::new();
    for g in text.graphemes(true) {
        let w = g.width();
        if w == 0 {
            continue;
        }
        if col >= from && col + w <= from + width {
            out.push((col, g));
        }
        col += w;
        if col >= from + width {
            break;
        }
    }
    out
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
    let sel = app.selection_rows();
    let hits = app.search_hits();
    let focused = app.focus() == Focus::Diff || app.mode() != Mode::Normal;
    let num_w = app.commit_num_width();
    let marks = app.review_gutter();
    let marked = app.review_marked_rows();
    let text_w = (area.width as usize).saturating_sub(gutter_width(num_w) + marks);
    let lines: Vec<Line> = rows
        .iter()
        .enumerate()
        .skip(scroll)
        .take(area.height as usize)
        .map(|(i, r)| {
            let mut base = style_of(r.kind);
            let selected = sel.is_some_and(|(lo, hi)| (lo..=hi).contains(&i));
            if selected {
                base = base.bg(SELECTED);
            } else if i == cur.row {
                base = base.bg(palette::SURFACE);
            }
            let mut spans = Vec::new();
            if marks > 0 {
                let m = if marked.binary_search(&i).is_ok() {
                    MARKER.to_string()
                } else {
                    " ".to_string()
                };
                spans.push(Span::styled(m, base.fg(palette::YELLOW)));
            }
            spans.push(Span::styled(
                gutter(app, r, num_w),
                base.fg(palette::OVERLAY),
            ));
            let mut col = hscroll;
            for (c, g) in visible(&r.text, hscroll, text_w) {
                if c > col {
                    spans.push(Span::styled(" ".repeat(c - col), base));
                }
                let mut s = base;
                if hits
                    .iter()
                    .any(|&(hr, a, b)| hr == i && (a..b).contains(&c))
                {
                    s = s.bg(HIT).fg(palette::BASE);
                }
                if focused && i == cur.row && c == cur.col && sel.is_none() {
                    s = s.add_modifier(Modifier::REVERSED);
                }
                spans.push(Span::styled(g.to_string(), s));
                col = c + g.width();
            }
            if focused && i == cur.row && r.text.is_empty() && sel.is_none() {
                spans.push(Span::styled(" ", base.add_modifier(Modifier::REVERSED)));
            }
            Line::from(spans).style(base)
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), area);
}

#[cfg(test)]
mod tests {
    use super::visible;

    #[test]
    fn visible_cuts_by_columns() {
        let v: Vec<_> = visible("ab日cd", 1, 3).into_iter().collect();
        assert_eq!(v, [(1, "b"), (2, "日")]);
        let v: Vec<_> = visible("ab日cd", 3, 2).into_iter().collect();
        assert_eq!(v, [(4, "c")], "a cut wide grapheme is dropped");
    }
}
