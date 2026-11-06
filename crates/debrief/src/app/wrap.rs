//! Line wrapping in the unified diff (`[view] wrap`, `<leader>w`). A row
//! longer than the text columns breaks into screen lines at grapheme
//! starts, never inside one; continuation lines get a blank gutter. The
//! cursor stays a (row, display column); `scroll` is the first screen line
//! drawn, so the view can start inside a tall row and reach all of it.
//! Side by side never wraps.
//!
//! `lines` holds each row's first screen line (and the total last); it is
//! empty when the view does not wrap, and every lookup is then the
//! identity (a row is a line), so the scroll arithmetic in `motion.rs`
//! and the code that keeps the cursor's place on screen are the same code
//! either way.

use super::App;
use super::commit::CommitView;
use super::motion::Cell;

/// The start column of each screen line `cells` wraps into at `w` text
/// columns: a cell that would end past the line's last column starts the
/// next one. A cell wider than `w` sits alone on its line (and is cut
/// there).
pub(super) fn breaks(cells: &[Cell], w: usize) -> Vec<usize> {
    let w = w.max(1);
    let mut out = vec![0];
    let mut start = 0;
    for c in cells {
        if c.col + c.width > start + w && c.col > start {
            start = c.col;
            out.push(start);
        }
    }
    out
}

/// How many screen lines [`breaks`] gives.
fn height(cells: &[Cell], w: usize) -> usize {
    match cells.last() {
        Some(c) if c.col + c.width > w.max(1) => breaks(cells, w).len(),
        _ => 1,
    }
}

/// A place for the cursor (see [`CommitView::nearest_place`]): a row, and
/// for a row taller than the view, the line of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Place {
    pub row: usize,
    pub line: Option<usize>,
}

impl CommitView {
    /// Rows wrap: wrapping is on and the layout is unified.
    pub(super) fn wrapping(&self) -> bool {
        self.wrap && !self.is_split()
    }

    /// Rebuild `lines` for the current rows, text width and layout when
    /// they changed (`lines_w` is the width they were built for). The view
    /// keeps its top row, and its place inside it.
    pub(super) fn relines(&mut self) {
        let w = self.text_w;
        let fresh = if self.wrapping() {
            self.lines_w == Some(w) && self.lines.len() == self.rows.len() + 1
        } else {
            self.lines.is_empty()
        };
        if fresh {
            return;
        }
        let top = self.row_at(self.scroll);
        let into = self.scroll - self.line(top).min(self.scroll);
        self.lines.clear();
        self.lines_w = None;
        if self.wrapping() {
            self.lines.reserve(self.rows.len() + 1);
            let mut at = 0;
            for r in 0..self.rows.len() {
                self.lines.push(at);
                at += height(self.cells(r), w);
            }
            self.lines.push(at);
            self.lines_w = Some(w);
        }
        let top = top.min(self.last_row());
        self.scroll = self.line(top) + into.min(self.height_of(top) - 1);
    }

    /// Screen lines in all rows.
    pub(super) fn total_lines(&self) -> usize {
        self.lines.last().copied().unwrap_or(self.rows.len())
    }

    /// The first screen line of `row`; past the last row, one line a row.
    pub(super) fn line(&self, row: usize) -> usize {
        match self.lines.get(row) {
            Some(&l) => l,
            None if self.lines.is_empty() => row,
            None => self.total_lines() + row.saturating_sub(self.rows.len()),
        }
    }

    /// Screen lines `row` takes (at least 1).
    pub(super) fn height_of(&self, row: usize) -> usize {
        (self.line(row + 1) - self.line(row)).max(1)
    }

    /// The row screen line `l` is on; past the last row, one line a row.
    pub(super) fn row_at(&self, l: usize) -> usize {
        if self.lines.is_empty() {
            return l;
        }
        let total = self.total_lines();
        if l >= total {
            return self.rows.len() + (l - total);
        }
        self.lines.partition_point(|&x| x <= l) - 1
    }

    /// The start column of each screen line of `row`: [`breaks`] when
    /// wrapping, else one line.
    pub fn segments(&self, row: usize) -> Vec<usize> {
        if self.wrapping() {
            breaks(self.cells(row), self.text_w)
        } else {
            vec![0]
        }
    }

    /// The screen line the cursor is on.
    pub(super) fn cursor_line(&self) -> usize {
        let segs = self.segments(self.cur.row);
        let k = segs
            .partition_point(|&s| s <= self.cur.col)
            .saturating_sub(1);
        self.line(self.cur.row) + k
    }

    /// The last screen line on screen.
    pub(super) fn last_visible_line(&self) -> usize {
        (self.scroll + self.view_h.max(1))
            .min(self.total_lines())
            .saturating_sub(1)
    }

    /// Where a drag past the top (`top`) or bottom edge selects to: the
    /// place on screen nearest that edge (see [`Self::nearest_place`]),
    /// else the row on the edge's line; and the line it is on.
    pub(super) fn edge_place(&self, top: bool) -> (usize, usize) {
        let l = if top {
            self.scroll
        } else {
            self.last_visible_line()
        };
        match self.nearest_place(l, None) {
            Some(Place { row, line }) => (row, line.unwrap_or(l)),
            None => (self.row_at(l).min(self.last_row()), l),
        }
    }

    /// In a cursor row taller than the view, the cursor to the start of
    /// its screen line `l` (when `l` is in it).
    pub(super) fn onto_line(&mut self, l: usize) {
        let row = self.cur.row;
        if self.height_of(row) <= self.view_h.max(1) || self.row_at(l) != row {
            return;
        }
        let segs = self.segments(row);
        let k = (l - self.line(row)).min(segs.len() - 1);
        self.set_col(segs[k]);
    }

    /// The cursor is where [`Self::keep_visible`] leaves the view alone:
    /// its whole row on screen, or for a row taller than the view, its
    /// line.
    pub(super) fn cursor_shown(&self) -> bool {
        let (top, vh) = (self.scroll, self.view_h.max(1));
        let (start, h) = (self.line(self.cur.row), self.height_of(self.cur.row));
        let (lo, hi) = if h <= vh {
            (start, start + h)
        } else {
            let l = self.cursor_line();
            (l, l + 1)
        };
        lo >= top && hi <= top + vh
    }

    /// The place on screen nearest screen line `target` (above it on a
    /// tie) that [`Self::keep_visible`] accepts: a row wholly on screen,
    /// or a line on screen of a row taller than the view. With `avoid`
    /// (lines `lo..hi`, the comment box), only places off those lines.
    /// `None` when there is none.
    pub(super) fn nearest_place(
        &self,
        target: usize,
        avoid: Option<(usize, usize)>,
    ) -> Option<Place> {
        let (top, vh) = (self.scroll, self.view_h.max(1));
        let end = (top + vh).min(self.total_lines());
        let off = |a: usize, b: usize| avoid.is_none_or(|(lo, hi)| b <= lo || a >= hi);
        let dist = |a: usize, b: usize| {
            if target < a {
                a - target
            } else if target >= b {
                target + 1 - b
            } else {
                0
            }
        };
        // (distance, first line, place).
        let mut best: Option<(usize, usize, Place)> = None;
        let mut pick = |d: usize, a: usize, p: Place| {
            if best.is_none_or(|b| (d, a) < (b.0, b.1)) {
                best = Some((d, a, p));
            }
        };
        let mut row = self.row_at(top);
        while row <= self.last_row() && self.line(row) < end {
            let (a, h) = (self.line(row), self.height_of(row));
            if h > vh {
                // A tall row: its lines on screen (off `avoid`), the one
                // nearest the target.
                let (lo, hi) = (a.max(top), (a + h).min(end));
                let edges = avoid.map_or([None, None], |(l, h)| [l.checked_sub(1), Some(h)]);
                let near = [Some(target.clamp(lo, hi - 1))]
                    .into_iter()
                    .chain(edges)
                    .flatten()
                    .filter(|&l| l >= lo && l < hi && off(l, l + 1))
                    .min_by_key(|&l| (dist(l, l + 1), l));
                if let Some(l) = near {
                    pick(dist(l, l + 1), l, Place { row, line: Some(l) });
                }
            } else if a >= top && a + h <= end && off(a, a + h) {
                pick(dist(a, a + h), a, Place { row, line: None });
            }
            row += 1;
        }
        best.map(|b| b.2)
    }

    /// The cursor to `p`: its row at the desired column, or the line of a
    /// tall row at the cursor's column on screen.
    pub(super) fn go_to_place(&mut self, p: Place) {
        match p.line {
            Some(l) if l != self.cursor_line() => self.move_to_line(l),
            None if p.row != self.cur.row => self.move_to_row(p.row),
            _ => {}
        }
    }

    /// `row`'s first line is on screen.
    pub(super) fn row_shown(&self, row: usize) -> bool {
        let l = self.line(row);
        l >= self.scroll && l < self.scroll + self.view_h.max(1)
    }

    /// The cursor to screen line `l`: that line's row at the desired
    /// column. Within its own row, or into a row taller than the view, on
    /// line `l` itself at the cursor's column on screen (as vim's `gj`), so
    /// a move by lines also moves through a tall row.
    pub(super) fn move_to_line(&mut self, l: usize) {
        let row = self.row_at(l).min(self.last_row());
        if !self.wrapping() || (row != self.cur.row && self.height_of(row) <= self.view_h.max(1)) {
            return self.move_to_row(row);
        }
        let here = self.segments(self.cur.row);
        let k = here
            .partition_point(|&s| s <= self.cur.col)
            .saturating_sub(1);
        let on_screen = self.cur.col - here[k];
        let segs = self.segments(row);
        let k = l.saturating_sub(self.line(row)).min(segs.len() - 1);
        let mut col = segs[k] + on_screen;
        if let Some(&next) = segs.get(k + 1) {
            col = col.min(next - 1);
        }
        self.cur.row = row;
        self.set_col(col);
    }

    /// `H` `M` `L`: the place on screen nearest line `l` (see
    /// [`Self::nearest_place`], so the view does not move): a row at
    /// column 0, or a tall row's line at its start.
    pub(super) fn goto_screen_line(&mut self, l: usize) {
        match self.nearest_place(l, None) {
            Some(Place { row, line: None }) => self.goto_row(row),
            Some(Place { row, line: Some(l) }) => {
                self.cur.row = row;
                let segs = self.segments(row);
                let k = (l - self.line(row)).min(segs.len() - 1);
                self.set_col(segs[k]);
            }
            None => self.goto_row(self.row_at(l).min(self.last_row())),
        }
    }

    /// The comment box covers screen lines `lo..hi`: the cursor to the
    /// nearest place on screen off it (see [`Self::nearest_place`]), else
    /// the nearest place under it, else into the view.
    pub(super) fn keep_off_box(&mut self, lo: usize, hi: usize) {
        let cur = self.cursor_line();
        match self
            .nearest_place(cur, Some((lo, hi)))
            .or_else(|| self.nearest_place(cur, None))
        {
            Some(p) => self.go_to_place(p),
            None => {
                let l = cur.clamp(self.scroll, self.scroll + self.view_h.max(1) - 1);
                if l != cur {
                    self.move_to_line(l);
                }
            }
        }
    }

    /// Screen lines between the view's top and the cursor's row's first
    /// line: what a rebuild of the rows keeps.
    pub(super) fn screen_offset(&self) -> usize {
        self.line(self.cur.row).saturating_sub(self.scroll)
    }

    /// Scroll so the cursor's row starts `off` lines below the view's top.
    pub(super) fn set_screen_offset(&mut self, off: usize) {
        self.scroll = self
            .line(self.cur.row)
            .saturating_sub(off)
            .min(self.max_scroll());
    }

    /// The row on screen line `dy` (from the pane's top), the start column
    /// of that line within it (0 past the last row), and the next line's
    /// start when the row goes on.
    pub(super) fn screen_hit(&self, dy: usize) -> (usize, usize, Option<usize>) {
        let l = self.scroll + dy;
        let row = self.row_at(l);
        let segs = self.segments(row);
        let k = l.saturating_sub(self.line(row));
        let from = segs.get(k).copied().unwrap_or(0);
        (row, from, segs.get(k + 1).copied())
    }
}

impl App {
    /// `<leader>w`: wrap long rows, or scroll them sideways.
    pub(super) fn toggle_wrap(&mut self) {
        self.wrap = !self.wrap;
        self.set_status(match (self.wrap, self.commit_split()) {
            (true, false) => "wrap",
            (true, true) => "wrap (unified only)",
            (false, _) => "no wrap",
        });
    }

    /// The diff's rows wrap.
    pub fn commit_wraps(&self) -> bool {
        self.commit.as_ref().is_some_and(CommitView::wrapping)
    }

    /// The start column of each screen line of diff row `row`.
    pub fn commit_segments(&self, row: usize) -> Vec<usize> {
        self.commit
            .as_ref()
            .map_or_else(|| vec![0], |v| v.segments(row))
    }

    /// The first row drawn, and how many of its screen lines are above
    /// the view.
    pub fn commit_top(&self) -> (usize, usize) {
        self.commit.as_ref().map_or((0, 0), |v| {
            let row = v.row_at(v.scroll);
            (row, v.scroll - v.line(row).min(v.scroll))
        })
    }

    /// Screen lines in the diff's rows (its row count unless wrapping).
    pub fn commit_total_lines(&self) -> usize {
        self.commit.as_ref().map_or(0, CommitView::total_lines)
    }

    /// The screen lines (from the diff pane's top) of rows `lo..=hi`;
    /// they may lie outside the pane.
    pub(super) fn commit_screen_span(&self, lo: usize, hi: usize) -> (i64, i64) {
        let Some(v) = &self.commit else {
            return (lo as i64, hi as i64);
        };
        let top = v.scroll as i64;
        (v.line(lo) as i64 - top, v.line(hi + 1) as i64 - 1 - top)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::motion::row_cells;

    #[test]
    fn breaks_at_grapheme_starts() {
        assert_eq!(breaks(&row_cells("abcdefg"), 3), [0, 3, 6]);
        assert_eq!(breaks(&row_cells("abc"), 3), [0]);
        assert_eq!(breaks(&row_cells(""), 3), [0]);
        // A wide grapheme that would straddle the edge starts the next line.
        assert_eq!(breaks(&row_cells("ab日c"), 3), [0, 2]);
        // A tab wider than the line sits alone on it.
        assert_eq!(breaks(&row_cells("\tx"), 3), [0, 4]);
    }

    #[test]
    fn height_matches_breaks() {
        for (s, w) in [
            ("abcdefg", 3),
            ("abc", 3),
            ("", 2),
            ("ab日c", 3),
            ("日日日", 1),
        ] {
            let c = row_cells(s);
            assert_eq!(height(&c, w), breaks(&c, w).len(), "{s:?} at {w}");
        }
    }
}
