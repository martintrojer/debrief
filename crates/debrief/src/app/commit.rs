//! The commit screen (spec §3 "Commit view"): the message, then every
//! file's unified diff in one scrolling stream of [`Row`]s.
//!
//! `show()` runs on a worker thread: the [`Vcs`](crate::vcs::Vcs) handle is
//! not `Send`, so the thread reopens the repository from its root and
//! sends the result back as a request-tagged [`AppEvent::Loaded`]; results
//! for a view that is no longer open are dropped.

use std::path::PathBuf;

use ratatui::layout::Rect;

use crate::vcs::{CommitDiff, FileDiff, LineKind};

use super::motion::{Cell, Cursor, row_cells};
use super::search::Search;
use super::{App, AppEvent, Mode, Screen};

/// What a diff row shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    /// A line of the commit message.
    Message,
    /// `── path ──`, with `old → new`, `(added)`, `(deleted)`, `(binary)`.
    FileHeader,
    /// The hunk's `@@` line.
    HunkHeader,
    /// A diff line; `text` is the line without its `+`/`-`/space prefix.
    Line(LineKind),
    /// `\ No newline at end of file` after the line `line` points at.
    /// Never a comment anchor.
    NoNewline,
    /// A binary file's placeholder row.
    Binary,
    /// Spacing between the message and files, and between files.
    Blank,
}

/// One row of the commit view. `file`, `hunk` and `line` index into
/// [`App::commit_diff`]: `files[file].hunks[hunk].lines[line]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub kind: RowKind,
    pub file: Option<usize>,
    pub hunk: Option<usize>,
    /// Index into `hunk.lines` (Line and NoNewline rows).
    pub line: Option<usize>,
    /// Untruncated text; for Line rows exactly `DiffLine::text`.
    pub text: String,
}

/// Fewest digits a gutter line-number column gets.
const MIN_NUM_W: usize = 4;

/// Digits of the widest old or new line number in `diff`, at least
/// [`MIN_NUM_W`].
pub fn num_width(diff: &CommitDiff) -> usize {
    let max = diff
        .files
        .iter()
        .flat_map(|f| &f.hunks)
        .flat_map(|h| &h.lines)
        .flat_map(|l| [l.old, l.new])
        .flatten()
        .max()
        .unwrap_or(0);
    max.to_string().len().max(MIN_NUM_W)
}

/// Columns before a row's text, `old new ± `, for line numbers `num_w`
/// digits wide.
pub fn gutter_width(num_w: usize) -> usize {
    2 * num_w + 4
}

/// The text of `\ No newline at end of file` rows.
pub const NO_NEWLINE: &str = "\\ No newline at end of file";

/// A file's display path: the new path, else the old one.
pub fn file_path(f: &FileDiff) -> &str {
    f.new_path
        .as_deref()
        .or(f.old_path.as_deref())
        .unwrap_or("?")
}

/// Added and removed line counts of a file.
pub fn file_counts(f: &FileDiff) -> (usize, usize) {
    let mut add = 0;
    let mut del = 0;
    for l in f.hunks.iter().flat_map(|h| &h.lines) {
        match l.kind {
            LineKind::Added => add += 1,
            LineKind::Removed => del += 1,
            LineKind::Context => {}
        }
    }
    (add, del)
}

fn header_text(f: &FileDiff) -> String {
    let mut s = match (&f.old_path, &f.new_path) {
        (Some(o), Some(n)) if o != n => format!("{o} → {n}"),
        (None, Some(n)) => format!("{n} (added)"),
        (Some(o), None) => format!("{o} (deleted)"),
        _ => file_path(f).to_string(),
    };
    if f.binary {
        s.push_str(" (binary)");
    }
    format!("── {s} ──")
}

/// Flatten a commit into rows: message lines, a blank row, then per file
/// its header, a binary placeholder or its hunks (header, lines, and a
/// no-newline row where one applies), files separated by blank rows.
pub fn flatten(diff: &CommitDiff) -> Vec<Row> {
    let row = |kind, file, hunk, line, text: &str| Row {
        kind,
        file,
        hunk,
        line,
        text: text.to_string(),
    };
    let mut rows = Vec::new();
    if !diff.message.is_empty() {
        for l in diff.message.lines() {
            rows.push(row(RowKind::Message, None, None, None, l));
        }
    }
    for (fi, f) in diff.files.iter().enumerate() {
        if !rows.is_empty() {
            rows.push(row(RowKind::Blank, None, None, None, ""));
        }
        rows.push(row(
            RowKind::FileHeader,
            Some(fi),
            None,
            None,
            &header_text(f),
        ));
        if f.binary && f.hunks.is_empty() {
            rows.push(row(
                RowKind::Binary,
                Some(fi),
                None,
                None,
                "binary file not shown",
            ));
        }
        for (hi, h) in f.hunks.iter().enumerate() {
            rows.push(row(
                RowKind::HunkHeader,
                Some(fi),
                Some(hi),
                None,
                &h.header,
            ));
            for (li, l) in h.lines.iter().enumerate() {
                let (fi, hi, li) = (Some(fi), Some(hi), Some(li));
                rows.push(row(RowKind::Line(l.kind), fi, hi, li, &l.text));
                if l.no_newline {
                    rows.push(row(RowKind::NoNewline, fi, hi, li, NO_NEWLINE));
                }
            }
        }
    }
    rows
}

/// A row as diff text: Line rows get one `+`/`-`/space prefix.
pub fn diff_text(r: &Row) -> String {
    match r.kind {
        RowKind::Line(LineKind::Added) => format!("+{}", r.text),
        RowKind::Line(LineKind::Removed) => format!("-{}", r.text),
        RowKind::Line(LineKind::Context) => format!(" {}", r.text),
        _ => r.text.clone(),
    }
}

/// Which pane of the commit screen has the keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Focus {
    #[default]
    Diff,
    Files,
}

/// The open commit.
pub struct CommitView {
    pub(super) commit: String,
    /// Tags the worker's answer; a different one is stale.
    pub(super) request: u64,
    pub(super) loading: bool,
    pub(super) diff: Option<CommitDiff>,
    pub(super) rows: Vec<Row>,
    /// Digits per gutter line-number column, from [`num_width`].
    pub(super) num_w: usize,
    pub(super) cells: Vec<Vec<Cell>>,
    pub(super) cur: Cursor,
    pub(super) want_col: usize,
    pub(super) scroll: usize,
    /// First text column drawn.
    pub(super) hscroll: usize,
    /// Diff rows on screen, from the layout.
    pub(super) view_h: usize,
    /// Text columns on screen (after the gutter), from the layout.
    pub(super) text_w: usize,
    pub(super) search: Search,
    /// Visual mode's anchor row.
    pub(super) anchor: usize,
    /// Selected file in the sidebar.
    pub(super) side_sel: usize,
}

impl CommitView {
    fn new(commit: String, request: u64) -> Self {
        CommitView {
            commit,
            request,
            loading: true,
            diff: None,
            rows: Vec::new(),
            num_w: MIN_NUM_W,
            cells: Vec::new(),
            cur: Cursor::default(),
            want_col: 0,
            scroll: 0,
            hscroll: 0,
            view_h: 20,
            text_w: 60,
            search: Search::default(),
            anchor: 0,
            side_sel: 0,
        }
    }

    pub(super) fn set_diff(&mut self, diff: CommitDiff) {
        self.rows = flatten(&diff);
        self.num_w = num_width(&diff);
        self.cells = self.rows.iter().map(|r| row_cells(&r.text)).collect();
        self.diff = Some(diff);
        self.loading = false;
        self.cur = Cursor::default();
        self.scroll = 0;
        self.hscroll = 0;
    }

    /// The file whose section holds `row` (the last header at or above it).
    pub(super) fn file_at(&self, row: usize) -> Option<usize> {
        let end = (row + 1).min(self.rows.len());
        self.rows[..end]
            .iter()
            .rev()
            .find(|r| r.kind == RowKind::FileHeader)
            .and_then(|r| r.file)
    }

    pub(super) fn header_row(&self, file: usize) -> Option<usize> {
        self.rows
            .iter()
            .position(|r| r.kind == RowKind::FileHeader && r.file == Some(file))
    }

    pub(super) fn file_count(&self) -> usize {
        self.diff.as_ref().map_or(0, |d| d.files.len())
    }
}

/// Load a commit on a worker thread; the result comes back on `tx`.
fn spawn_show(root: PathBuf, commit: String, request: u64, tx: std::sync::mpsc::Sender<AppEvent>) {
    std::thread::spawn(move || {
        let result = crate::vcs::open(&root)
            .and_then(|v| v.show(&commit))
            .map_err(|e| format!("{e:#}"));
        let _ = tx.send(AppEvent::Loaded { request, result });
    });
}

impl App {
    /// Open the log's commit `i`: mark it seen, switch screens and start
    /// loading its diff. With no event sender (unit tests) it loads here.
    pub(super) fn open_index(&mut self, i: usize) {
        let Some(info) = self.rows.get(i).map(|r| r.info.clone()) else {
            return;
        };
        self.mark_row(i);
        self.cursor = i;
        self.screen = Screen::Commit(i);
        self.mode = Mode::Normal;
        self.focus = Focus::Diff;
        self.next_request += 1;
        let request = self.next_request;
        self.commit = Some(CommitView::new(info.commit.clone(), request));
        match self.sender.clone() {
            Some(tx) => spawn_show(self.vcs.root().to_path_buf(), info.commit, request, tx),
            None => {
                let result = self.vcs.show(&info.commit).map_err(|e| format!("{e:#}"));
                self.loaded(request, result);
            }
        }
    }

    /// A worker's answer: show it if its view is still open.
    pub(super) fn loaded(&mut self, request: u64, result: Result<CommitDiff, String>) {
        let Some(v) = self.commit.as_mut().filter(|v| v.request == request) else {
            return;
        };
        if !matches!(self.screen, Screen::Commit(_)) {
            return;
        }
        match result {
            Ok(diff) => {
                v.set_diff(diff);
                self.commit_follow();
            }
            Err(e) => {
                v.loading = false;
                let short = v.commit.get(..8).unwrap_or(&v.commit).to_string();
                self.set_status(format!("cannot show {short}: {e}"));
            }
        }
    }

    /// Esc / q: back to the log, the cursor on the same commit.
    pub(super) fn close_commit(&mut self) {
        if let Some(id) = self.commit.take().map(|v| v.commit)
            && let Some(i) = self.rows.iter().position(|r| r.info.commit == id)
        {
            self.cursor = i;
        }
        self.screen = Screen::Log;
        self.mode = Mode::Normal;
        self.focus = Focus::Diff;
        self.keep_visible();
    }

    /// `]]` (`forward`, the next row in log order) / `[[`.
    pub(super) fn step_commit(&mut self, forward: bool) {
        let Screen::Commit(i) = self.screen else {
            return;
        };
        let to = if forward {
            Some(i + 1).filter(|&j| j < self.rows.len())
        } else {
            i.checked_sub(1)
        };
        match to {
            Some(j) => self.open_index(j),
            None => self.set_status(if forward {
                "no next commit"
            } else {
                "no previous commit"
            }),
        }
    }

    /// After a reload: keep the open commit's log index on its commit.
    pub(super) fn commit_reindex(&mut self) {
        if let (Screen::Commit(_), Some(v)) = (self.screen, &self.commit) {
            let i = self
                .rows
                .iter()
                .position(|r| r.info.commit == v.commit)
                .unwrap_or_else(|| self.cursor.min(self.rows.len().saturating_sub(1)));
            self.cursor = i;
            self.screen = Screen::Commit(i);
        }
    }

    /// The diff rows of the open commit (empty while loading or on the log).
    pub fn commit_rows(&self) -> &[Row] {
        self.commit.as_ref().map_or(&[], |v| &v.rows)
    }

    /// The diff flattened into [`App::commit_rows`].
    pub fn commit_diff(&self) -> Option<&CommitDiff> {
        self.commit.as_ref()?.diff.as_ref()
    }

    /// The commit ID of the open commit.
    pub fn commit_id(&self) -> Option<&str> {
        self.commit.as_ref().map(|v| v.commit.as_str())
    }

    /// Whether the open commit's diff is still loading.
    pub fn commit_loading(&self) -> bool {
        self.commit.as_ref().is_some_and(|v| v.loading)
    }

    /// The diff cursor: row and display column within the row's text.
    pub fn commit_cursor(&self) -> Cursor {
        self.commit.as_ref().map_or(Cursor::default(), |v| v.cur)
    }

    /// First row and first text column drawn.
    pub fn commit_scroll(&self) -> (usize, usize) {
        self.commit
            .as_ref()
            .map_or((0, 0), |v| (v.scroll, v.hscroll))
    }

    /// Digits per gutter line-number column of the open commit.
    pub fn commit_num_width(&self) -> usize {
        self.commit.as_ref().map_or(MIN_NUM_W, |v| v.num_w)
    }

    /// The file whose section holds the cursor.
    pub fn current_file(&self) -> Option<usize> {
        let v = self.commit.as_ref()?;
        v.file_at(v.cur.row)
    }

    pub fn focus(&self) -> Focus {
        self.focus
    }

    /// Size the diff view from the layout and keep the cursor in it.
    pub(super) fn commit_follow(&mut self) {
        let area = Rect::new(0, 0, self.size.0, self.size.1.saturating_sub(1));
        let diff = self.commit_layout(area).diff;
        if let Some(v) = &mut self.commit {
            v.view_h = (diff.height as usize).max(1);
            v.text_w = (diff.width as usize)
                .saturating_sub(gutter_width(v.num_w))
                .max(1);
            v.keep_visible();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vcs::{DiffLine, Hunk};

    fn line(kind: LineKind, text: &str, no_newline: bool) -> DiffLine {
        DiffLine {
            kind,
            old: None,
            new: None,
            text: text.into(),
            no_newline,
        }
    }

    #[test]
    fn flattens_message_files_hunks_and_no_newline() {
        let diff = CommitDiff {
            message: "subject\n\nbody".into(),
            files: vec![
                FileDiff {
                    old_path: Some("a".into()),
                    new_path: Some("b".into()),
                    binary: false,
                    hunks: vec![Hunk {
                        header: "@@ -1 +1 @@".into(),
                        old_start: 1,
                        new_start: 1,
                        lines: vec![
                            line(LineKind::Removed, "x", true),
                            line(LineKind::Added, "y", false),
                        ],
                    }],
                },
                FileDiff {
                    old_path: None,
                    new_path: Some("bin".into()),
                    binary: true,
                    hunks: vec![],
                },
            ],
        };
        let rows = flatten(&diff);
        let kinds: Vec<_> = rows.iter().map(|r| r.kind).collect();
        use RowKind::*;
        assert_eq!(
            kinds,
            [
                Message,
                Message,
                Message,
                Blank,
                FileHeader,
                HunkHeader,
                Line(LineKind::Removed),
                NoNewline,
                Line(LineKind::Added),
                Blank,
                FileHeader,
                Binary
            ]
        );
        assert_eq!(rows[4].text, "── a → b ──");
        assert_eq!(rows[10].text, "── bin (added) (binary) ──");
        assert_eq!(
            (rows[7].file, rows[7].hunk, rows[7].line),
            (Some(0), Some(0), Some(0))
        );
        assert_eq!(rows[7].text, NO_NEWLINE);
        let text: Vec<_> = rows[6..9].iter().map(diff_text).collect();
        assert_eq!(text, ["-x", NO_NEWLINE, "+y"]);
    }
}
