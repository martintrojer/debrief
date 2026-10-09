//! The review batch in the app (spec §3 "Comments"): the debrief-review
//! batch for `Vcs::root()`, shared with ramble, re-read at most once a
//! second from the tick. Comments made on the displayed commit mark their
//! rows in the gutter; `]r` / `[r` jump between them, `K` shows them.
//!
//! No re-anchoring (spec Q11): a comment marks rows only when its revision
//! is the displayed commit. A comment on an earlier version of a jj change
//! stays in the batch with its own revision and excerpt.

use std::time::{Duration, Instant};

use debrief_review::{Comment, Review, Side};

use super::commit::{RowKind, file_path};
use super::{App, Mode};
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
    }

    /// Comments made on the displayed commit.
    fn comments_here(&self) -> Vec<&Comment> {
        let (Some(r), Some(id)) = (&self.review, self.commit_id()) else {
            return Vec::new();
        };
        r.comments()
            .iter()
            .filter(|c| c.rev.as_ref().is_some_and(|rev| rev.commit == id))
            .collect()
    }

    /// Whether diff row `row` lies in comment `c` (its file, side and line).
    fn row_in(&self, row: usize, c: &Comment) -> bool {
        let (Some(v), Some(diff)) = (&self.commit, self.commit_diff()) else {
            return false;
        };
        let Some(r) = v.rows.get(row) else {
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
        if file_path(&diff.files[f]) != c.path {
            return false;
        }
        let n = match (c.side, kind) {
            (Some(Side::New), LineKind::Added | LineKind::Context) => line.new,
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

    /// Columns of the marker gutter: 1 while the commit has comments.
    pub fn review_gutter(&self) -> usize {
        usize::from(!self.comments_here().is_empty())
    }

    /// The status-line preview: `● ` and the first line of the first
    /// comment on the cursor row.
    pub fn review_preview(&self) -> Option<String> {
        let c = *self.comments_at(self.commit_cursor().row).first()?;
        Some(format!("{MARKER} {}", c.body.lines().next().unwrap_or("")))
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
    /// over its body, in a popup.
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
                format!("{MARKER} {at}\n{}", c.body.trim_end())
            })
            .collect();
        self.popup = Some(parts.join("\n\n"));
        self.mode = Mode::Popup;
    }

    /// The `K` popup's text while it is open.
    pub fn popup(&self) -> Option<&str> {
        self.popup.as_deref()
    }

    pub(super) fn popup_close(&mut self) {
        self.popup = None;
        if self.mode == Mode::Popup {
            self.mode = Mode::Normal;
        }
    }
}
