//! The review batch in the app (spec §3 "Comments"): the debrief-review
//! batch for `Vcs::root()`, shared with ramble, re-read at most once a
//! second from the tick. Comments made on the displayed commit mark their
//! rows in the gutter; `]r` / `[r` jump between them, `K` shows them.
//!
//! No re-anchoring (spec Q11): a comment marks rows only when its revision
//! is the displayed commit. A comment on an earlier version of a jj change
//! stays in the batch with its own revision and excerpt.
//!
//! On the working copy (spec Q23) the markers are working-copy comments
//! (no revision, ramble's too) on the new side, and comments on the base
//! commit's old side.

use std::time::{Duration, Instant};

use debrief_review::{Comment, Review, Side};

use ratatui::layout::Rect;
use unicode_width::UnicodeWidthChar;

use super::commit::{RowKind, file_path};
use super::{App, DiffRow, Mode};
use crate::vcs::LineKind;

/// The gutter marker.
pub const MARKER: char = '●';
/// How often the batch file is re-read.
pub const REVIEW_POLL: Duration = Duration::from_secs(1);
/// Status when `]r` / `[r` finds nothing on this commit.
pub const NO_REVIEW: &str = "No review comments on this commit";
/// Status when `]r` / `[r` is at the last / first comment.
pub const NO_MORE_REVIEW: &str = "No more review comments";
/// Status when there is no batch (the cache dir couldn't be read).
pub const NO_BATCH: &str = "No review batch";

/// `[ISSUE] ` for a typed comment, `""` for an untyped one.
pub fn kind_tag(c: &Comment) -> String {
    c.kind.as_deref().map_or(String::new(), |k| {
        format!("[{}] ", debrief_review::Kind::label(k))
    })
}

impl App {
    /// Open the batch for the repo root; a failure leaves the status.
    pub(super) fn review_open(&mut self) {
        match Review::open(&self.opts.cache_dir, self.vcs.root()) {
            Ok(r) => self.review = Some(r),
            Err(e) => self.set_status(format!("review: {e}")),
        }
        self.review_polled = Some(Instant::now());
    }

    /// The open batch.
    pub fn review(&self) -> Option<&Review> {
        self.review.as_ref()
    }

    /// Re-read the batch now; errors go to the status line.
    pub(super) fn review_reload(&mut self) {
        if let Some(r) = &mut self.review
            && let Err(e) = r.reload()
        {
            let msg = format!("review: {e}");
            self.set_status(msg);
        }
    }

    /// From the tick: re-read the batch at most once per [`REVIEW_POLL`].
    pub(super) fn review_tick(&mut self, now: Instant) {
        if self.review.is_none() || self.review_polled.is_some_and(|t| now < t + REVIEW_POLL) {
            return;
        }
        self.review_polled = Some(now);
        self.review_reload();
        self.picker_refresh();
    }

    /// Comments made on the displayed commit.
    fn comments_here(&self) -> Vec<&Comment> {
        let (Some(r), Some(id)) = (&self.review, self.commit_id()) else {
            return Vec::new();
        };
        let working = self.viewing_working_copy();
        let base = working
            .then(|| self.working_base().ok().map(|b| b.commit))
            .flatten();
        r.comments()
            .iter()
            .filter(|c| match &c.rev {
                None => working,
                Some(rev) if rev.commit == id => true,
                Some(rev) => {
                    working && c.side == Some(Side::Old) && base.as_deref() == Some(&rev.commit)
                }
            })
            .collect()
    }

    /// Whether diff row `row` lies in comment `c` (its file, side and line).
    pub(super) fn row_in(&self, row: usize, c: &Comment) -> bool {
        let side = self.split_side();
        self.row_in_side(self.commit_rows(), side, row, c)
    }

    /// [`App::row_in`] over `rows`: unified rows (`side` `None`), or one
    /// side by side's, where a comment marks only the side it is on.
    fn row_in_side(&self, rows: &[DiffRow], side: Option<Side>, row: usize, c: &Comment) -> bool {
        let Some(diff) = self.commit_diff() else {
            return false;
        };
        if side.is_some_and(|s| s != c.side.unwrap_or(Side::New)) {
            return false;
        }
        let Some(r) = rows.get(row) else {
            return false;
        };
        let RowKind::Line(kind) = r.kind else {
            return false;
        };
        let (Some(f), Some(h), Some(l)) = (r.file, r.hunk, r.line) else {
            return false;
        };
        let Some(line) = diff.files[f].hunks.get(h).and_then(|h| h.lines.get(l)) else {
            return false;
        };
        let fd = &diff.files[f];
        // The side's own path (K11). Old-side comments match only the old
        // path; new-side ones also the display path older comments use.
        let path_ok = match c.side {
            Some(Side::Old) => fd.old_path.as_deref() == Some(c.path.as_str()),
            _ => fd.new_path.as_deref() == Some(c.path.as_str()) || file_path(fd) == c.path,
        };
        if !path_ok {
            return false;
        }
        let n = match (c.side, kind) {
            (Some(Side::New) | None, LineKind::Added | LineKind::Context) => line.new,
            (Some(Side::Old), LineKind::Removed | LineKind::Context) => line.old,
            _ => None,
        };
        n.is_some_and(|n| (c.lines.0..=c.lines.1).contains(&n))
    }

    /// Comments on the displayed commit that cover diff row `row`.
    pub fn comments_at(&self, row: usize) -> Vec<&Comment> {
        self.comments_here()
            .into_iter()
            .filter(|c| self.row_in(row, c))
            .collect()
    }

    /// Every marked diff row, sorted.
    pub fn review_marked_rows(&self) -> Vec<usize> {
        let cs = self.comments_here();
        if cs.is_empty() {
            return Vec::new();
        }
        (0..self.commit_rows().len())
            .filter(|&r| cs.iter().any(|c| self.row_in(r, c)))
            .collect()
    }

    /// Side by side: the marked rows of one side, sorted.
    pub fn split_marked_rows(&self, side: Side) -> Vec<usize> {
        let cs = self.comments_here();
        let rows = self.split_rows(side);
        if cs.is_empty() {
            return Vec::new();
        }
        (0..rows.len())
            .filter(|&r| cs.iter().any(|c| self.row_in_side(rows, Some(side), r, c)))
            .collect()
    }

    /// Columns of the marker gutter: 1 while the commit has comments.
    pub fn review_gutter(&self) -> usize {
        usize::from(!self.comments_here().is_empty())
    }

    /// The status-line preview: `● `, the kind (`[ISSUE] `) and the first
    /// line of the first comment on the cursor row.
    pub fn review_preview(&self) -> Option<String> {
        let c = *self.comments_at(self.commit_cursor().row).first()?;
        Some(format!(
            "{MARKER} {}{}",
            kind_tag(c),
            c.body.lines().next().unwrap_or("")
        ))
    }

    /// Rows `]r` / `[r` stop on: the first marked row of each comment.
    fn review_targets(&self) -> Vec<usize> {
        let n = self.commit_rows().len();
        let mut rows: Vec<usize> = self
            .comments_here()
            .iter()
            .filter_map(|c| (0..n).find(|&r| self.row_in(r, c)))
            .collect();
        rows.sort_unstable();
        rows.dedup();
        rows
    }

    /// `]r` (true) / `[r`: the next / previous commented row.
    pub(super) fn review_jump(&mut self, forward: bool, count: usize) {
        let targets = self.review_targets();
        if targets.is_empty() {
            return self.set_status(NO_REVIEW);
        }
        let mut row = self.commit_cursor().row;
        let mut moved = false;
        for _ in 0..count.max(1) {
            let next = if forward {
                targets.iter().find(|&&r| r > row)
            } else {
                targets.iter().rev().find(|&&r| r < row)
            };
            match next {
                Some(&r) => {
                    row = r;
                    moved = true;
                }
                None => break,
            }
        }
        if !moved {
            return self.set_status(NO_MORE_REVIEW);
        }
        if let Some(v) = &mut self.commit {
            v.goto_row(row);
        }
    }

    /// `K`: every comment on the cursor row, `● line a` / `● lines a-b`
    /// (and its kind, `[ISSUE]`) over its body, in a popup.
    pub(super) fn review_hover(&mut self) {
        let cs = self.comments_at(self.commit_cursor().row);
        if cs.is_empty() {
            return self.set_status("No comment here");
        }
        let parts: Vec<String> = cs
            .iter()
            .map(|c| {
                let (a, b) = c.lines;
                let at = if a == b {
                    format!("line {a}")
                } else {
                    format!("lines {a}-{b}")
                };
                let head = format!("{MARKER} {at} {}", kind_tag(c));
                format!("{}\n{}", head.trim_end(), c.body.trim_end())
            })
            .collect();
        self.popup = Some(parts.join("\n\n"));
        self.popup_scroll = 0;
        self.mode = Mode::Popup;
    }

    /// The `K` popup's text while it is open.
    pub fn popup(&self) -> Option<&str> {
        self.popup.as_deref()
    }

    /// The `K` popup laid out for the current screen: its box, its text
    /// wrapped to the box, and the first visible line. The box grows with
    /// the wrapped line count up to the screen height minus 2 (one row of
    /// margin above and below).
    pub fn popup_view(&self) -> Option<PopupView> {
        let text = self.popup.as_deref()?;
        let (sw, sh) = (self.size.0, self.size.1.saturating_sub(1));
        if sw < 6 || sh < 3 {
            return None;
        }
        let w = sw.saturating_sub(4).clamp(1, 72);
        let lines = wrap(text, w.saturating_sub(2).max(1) as usize);
        let body = (lines.len() as u16).min(sh.saturating_sub(4)).max(1);
        let h = body + 2;
        let rect = Rect::new((sw - w) / 2, (sh - h) / 2, w, h);
        let max = lines.len().saturating_sub(body as usize);
        Some(PopupView {
            rect,
            scroll: self.popup_scroll.min(max),
            max_scroll: max,
            lines,
        })
    }

    /// `j` / `k` / `C-d` / `C-u` / `gg` / `G` in an overflowing popup.
    pub(super) fn popup_move(&mut self, m: PopupMove) {
        let Some(v) = self.popup_view() else { return };
        let half = ((v.rect.height as usize).saturating_sub(2) / 2).max(1);
        let s = v.scroll;
        self.popup_scroll = match m {
            PopupMove::Down => s + 1,
            PopupMove::Up => s.saturating_sub(1),
            PopupMove::HalfDown => s + half,
            PopupMove::HalfUp => s.saturating_sub(half),
            PopupMove::Top => 0,
            PopupMove::Bottom => v.max_scroll,
        }
        .min(v.max_scroll);
    }

    pub(super) fn popup_close(&mut self) {
        self.popup = None;
        self.popup_scroll = 0;
        if self.mode == Mode::Popup {
            self.mode = Mode::Normal;
        }
    }
}

/// Scrolling in the `K` popup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PopupMove {
    Down,
    Up,
    HalfDown,
    HalfUp,
    Top,
    Bottom,
}

/// The `K` popup laid out (see [`App::popup_view`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PopupView {
    /// The box, borders included.
    pub rect: Rect,
    /// `text` wrapped to the inside of the box.
    pub lines: Vec<String>,
    /// The first visible line.
    pub scroll: usize,
    pub max_scroll: usize,
}

/// Word-wrap `text` to `width` columns, keeping its line breaks; a word
/// wider than `width` is split.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        let mut cur = String::new();
        let mut w = 0;
        for word in line.split_inclusive(' ') {
            let ww: usize = word.chars().map(|c| c.width().unwrap_or(0)).sum();
            let fits = w + ww.min(width + 1) <= width + usize::from(word.ends_with(' '));
            if !fits && !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
                w = 0;
            }
            for c in word.chars() {
                let cw = c.width().unwrap_or(0);
                if w + cw > width && !cur.is_empty() && c != ' ' {
                    out.push(std::mem::take(&mut cur));
                    w = 0;
                }
                cur.push(c);
                w += cw;
            }
        }
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::wrap;

    #[test]
    fn wrap_breaks_between_words_and_splits_long_ones() {
        assert_eq!(wrap("aa bb cc", 5), ["aa bb ", "cc"]);
        assert_eq!(wrap("abcdefg", 3), ["abc", "def", "g"]);
        assert_eq!(wrap("a\n\nb", 3), ["a", "", "b"]);
        assert_eq!(wrap("", 3), Vec::<String>::new());
    }
}
