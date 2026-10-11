//! The diff pane side by side: old lines on the
//! left, new on the right, each half with its own `●` marker column,
//! line-number column, sign, row backgrounds, syntax colours (a context
//! line's on the left from the old side) and
//! horizontal scroll, and a one-column divider between them. Filler rows
//! (the shorter side of a change block) are a dim hatch. Message, header,
//! placeholder and blank rows span both halves. The cursor, the
//! selection and a comment's rows are drawn on the cursor's side.

use debrief_review::Side;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::Paragraph;

use super::commit::{Pane, diff_line, draw_row, sign_of};
use super::palette;
use crate::app::commit::side_gutter_width;
use crate::app::split::spans_both;
use crate::app::{App, DiffRow, Focus, Mode};

pub(super) fn draw(frame: &mut Frame, app: &App, area: Rect) {
    let Some(mine) = app.split_side() else { return };
    let num_w = app.commit_num_width();
    let cur = app.commit_cursor();
    let focused = app.focus() == Focus::Diff || app.mode() != Mode::Normal;
    let sel = app.selection_rows().or(app.comment_rows());
    let marks = app.review_gutter();
    let pane = |side: Side| {
        let on = side == mine;
        Pane {
            rows: app.split_rows(side),
            hscroll: app.split_hscroll(side),
            hits: app.split_search_hits(side),
            cursor: on.then_some(cur.row),
            caret: (on && focused).then_some(cur),
            sel: sel.filter(|_| on),
            marks,
            marked: if marks > 0 {
                app.split_marked_rows(side)
            } else {
                Vec::new()
            },
            gutter: Box::new(move |r: &DiffRow| {
                let n = diff_line(app, r).and_then(|l| match side {
                    Side::Old => l.old,
                    Side::New => l.new,
                });
                match n {
                    Some(n) => (format!("{n:>num_w$} "), Some(sign_of(r.kind))),
                    None => (" ".repeat(side_gutter_width(num_w)), None),
                }
            }),
            gutter_w: side_gutter_width(num_w),
            side: Some(side),
        }
    };
    let (left, right) = (pane(Side::Old), pane(Side::New));
    let (lrect, rrect) = app.split_panes(area);
    let across = if mine == Side::Old { &left } else { &right };
    let scroll = app.commit_scroll().0;
    for (y, i) in (scroll..left.rows.len())
        .take(area.height as usize)
        .enumerate()
    {
        let y = area.y + y as u16;
        let r = &left.rows[i];
        if spans_both(r.kind) {
            let rect = Rect::new(area.x, y, area.width, 1);
            draw_row(frame, app, across, i, r, rect);
            continue;
        }
        draw_row(
            frame,
            app,
            &left,
            i,
            r,
            Rect::new(lrect.x, y, lrect.width, 1),
        );
        if let Some(r) = right.rows.get(i) {
            draw_row(
                frame,
                app,
                &right,
                i,
                r,
                Rect::new(rrect.x, y, rrect.width, 1),
            );
        }
        if rrect.x > lrect.right() {
            frame.render_widget(
                Paragraph::new("│").style(Style::new().fg(palette::SURFACE)),
                Rect::new(lrect.right(), y, 1, 1),
            );
        }
    }
}
