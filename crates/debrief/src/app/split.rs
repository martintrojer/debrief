//! Side by side (spec §3 "Side by side", Q30): a second row model built
//! from the same [`CommitDiff`], old lines on the left, new on the right.
//!
//! Both sides have one row per screen row. Message, file header, hunk
//! header, placeholder and blank rows are the same on both sides (drawn
//! across the pane); context lines appear on both; in a change block (a
//! run of removed then added lines, as git emits them) removed `i` pairs
//! with added `i` and the shorter side gets [`RowKind::Filler`] rows,
//! whose `line` is the paired line on the other side. A no-newline row
//! follows its line on its side.
//!
//! The cursor is on one side: [`CommitView::rows`] and `cells` hold that
//! side, [`Split`] the other, and `Tab` swaps them. So motions, visual
//! mode, comments, markers, `]r`, `K`, `<leader>o` and search act on the
//! cursor's side with the unified code: a left row is a removed or
//! context line (old numbers), a right row an added or context line (new
//! numbers).

use debrief_review::Side;
use ratatui::layout::Rect;

use super::commit::{CommitView, Row, RowKind, flatten};
use super::motion::{Cell, row_cells};
use super::{App, Mode};
use crate::vcs::{CommitDiff, DiffLine, LineKind};

/// The status when `t` finds the diff pane too narrow.
pub fn too_narrow(min: u16, have: u16) -> String {
    format!("side by side needs {min} columns (have {have})")
}

/// The side the cursor is not on.
pub(super) fn other(side: Side) -> Side {
    match side {
        Side::Old => Side::New,
        Side::New => Side::Old,
    }
}

/// The side of the layout the cursor is not on.
#[derive(Debug, Clone, Default)]
pub(super) struct Split {
    pub(super) rows: Vec<Row>,
    pub(super) cells: Vec<Vec<Cell>>,
    /// Its first text column drawn.
    pub(super) hscroll: usize,
}

/// The side by side rows of `diff`: (left, right), the same length.
pub fn flatten_split(diff: &CommitDiff) -> (Vec<Row>, Vec<Row>) {
    let unified = flatten(diff);
    let (mut left, mut right) = (Vec::new(), Vec::new());
    for r in unified {
        match r.kind {
            RowKind::Line(_) | RowKind::NoNewline | RowKind::Filler => continue,
            RowKind::HunkHeader => {
                let (Some(f), Some(h)) = (r.file, r.hunk) else {
                    continue;
                };
                left.push(r.clone());
                right.push(r);
                pair_hunk(&diff.files[f].hunks[h].lines, f, h, &mut left, &mut right);
            }
            _ => {
                left.push(r.clone());
                right.push(r);
            }
        }
    }
    (left, right)
}

/// Rows of one hunk's lines, paired.
fn pair_hunk(lines: &[DiffLine], f: usize, h: usize, left: &mut Vec<Row>, right: &mut Vec<Row>) {
    let row = |kind, line: usize, text: &str| Row {
        kind,
        file: Some(f),
        hunk: Some(h),
        line: Some(line),
        text: text.to_string(),
    };
    let side = |own: Option<usize>, paired: Option<usize>| match own {
        Some(i) => row(RowKind::Line(lines[i].kind), i, &lines[i].text),
        None => row(RowKind::Filler, paired.unwrap_or(0), ""),
    };
    let mut push = |o: Option<usize>, n: Option<usize>| {
        left.push(side(o, n));
        right.push(side(n, o));
        let nonl = |i: Option<usize>| i.filter(|&i| lines[i].no_newline);
        let (lo, rn) = (nonl(o), nonl(n));
        if lo.is_some() || rn.is_some() {
            let mark = |own: Option<usize>, at: Option<usize>| match own {
                Some(i) => row(RowKind::NoNewline, i, super::commit::NO_NEWLINE),
                None => row(RowKind::Filler, at.unwrap_or(0), ""),
            };
            left.push(mark(lo, o.or(n)));
            right.push(mark(rn, n.or(o)));
        }
    };
    let mut i = 0;
    while i < lines.len() {
        if lines[i].kind == LineKind::Context {
            push(Some(i), Some(i));
            i += 1;
            continue;
        }
        let start = i;
        while i < lines.len() && lines[i].kind == LineKind::Removed {
            i += 1;
        }
        let removed = start..i;
        let start = i;
        while i < lines.len() && lines[i].kind == LineKind::Added {
            i += 1;
        }
        let added = start..i;
        for k in 0..removed.len().max(added.len()) {
            let o = (k < removed.len()).then(|| removed.start + k);
            let n = (k < added.len()).then(|| added.start + k);
            push(o, n);
        }
    }
}

/// What a row stands for, to find it again in the other layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RowRef {
    /// A message row (the same index in both layouts).
    Index(usize),
    /// A header or placeholder row of `file` (and `hunk`).
    Kind(RowKind, Option<usize>, Option<usize>),
    /// A diff line (or filler: its paired line), and the side it prefers.
    Line(usize, usize, usize, Side),
    /// A no-newline row of a line.
    NoNewline(usize, usize, usize),
}

fn row_ref(rows: &[Row], i: usize, side: Option<Side>) -> RowRef {
    let Some(r) = rows.get(i) else {
        return RowRef::Index(i);
    };
    match (r.kind, r.file, r.hunk, r.line) {
        (RowKind::Line(k), Some(f), Some(h), Some(l)) => {
            let prefer = match (side, k) {
                (Some(s), _) => s,
                (None, LineKind::Removed) => Side::Old,
                (None, _) => Side::New,
            };
            RowRef::Line(f, h, l, prefer)
        }
        (RowKind::Filler, Some(f), Some(h), Some(l)) => {
            RowRef::Line(f, h, l, other(side.unwrap_or(Side::New)))
        }
        (RowKind::NoNewline, Some(f), Some(h), Some(l)) => RowRef::NoNewline(f, h, l),
        (RowKind::Message, ..) => RowRef::Index(i),
        // A blank row sits just above the row after it.
        (RowKind::Blank, ..) => match row_ref(rows, i + 1, side) {
            RowRef::Kind(k, f, h) => RowRef::Kind(k, f, h),
            _ => RowRef::Index(i),
        },
        (k, f, h, _) => RowRef::Kind(k, f, h),
    }
}

/// Whether row `r` is `want` (a Blank stands for the row after it).
fn matches(r: &Row, want: RowRef) -> bool {
    match want {
        RowRef::Index(_) => false,
        RowRef::Kind(k, f, h) => r.kind == k && r.file == f && r.hunk == h,
        RowRef::Line(f, h, l, _) => {
            matches!(r.kind, RowKind::Line(_))
                && (r.file, r.hunk, r.line) == (Some(f), Some(h), Some(l))
        }
        RowRef::NoNewline(f, h, l) => {
            r.kind == RowKind::NoNewline && (r.file, r.hunk, r.line) == (Some(f), Some(h), Some(l))
        }
    }
}

impl CommitView {
    /// Side by side is the layout (wanted, before the diff loads).
    pub(super) fn is_split(&self) -> bool {
        self.split_on
    }

    /// The cursor's side in side by side.
    pub(super) fn split_side(&self) -> Option<Side> {
        self.split_on.then_some(self.side)
    }

    /// The rows of `diff` in the current layout, the cursor's side in
    /// `rows`.
    pub(super) fn build_rows(&mut self, diff: &CommitDiff) {
        if self.split_on {
            let (left, right) = flatten_split(diff);
            let (mine, theirs) = match self.side {
                Side::Old => (left, right),
                Side::New => (right, left),
            };
            self.rows = mine;
            self.split = Some(Split {
                cells: theirs.iter().map(|r| row_cells(&r.text)).collect(),
                rows: theirs,
                hscroll: 0,
            });
        } else {
            self.rows = flatten(diff);
            self.split = None;
        }
        self.cells = self.rows.iter().map(|r| row_cells(&r.text)).collect();
    }

    /// Put the cursor on the other side (same row), its rows, cells,
    /// horizontal scroll and search hits with it.
    pub(super) fn swap_side(&mut self) {
        let Some(s) = &mut self.split else { return };
        std::mem::swap(&mut self.rows, &mut s.rows);
        std::mem::swap(&mut self.cells, &mut s.cells);
        std::mem::swap(&mut self.hscroll, &mut s.hscroll);
        self.search.swap_sides();
        self.side = other(self.side);
        self.clamp_cursor();
    }

    /// `Tab`: the other side, on the same row; on a filler row, the
    /// nearest row of that side in the block that is not one (above on
    /// a tie).
    pub(super) fn switch_side(&mut self) {
        let Some(s) = &self.split else { return };
        let row = self.cur.row;
        let in_block = |r: usize| {
            matches!(
                s.rows.get(r).map(|r| r.kind),
                Some(RowKind::Line(_) | RowKind::Filler | RowKind::NoNewline)
            )
        };
        let filler = |r: usize| s.rows.get(r).map(|r| r.kind) == Some(RowKind::Filler);
        let mut to = row;
        if filler(row) {
            let mut up = None;
            let mut r = row;
            while r > 0 && in_block(r - 1) {
                r -= 1;
                if !filler(r) {
                    up = Some(r);
                    break;
                }
            }
            let mut down = None;
            let mut r = row;
            while in_block(r + 1) {
                r += 1;
                if !filler(r) {
                    down = Some(r);
                    break;
                }
            }
            to = match (up, down) {
                (Some(u), Some(d)) => {
                    if row - u <= d - row {
                        u
                    } else {
                        d
                    }
                }
                (u, d) => u.or(d).unwrap_or(row),
            };
        }
        self.swap_side();
        self.cur.row = to;
        self.clamp_cursor();
    }

    /// Switch layouts, the cursor kept on its line (and its row on
    /// screen): unified -> side by side puts it on the side the line
    /// belongs to (new, unless the line has no new side).
    pub(super) fn relayout(&mut self, split: bool) {
        if split == self.split_on {
            return;
        }
        let from = row_ref(&self.rows, self.cur.row, self.split_side());
        let on_screen = self.cur.row.saturating_sub(self.scroll);
        self.split_on = split;
        self.side = Side::New;
        let Some(diff) = self.diff.take() else {
            return;
        };
        self.build_rows(&diff);
        self.diff = Some(diff);
        self.hscroll = 0;
        let (row, side) = self.find_ref(from);
        if split && side != self.side {
            self.swap_side();
        }
        self.goto_row(row);
        self.scroll = self
            .cur
            .row
            .saturating_sub(on_screen)
            .min(self.max_scroll());
    }

    /// The row (and side) of `want` in the current layout.
    fn find_ref(&self, want: RowRef) -> (usize, Side) {
        if let RowRef::Index(i) = want {
            return (i, Side::New);
        }
        let prefer = match want {
            RowRef::Line(.., s) => s,
            _ => Side::New,
        };
        // `rows` is the new side here (relayout starts there); unified
        // rows are both sides.
        let theirs = match &self.split {
            Some(s) => Some(&s.rows),
            None => Some(&self.rows),
        };
        let sides = [(Side::New, Some(&self.rows)), (Side::Old, theirs)];
        let found = |s: Side| {
            let rows = sides.iter().find(|(x, _)| *x == s)?.1?;
            rows.iter().position(|r| matches(r, want))
        };
        if let Some(r) = found(prefer) {
            return (r, prefer);
        }
        if let Some(r) = found(other(prefer)) {
            return (r, other(prefer));
        }
        (0, Side::New)
    }
}

impl App {
    /// The session's layout is side by side (`t`), whether or not the pane
    /// is wide enough to show it.
    pub fn split_wanted(&self) -> bool {
        self.split
    }

    /// The open commit shows side by side.
    pub fn commit_split(&self) -> bool {
        self.commit.as_ref().is_some_and(CommitView::is_split)
    }

    /// The cursor's side when side by side.
    pub fn split_side(&self) -> Option<Side> {
        self.commit.as_ref()?.split_side()
    }

    /// The rows of one side (empty when not side by side).
    pub fn split_rows(&self, side: Side) -> &[Row] {
        let Some(v) = self.commit.as_ref().filter(|v| v.is_split()) else {
            return &[];
        };
        if v.side == side {
            &v.rows
        } else {
            v.split.as_ref().map_or(&[], |s| &s.rows)
        }
    }

    /// The first text column drawn on one side.
    pub fn split_hscroll(&self, side: Side) -> usize {
        let Some(v) = &self.commit else { return 0 };
        if v.side == side {
            v.hscroll
        } else {
            v.split.as_ref().map_or(0, |s| s.hscroll)
        }
    }

    /// The left and right panes of the diff pane `diff` (a divider
    /// column between them).
    pub fn split_panes(&self, diff: Rect) -> (Rect, Rect) {
        let lw = diff.width.saturating_sub(1) / 2;
        let left = Rect::new(diff.x, diff.y, lw, diff.height);
        let rx = diff.x + lw + 1.min(diff.width);
        let right = Rect::new(rx, diff.y, diff.width.saturating_sub(lw + 1), diff.height);
        (left, right)
    }

    /// The cursor side's pane in side by side, else `diff` itself.
    pub fn side_pane(&self, diff: Rect) -> Rect {
        let (left, right) = self.split_panes(diff);
        match self.split_side() {
            Some(Side::Old) => left,
            Some(Side::New) => right,
            None => diff,
        }
    }

    /// `t`: unified <-> side by side for the session; side by side needs
    /// a diff pane `[view] split_min_width` wide.
    pub(super) fn toggle_layout(&mut self) {
        let width = self.diff_width();
        let min = self.config.view.split_min_width;
        if !self.split && width < min {
            return self.set_status(too_narrow(min, width));
        }
        self.split = !self.split;
        self.set_status(if self.split {
            "side by side"
        } else {
            "unified"
        });
        self.layout_sync(width);
    }

    /// The diff pane's width on the current terminal.
    pub(super) fn diff_width(&self) -> u16 {
        let area = Rect::new(0, 0, self.size.0, self.size.1.saturating_sub(1));
        self.commit_layout(area).diff.width
    }

    /// Show the session's layout in a diff pane `width` wide: unified
    /// below the minimum (with a status note when that undoes side by
    /// side), side by side again once it is wide enough. Not while a
    /// selection or a comment holds row numbers.
    pub(super) fn layout_sync(&mut self, width: u16) {
        if matches!(self.mode, Mode::Visual | Mode::Comment) {
            return;
        }
        let min = self.config.view.split_min_width;
        let want = self.split && width >= min;
        let Some(v) = &mut self.commit else { return };
        if v.is_split() == want {
            return;
        }
        v.relayout(want);
        if self.split && !want {
            self.set_status(format!("{}; unified for now", too_narrow(min, width)));
        }
        self.refresh_search();
    }

    /// `Tab` in the diff: the cursor to the other side.
    pub(super) fn switch_side(&mut self) {
        match &mut self.commit {
            Some(v) if v.is_split() => v.switch_side(),
            _ => self.set_status("Tab switches sides in side by side (t)"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vcs::{FileDiff, Hunk};

    fn line(kind: LineKind, old: Option<u32>, new: Option<u32>, text: &str) -> DiffLine {
        DiffLine {
            kind,
            old,
            new,
            text: text.into(),
            no_newline: false,
        }
    }

    fn diff(lines: Vec<DiffLine>) -> CommitDiff {
        CommitDiff {
            message: String::new(),
            files: vec![FileDiff {
                old_path: Some("a".into()),
                new_path: Some("a".into()),
                binary: false,
                note: None,
                mode_change: None,
                copied: false,
                hunks: vec![Hunk {
                    header: "@@ -1,4 +1,5 @@".into(),
                    old_start: 1,
                    new_start: 1,
                    lines,
                }],
            }],
        }
    }

    fn show(rows: &[Row]) -> Vec<String> {
        rows.iter()
            .map(|r| match r.kind {
                RowKind::Filler => "~".to_string(),
                RowKind::NoNewline => "\\".to_string(),
                RowKind::FileHeader => "F".to_string(),
                RowKind::HunkHeader => "@".to_string(),
                _ => r.text.clone(),
            })
            .collect()
    }

    #[test]
    fn a_change_block_pairs_removed_with_added_and_pads_the_shorter_side() {
        use LineKind::*;
        let d = diff(vec![
            line(Context, Some(1), Some(1), "c1"),
            line(Removed, Some(2), None, "r1"),
            line(Removed, Some(3), None, "r2"),
            line(Added, None, Some(2), "a1"),
            line(Added, None, Some(3), "a2"),
            line(Added, None, Some(4), "a3"),
            line(Context, Some(4), Some(5), "c2"),
        ]);
        let (l, r) = flatten_split(&d);
        assert_eq!(show(&l), ["F", "@", "c1", "r1", "r2", "~", "c2"]);
        assert_eq!(show(&r), ["F", "@", "c1", "a1", "a2", "a3", "c2"]);
        // The filler points at its paired line.
        assert_eq!(l[5].line, Some(5));
    }

    #[test]
    fn no_newline_rows_attach_to_their_side() {
        use LineKind::*;
        let mut r = line(Removed, Some(1), None, "old");
        r.no_newline = true;
        let d = diff(vec![r, line(Added, None, Some(1), "new")]);
        let (l, r) = flatten_split(&d);
        assert_eq!(show(&l), ["F", "@", "old", "\\"]);
        assert_eq!(show(&r), ["F", "@", "new", "~"]);
    }
}
