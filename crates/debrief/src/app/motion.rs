//! Cursor motions and scrolling in the commit view, over grapheme cells of
//! each row's untruncated text (copied from ramble's motion.rs and
//! scroll.rs). The view follows the cursor both ways: rows scroll
//! vertically, columns horizontally (unless rows wrap; see `wrap.rs`).

use super::commit::{CommitView, RowKind};
use super::display::{self, Glyph};
use crate::vcs::LineKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Class {
    Empty,
    Blank,
    Word,
    Punct,
}

/// One grapheme of a row's text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Cell {
    pub col: usize,
    pub width: usize,
    pub class: Class,
}

/// Cursor position: row, and the display column of a grapheme's start.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Cursor {
    pub row: usize,
    pub col: usize,
}

/// A word-motion position: row plus grapheme index (0 on an empty row).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Pos {
    row: usize,
    idx: usize,
}

/// Grapheme cells of `text`, laid out by [`display::cells`] (tabs to the
/// next stop, a trailing `\r` hidden, zero-width graphemes skipped).
pub(super) fn row_cells(text: &str) -> Vec<Cell> {
    display::cells(text)
        .into_iter()
        .map(|c| {
            let class = match c.glyph {
                Glyph::Tab => Class::Blank,
                Glyph::Control => Class::Punct,
                Glyph::Text(g) => {
                    let ch = g.chars().next().unwrap_or(' ');
                    if ch.is_whitespace() {
                        Class::Blank
                    } else if ch.is_alphanumeric() || ch == '_' {
                        Class::Word
                    } else {
                        Class::Punct
                    }
                }
            };
            Cell {
                col: c.col,
                width: c.width,
                class,
            }
        })
        .collect()
}

impl CommitView {
    pub(super) fn total_rows(&self) -> usize {
        self.rows.len()
    }

    pub(super) fn last_row(&self) -> usize {
        self.rows.len().saturating_sub(1)
    }

    pub(super) fn cells(&self, row: usize) -> &[Cell] {
        self.cells.get(row).map_or(&[], Vec::as_slice)
    }

    fn cell_index(&self, row: usize, col: usize) -> usize {
        self.cells(row)
            .iter()
            .rposition(|c| c.col <= col)
            .unwrap_or(0)
    }

    /// Vertical move to `row`, keeping the desired column.
    pub(super) fn move_to_row(&mut self, row: usize) {
        self.cur.row = row.min(self.last_row());
        self.clamp_cursor();
    }

    /// Jump to `row` at column 0 (gg, G, H, M, L, ]f, ...).
    pub(super) fn goto_row(&mut self, row: usize) {
        self.cur.row = row.min(self.last_row());
        self.set_col(0);
    }

    pub(super) fn vertical(&mut self, delta: isize) {
        let row = self.cur.row.saturating_add_signed(delta);
        self.move_to_row(row);
    }

    pub(super) fn horizontal(&mut self, delta: isize) {
        let cells = self.cells(self.cur.row);
        if cells.is_empty() {
            return;
        }
        let idx = self.cell_index(self.cur.row, self.cur.col);
        let new = idx
            .saturating_add_signed(delta)
            .min(cells.len().saturating_sub(1));
        self.set_col(cells[new].col);
    }

    pub(super) fn set_col(&mut self, col: usize) {
        self.want_col = col;
        self.clamp_cursor();
        self.want_col = self.cur.col;
    }

    /// `$`: end of the line `n - 1` rows down.
    pub(super) fn line_end(&mut self, n: usize) {
        self.vertical(n as isize - 1);
        self.want_col = usize::MAX;
        self.clamp_cursor();
    }

    /// Put the cursor column on the grapheme under `want_col`.
    pub(super) fn clamp_cursor(&mut self) {
        self.cur.row = self.cur.row.min(self.last_row());
        let cells = self.cells(self.cur.row);
        self.cur.col = match cells.iter().rposition(|c| c.col <= self.want_col) {
            Some(i) => cells[i].col,
            None => 0,
        };
    }

    pub(super) fn repeat(&mut self, n: usize, f: fn(&Self, Pos) -> Pos) {
        let mut p = self.pos();
        for _ in 0..n {
            p = f(self, p);
        }
        self.set_pos(p);
    }

    fn pos(&self) -> Pos {
        Pos {
            row: self.cur.row,
            idx: self.cell_index(self.cur.row, self.cur.col),
        }
    }

    fn set_pos(&mut self, p: Pos) {
        self.cur.row = p.row.min(self.last_row());
        let col = self.cells(self.cur.row).get(p.idx).map_or(0, |c| c.col);
        self.set_col(col);
    }

    fn class(&self, p: Pos) -> Class {
        self.cells(p.row)
            .get(p.idx)
            .map_or(Class::Empty, |c| c.class)
    }

    fn next(&self, p: Pos) -> Option<Pos> {
        if p.idx + 1 < self.cells(p.row).len() {
            Some(Pos {
                row: p.row,
                idx: p.idx + 1,
            })
        } else if p.row + 1 < self.total_rows() {
            Some(Pos {
                row: p.row + 1,
                idx: 0,
            })
        } else {
            None
        }
    }

    fn prev(&self, p: Pos) -> Option<Pos> {
        if p.idx > 0 {
            Some(Pos {
                row: p.row,
                idx: p.idx - 1,
            })
        } else if p.row > 0 {
            let row = p.row - 1;
            Some(Pos {
                row,
                idx: self.cells(row).len().saturating_sub(1),
            })
        } else {
            None
        }
    }

    /// `w`: start of the next word; an empty row counts as a word.
    pub(super) fn word_forward(&self, p: Pos) -> Pos {
        let start = self.class(p);
        let mut q = p;
        if matches!(start, Class::Word | Class::Punct) {
            loop {
                let Some(n) = self.next(q) else { return q };
                let crossed = n.row != q.row;
                q = n;
                if crossed || self.class(q) != start {
                    break;
                }
            }
        } else {
            match self.next(q) {
                Some(n) => q = n,
                None => return q,
            }
        }
        while self.class(q) == Class::Blank {
            match self.next(q) {
                Some(n) => q = n,
                None => return q,
            }
        }
        q
    }

    /// `e`: end of the current or next word.
    pub(super) fn word_end(&self, p: Pos) -> Pos {
        let Some(mut q) = self.next(p) else { return p };
        while matches!(self.class(q), Class::Blank | Class::Empty) {
            match self.next(q) {
                Some(n) => q = n,
                None => return q,
            }
        }
        let c = self.class(q);
        while let Some(n) = self.next(q) {
            if n.row != q.row || self.class(n) != c {
                break;
            }
            q = n;
        }
        q
    }

    /// `b`: start of the current or previous word.
    pub(super) fn word_backward(&self, p: Pos) -> Pos {
        let Some(mut q) = self.prev(p) else { return p };
        while self.class(q) == Class::Blank {
            match self.prev(q) {
                Some(n) => q = n,
                None => return q,
            }
        }
        let c = self.class(q);
        if c == Class::Empty {
            return q;
        }
        while let Some(n) = self.prev(q) {
            if n.row != q.row || self.class(n) != c {
                break;
            }
            q = n;
        }
        q
    }

    fn blank_row(&self, row: usize) -> bool {
        let filler = self.rows.get(row).map(|r| r.kind) == Some(RowKind::Filler);
        !filler && self.cells(row).iter().all(|c| c.class == Class::Blank)
    }

    /// `}`: the next blank row after the current paragraph (or the last row).
    pub(super) fn paragraph_forward(&self, p: Pos) -> Pos {
        let n = self.total_rows();
        let mut r = p.row;
        while r < n && self.blank_row(r) {
            r += 1;
        }
        while r < n && !self.blank_row(r) {
            r += 1;
        }
        Pos {
            row: r.min(self.last_row()),
            idx: 0,
        }
    }

    /// `{`: the previous blank row before the current paragraph (or row 0).
    pub(super) fn paragraph_backward(&self, p: Pos) -> Pos {
        let mut r = p.row;
        while r > 0 && self.blank_row(r) {
            r -= 1;
        }
        while r > 0 && !self.blank_row(r) {
            r -= 1;
        }
        Pos { row: r, idx: 0 }
    }

    /// `]f` `[f` `]c` `[c`: the `n`th row of `kind` after (or before) the
    /// cursor; stays put when there is none. Returns whether it moved.
    pub(super) fn jump_kind(&mut self, kind: RowKind, forward: bool, n: usize) -> bool {
        let mut row = self.cur.row;
        let mut moved = false;
        for _ in 0..n.max(1) {
            let found = if forward {
                (row + 1..self.rows.len()).find(|&r| self.rows[r].kind == kind)
            } else {
                (0..row).rev().find(|&r| self.rows[r].kind == kind)
            };
            match found {
                Some(r) => {
                    row = r;
                    moved = true;
                }
                None => break,
            }
        }
        if moved {
            self.goto_row(row);
        }
        moved
    }

    /// `]c` `[c` in the full file: the first row of the `n`th change
    /// block (a run of added and removed lines) after (or before) the
    /// cursor's; stays put when there is none. Returns whether it moved.
    pub(super) fn jump_change(&mut self, forward: bool, n: usize) -> bool {
        let edit = |r: usize| {
            matches!(
                self.rows[r].kind,
                RowKind::Line(LineKind::Added | LineKind::Removed) | RowKind::Filler
            )
        };
        // A no-newline row belongs to the line above it.
        let changed =
            |r: usize| edit(r) || (self.rows[r].kind == RowKind::NoNewline && r > 0 && edit(r - 1));
        // A block starts at a changed row whose previous row is not one.
        let start = |r: usize| changed(r) && (r == 0 || !changed(r - 1));
        let mut row = self.cur.row;
        let mut moved = false;
        for _ in 0..n.max(1) {
            let found = if forward {
                (row + 1..self.rows.len()).find(|&r| start(r))
            } else {
                (0..row).rev().find(|&r| start(r))
            };
            match found {
                Some(r) => {
                    row = r;
                    moved = true;
                }
                None => break,
            }
        }
        if moved {
            self.goto_row(row);
        }
        moved
    }

    // Scrolling (ramble's scroll.rs), in screen lines: `scroll` is the
    // first one drawn and the view is `view_h` of them. A wrapped row
    // takes several (see `wrap.rs`); without wrapping a row is a line.

    pub(super) fn max_scroll(&self) -> usize {
        (self.total_lines() + self.overscroll).saturating_sub(self.view_h)
    }

    /// `C-d` (down) / `C-u` (up): the view and the cursor half a page.
    pub(super) fn half_page(&mut self, down: bool) {
        let half = (self.view_h / 2).max(1);
        let cur = self.cursor_line();
        let target = if down {
            (cur + half).min(self.total_lines().saturating_sub(1))
        } else {
            cur.saturating_sub(half)
        };
        self.scroll = if down {
            (self.scroll + half).min(self.max_scroll())
        } else {
            self.scroll.saturating_sub(half)
        };
        self.move_to_line(target);
    }

    /// `C-f`.
    pub(super) fn page_down(&mut self) {
        let new = (self.scroll + self.view_h).min(self.max_scroll());
        if new == self.scroll {
            self.move_to_row(self.last_row());
        } else {
            self.scroll = new;
            self.move_into_view();
        }
    }

    /// `C-b`.
    pub(super) fn page_up(&mut self) {
        let vh = self.view_h;
        let new = self.scroll.saturating_sub(vh);
        if new == self.scroll {
            self.move_to_row(0);
        } else {
            self.scroll = new;
            self.move_into_view();
        }
    }

    /// `C-e`.
    pub(super) fn line_down(&mut self) {
        self.scroll = (self.scroll + 1).min(self.max_scroll());
        self.move_into_view();
    }

    /// `C-y`.
    pub(super) fn line_up(&mut self) {
        self.scroll = self.scroll.saturating_sub(1);
        self.move_into_view();
    }

    /// After the view scrolled, the cursor to the nearest place on screen
    /// [`Self::keep_visible`] accepts, so the view does not scroll back.
    pub(super) fn move_into_view(&mut self) {
        if self.cursor_shown() {
            return;
        }
        let cur = self.cursor_line();
        if let Some(p) = self.nearest_place(cur, None) {
            return self.go_to_place(p);
        }
        // No row fits on screen whole here: the next row the way the view
        // went, which `keep_visible` then shows, so the view still moves
        // on (a jump past the rows that cannot be shown whole).
        let (top, vh) = (self.scroll, self.view_h.max(1));
        let down = self.line(self.cur.row) < top;
        let row = self
            .row_at(if down { top } else { top + vh - 1 })
            .min(self.last_row());
        let row = if down && self.line(row) < top {
            (row + 1).min(self.last_row())
        } else if !down && self.line(row) + self.height_of(row) > top + vh {
            row.saturating_sub(1)
        } else {
            row
        };
        self.move_to_row(row);
    }

    /// The cursor row's first line and its height.
    fn cursor_row_lines(&self) -> (usize, usize) {
        (self.line(self.cur.row), self.height_of(self.cur.row))
    }

    /// `zz`: the cursor's row in the middle; a row taller than the view,
    /// the cursor's line.
    pub(super) fn center_cursor(&mut self) {
        let (start, h) = self.cursor_row_lines();
        let top = if h <= self.view_h {
            start.saturating_sub((self.view_h - h) / 2)
        } else {
            self.cursor_line()
                .saturating_sub((self.view_h.max(1) - 1) / 2)
        };
        self.scroll = top.min(self.max_scroll());
    }

    /// `zt`: the cursor's row at the top (a taller row: the cursor's line).
    pub(super) fn cursor_to_top(&mut self) {
        let (start, h) = self.cursor_row_lines();
        let top = if h <= self.view_h {
            start
        } else {
            self.cursor_line()
        };
        self.scroll = top.min(self.max_scroll());
    }

    /// `zb`: the cursor's row at the bottom (a taller row: the cursor's
    /// line).
    pub(super) fn cursor_to_bottom(&mut self) {
        let (start, h) = self.cursor_row_lines();
        let end = if h <= self.view_h {
            start + h
        } else {
            self.cursor_line() + 1
        };
        self.scroll = end.saturating_sub(self.view_h);
    }

    /// `H`: line `n` from the top of the view.
    pub(super) fn screen_top(&mut self, n: usize) {
        let l = (self.scroll + n - 1).min(self.last_visible_line());
        self.goto_screen_line(l);
    }

    /// `L`: line `n` from the bottom of the view.
    pub(super) fn screen_bottom(&mut self, n: usize) {
        let l = self
            .last_visible_line()
            .saturating_sub(n - 1)
            .max(self.scroll);
        self.goto_screen_line(l);
    }

    /// `M`: middle of the view.
    pub(super) fn screen_middle(&mut self) {
        let last = self.last_visible_line().max(self.scroll);
        self.goto_screen_line(self.scroll + (last - self.scroll) / 2);
    }

    /// Scroll just enough to show the cursor: its whole row when it fits,
    /// else the cursor's line; and unless wrapping, the whole grapheme
    /// under it within the text columns.
    pub(super) fn keep_visible(&mut self) {
        let vh = self.view_h.max(1);
        let (start, h) = self.cursor_row_lines();
        let (lo, hi) = if h <= vh {
            (start, start + h)
        } else {
            let l = self.cursor_line();
            (l, l + 1)
        };
        if lo < self.scroll {
            self.scroll = lo;
        } else if hi > self.scroll + vh {
            self.scroll = hi - vh;
        }
        self.scroll = self.scroll.min(self.max_scroll());
        if self.wrapping() {
            self.hscroll = 0;
            return;
        }
        let w = self.text_w.max(1);
        let cell_w = self
            .cells(self.cur.row)
            .get(self.cell_index(self.cur.row, self.cur.col))
            .map_or(1, |c| c.width);
        let end = self.cur.col + cell_w;
        if self.cur.col < self.hscroll {
            self.hscroll = self.cur.col;
        } else if end > self.hscroll + w {
            self.hscroll = end.saturating_sub(w);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cells_are_grapheme_columns() {
        let c = row_cells("a日e\u{301}x");
        let cols: Vec<_> = c.iter().map(|c| (c.col, c.width)).collect();
        assert_eq!(cols, [(0, 1), (1, 2), (3, 1), (4, 1)]);
        let c = row_cells("\tx\r");
        let cols: Vec<_> = c.iter().map(|c| (c.col, c.width, c.class)).collect();
        assert_eq!(cols, [(0, 4, Class::Blank), (4, 1, Class::Word)]);
    }
}
