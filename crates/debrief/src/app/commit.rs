//! The commit screen: the message, then every
//! file's unified diff in one scrolling stream of [`Row`]s.
//!
//! `show()` runs on a worker thread: the [`Vcs`](crate::vcs::Vcs) handle is
//! not `Send`, so the thread reopens the repository from its root (with
//! the same options and jj operation pin) and
//! sends the result back as a request-tagged [`AppEvent::Loaded`], then
//! its syntax colours as an [`AppEvent::Highlighted`] with the same tag, so
//! the diff shows at once and the colours follow. Results for a view that
//! is no longer open are dropped. git's working copy loads the same way,
//! as an [`AppEvent::WorkingLoaded`].

use std::collections::BTreeSet;
use std::path::PathBuf;

use debrief_review::Side;
use ratatui::layout::Rect;

use crate::vcs::{
    CommitDiff, CommitInfo, Context, FileDiff, Kind, LineKind, OpenOptions, WorkingCopy,
};

use super::file_tree::Entry as FileEntry;
use super::highlight::{self, DiffSpans, Spans};
use super::motion::{Cell, Cursor};
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
    /// A file's placeholder row instead of hunks: a binary file, a note
    /// (too large, not a regular file), or [`NO_CONTENT`].
    Binary,
    /// Spacing between the message and files, and between files.
    Blank,
    /// Side by side: the padding of the shorter side of a change block.
    /// `line` is the paired line on the other side.
    Filler,
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

/// Columns before a row's text on one side of the side by side layout,
/// `num ± `.
pub fn side_gutter_width(num_w: usize) -> usize {
    num_w + 3
}

/// The placeholder row of a file with no hunks that is not binary (a
/// mode change, a pure rename or copy, an empty file).
pub const NO_CONTENT: &str = "(no content changes)";

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
    if let Some((old, new)) = &f.mode_change {
        s.push_str(&format!(" (mode {old} → {new})"));
    }
    format!("── {s} ──")
}

/// Flatten a commit into rows: message lines, a blank row, then per file
/// its header, a placeholder or its hunks (header, lines, and a
/// no-newline row where one applies), files separated by blank rows.
/// Files in `order` (indices into `diff.files`, which rows keep): the
/// file list's tree order.
pub fn flatten_in(diff: &CommitDiff, order: &[usize]) -> Vec<Row> {
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
    for (fi, f) in order.iter().filter_map(|&i| Some((i, diff.files.get(i)?))) {
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
        if let Some(note) = &f.note {
            rows.push(row(RowKind::Binary, Some(fi), None, None, note));
        } else if f.binary && f.hunks.is_empty() {
            rows.push(row(
                RowKind::Binary,
                Some(fi),
                None,
                None,
                "binary file not shown",
            ));
        } else if f.hunks.is_empty() {
            rows.push(row(RowKind::Binary, Some(fi), None, None, NO_CONTENT));
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

/// The commit-view message of git's working copy.
pub fn working_header(base: Option<&CommitInfo>) -> String {
    match base {
        Some(b) => format!(
            "Working copy (uncommitted) on {} {}",
            super::short_id(b),
            b.summary
        ),
        None => "Working copy (uncommitted; no commits yet)".to_string(),
    }
}

/// git's working copy as the diff the commit view shows.
fn working_diff(w: &WorkingCopy) -> CommitDiff {
    CommitDiff {
        message: working_header(w.base.as_ref()),
        files: w.diff.files.clone(),
    }
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
    /// The file list's tree.
    Files,
    /// The commit list under the tree (`[view] sidebar_commits`).
    Commits,
}

/// The open commit.
pub struct CommitView {
    pub(super) commit: String,
    /// Tags the worker's answer; a different one is stale.
    pub(super) request: u64,
    pub(super) loading: bool,
    pub(super) diff: Option<CommitDiff>,
    pub(super) rows: Vec<Row>,
    /// Syntax colours of `diff`, `[file][hunk][line]`; empty until they
    /// arrive (or with highlighting off).
    pub(super) spans: DiffSpans,
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
    /// Rows the view may scroll past the last one (room for the comment
    /// box under them).
    pub(super) overscroll: usize,
    /// Text columns on screen (after the gutter), from the layout.
    pub(super) text_w: usize,
    pub(super) search: Search,
    /// Visual mode's anchor row.
    pub(super) anchor: usize,
    /// The file list's selected entry (an index into `entries`; it may
    /// be inside a collapsed directory, which then shows it).
    pub(super) side_sel: usize,
    /// The file list as a tree (else flat, in git's order).
    pub(super) tree: bool,
    /// The file list's first drawn row after a wheel scroll or a click
    /// (see [`App::files_skip`]).
    pub(super) files_top: Option<FilesTop>,
    /// The file list's entries, every directory expanded.
    pub(super) entries: Vec<FileEntry>,
    /// The diff's file order: the files of `entries`, top to bottom.
    pub(super) order: Vec<usize>,
    /// The collapsed directories' paths.
    pub(super) collapsed: BTreeSet<String>,
    /// A picked comment to put the cursor on once the diff loads.
    pub(super) goto_comment: Option<debrief_review::Comment>,
    /// The working copy: git's uncommitted changes or jj's `@`.
    pub(super) working: bool,
    /// git's working copy: `HEAD` (`None` before the first commit).
    pub(super) base: Option<CommitInfo>,
    /// A reload in place: the cursor row to keep once the diff loads.
    pub(super) keep_row: Option<usize>,
    /// A `T` reload: the line to put the cursor back on once it loads,
    /// and the cursor's row on screen to keep.
    pub(super) keep_anchor: Option<(Anchor, usize)>,
    /// A saved or captured view to put back once the diff loads.
    pub(super) restore: Option<super::session::Restore>,
    /// Side by side wanted: the layout `set_diff` builds.
    pub(super) split_on: bool,
    /// Side by side as shown: the other side's rows (`rows` holds the
    /// cursor's side).
    pub(super) split: Option<super::split::Split>,
    /// The cursor's side in side by side.
    pub(super) side: Side,
}

pub(super) use crate::session::Anchor;

/// The file-list row its view keeps on screen: the selection while the
/// list has the keys (`true`), else the diff cursor's file.
pub(super) type FilesFollow = (bool, Option<usize>);

/// The file list's first drawn row as a wheel scroll or a click left it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct FilesTop {
    pub(super) top: usize,
    /// The list's height then.
    pub(super) h: usize,
    /// What the view followed then; `None` once a key moved in the list.
    pub(super) follow: Option<FilesFollow>,
}

impl CommitView {
    fn new(commit: String, request: u64) -> Self {
        CommitView {
            commit,
            request,
            loading: true,
            diff: None,
            rows: Vec::new(),
            spans: Vec::new(),
            num_w: MIN_NUM_W,
            cells: Vec::new(),
            cur: Cursor::default(),
            want_col: 0,
            scroll: 0,
            hscroll: 0,
            view_h: 20,
            overscroll: 0,
            text_w: 60,
            search: Search::default(),
            anchor: 0,
            side_sel: 0,
            files_top: None,
            tree: true,
            entries: Vec::new(),
            order: Vec::new(),
            collapsed: BTreeSet::new(),
            goto_comment: None,
            working: false,
            base: None,
            keep_row: None,
            keep_anchor: None,
            restore: None,
            split_on: false,
            split: None,
            side: Side::New,
        }
    }

    pub(super) fn set_diff(&mut self, diff: CommitDiff) {
        self.build_entries(&diff);
        self.build_rows(&diff);
        self.spans = Vec::new();
        self.num_w = num_width(&diff);
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

    /// The header row of the first file (in the diff's order) at or under
    /// `path`, either side of a rename.
    pub(super) fn path_row(&self, path: &str) -> Option<usize> {
        let diff = self.diff.as_ref()?;
        let under = |p: &Option<String>| p.as_deref().is_some_and(|p| crate::vcs::under(p, path));
        let file = self.order.iter().copied().find(|&i| {
            diff.files
                .get(i)
                .is_some_and(|f| under(&f.new_path) || under(&f.old_path))
        })?;
        self.header_row(file)
    }

    pub(super) fn header_row(&self, file: usize) -> Option<usize> {
        self.rows
            .iter()
            .position(|r| r.kind == RowKind::FileHeader && r.file == Some(file))
    }

    /// The cursor row as an [`Anchor`].
    pub(super) fn anchor(&self) -> Option<Anchor> {
        let diff = self.diff.as_ref()?;
        let r = self.rows.get(self.cur.row)?;
        let Some(fi) = r.file.or_else(|| self.file_at(self.cur.row)) else {
            return Some(Anchor::Row(self.cur.row));
        };
        let f = diff.files.get(fi)?;
        let path = file_path(f).to_string();
        let line = match (r.hunk, r.line) {
            (Some(h), Some(l)) => f.hunks.get(h)?.lines.get(l),
            _ => None,
        };
        // Side by side's left side prefers its own (old) numbers.
        let old_first = self.split_side() == Some(Side::Old);
        Some(match line {
            Some(l) => match (l.new, l.old) {
                (_, Some(o)) if old_first => Anchor::Line {
                    path,
                    side: Side::Old,
                    line: o,
                },
                (Some(n), _) => Anchor::Line {
                    path,
                    side: Side::New,
                    line: n,
                },
                (None, Some(o)) => Anchor::Line {
                    path,
                    side: Side::Old,
                    line: o,
                },
                (None, None) => Anchor::File(path),
            },
            None => Anchor::File(path),
        })
    }

    /// The row of `a` in the current rows: the same line when it is still
    /// shown, else the nearest line of the same file (by that side's line
    /// number), else the file's header.
    pub(super) fn anchor_row(&self, a: &Anchor) -> Option<usize> {
        let diff = self.diff.as_ref()?;
        let (path, new, line) = match a {
            Anchor::Row(r) => return Some(*r),
            Anchor::File(p) => (p, true, None),
            Anchor::Line { path, side, line } => (path, *side == Side::New, Some(*line)),
        };
        let fi = diff.files.iter().position(|f| file_path(f) == path)?;
        let header = self.header_row(fi);
        let Some(want) = line else { return header };
        let side = |r: &Row| -> Option<u32> {
            if !matches!(r.kind, RowKind::Line(_)) || r.file != Some(fi) {
                return None;
            }
            let l = diff.files[fi].hunks.get(r.hunk?)?.lines.get(r.line?)?;
            if new { l.new } else { l.old }
        };
        self.rows
            .iter()
            .enumerate()
            .filter_map(|(i, r)| side(r).map(|n| (i, n.abs_diff(want))))
            .min_by_key(|&(_, d)| d)
            .map(|(i, _)| i)
            .or(header)
    }

    pub(super) fn file_count(&self) -> usize {
        self.diff.as_ref().map_or(0, |d| d.files.len())
    }
}

/// Load a commit on a worker thread; the result comes back on `tx`,
/// followed by its colours when `colour` is set and any line has some.
/// `pin`: the jj operation the app reads at, so the worker sees the same
/// snapshot.
fn spawn_show(
    (root, opts, pin): (PathBuf, OpenOptions, Option<String>),
    commit: String,
    context: Context,
    request: u64,
    colour: bool,
    tx: std::sync::mpsc::Sender<AppEvent>,
) {
    std::thread::spawn(move || {
        let result = crate::vcs::reopen(&root, opts, pin)
            .and_then(|v| v.show(&commit, context))
            .map_err(|e| format!("{e:#}"));
        let diff = result.as_ref().ok().filter(|_| colour).cloned();
        if tx.send(AppEvent::Loaded { request, result }).is_err() {
            return;
        }
        if let Some(spans) = diff.as_ref().and_then(highlight::highlight_diff) {
            let _ = tx.send(AppEvent::Highlighted { request, spans });
        }
    });
}

/// Read git's working copy on a worker thread, as [`spawn_show`] does a
/// commit: the result comes back as an [`AppEvent::WorkingLoaded`], then
/// its colours.
fn spawn_working(
    (root, opts): (PathBuf, OpenOptions),
    context: Context,
    request: u64,
    colour: bool,
    tx: std::sync::mpsc::Sender<AppEvent>,
) {
    std::thread::spawn(move || {
        let result = crate::vcs::reopen(&root, opts, None)
            .and_then(|v| v.working_copy(context))
            .map_err(|e| format!("{e:#}"));
        let diff = result
            .as_ref()
            .ok()
            .and_then(Option::as_ref)
            .filter(|_| colour)
            .map(working_diff);
        if tx
            .send(AppEvent::WorkingLoaded { request, result })
            .is_err()
        {
            return;
        }
        if let Some(spans) = diff.as_ref().and_then(highlight::highlight_diff) {
            let _ = tx.send(AppEvent::Highlighted { request, spans });
        }
    });
}

impl App {
    /// Open the log's commit `i`: mark it seen, switch screens and start
    /// loading its diff. With no event sender (unit tests) it loads here.
    /// The working copy is brought up to date first: git re-reads it, jj
    /// snapshots and opens the current `@`.
    pub(super) fn open_index(&mut self, i: usize) {
        let Some(row) = self.rows.get(i) else {
            return;
        };
        if row.working {
            return self.open_working();
        }
        self.open_row(i, false);
    }

    fn open_working(&mut self) {
        match self.vcs.kind() {
            Kind::Git => {
                let Some(i) = self.working_index() else {
                    return self.set_status("The working copy is clean");
                };
                let base = self.working.as_ref().and_then(|w| w.base.clone());
                let request = self.enter_view(i);
                if let Some(v) = &mut self.commit {
                    v.working = true;
                    v.base = base;
                }
                self.start_working(request);
            }
            Kind::Jj => {
                self.reload(true);
                match self.working_index() {
                    Some(i) => self.open_row(i, true),
                    None => self.set_status("@ is not in the log"),
                }
            }
        }
    }

    /// Switch to the commit screen on log row `i` with a fresh view;
    /// returns its request number. Opened from the commit list, the list
    /// keeps the keys and an auto peek stays open.
    fn enter_view(&mut self, i: usize) -> u64 {
        // The view being left is remembered for its session.
        self.session_capture();
        self.hidden_open = None;
        let from_list = self.commit_list.opening;
        let id = self.rows[i].info.commit.clone();
        self.mark_row(i);
        self.cursor = i;
        self.screen = Screen::Commit(i);
        self.mode = Mode::Normal;
        if !from_list {
            self.set_focus(Focus::Diff);
        }
        self.commit_list_centre(i);
        self.next_request += 1;
        let request = self.next_request;
        // The session's layout follows once the diff (and the file list
        // that narrows the pane) is in: `loaded` -> `commit_follow`.
        self.commit = Some(CommitView::new(id, request));
        // Before the load: a session sets the context it loads in.
        self.session_enter(i);
        let tree = self.files_tree;
        if let Some(v) = &mut self.commit {
            v.tree = tree;
        }
        if from_list {
            // A saved session's file list setting may hide the list.
            self.sidebar_hold();
            self.commit_list_refocus();
        }
        request
    }

    fn open_row(&mut self, i: usize, working: bool) {
        let request = self.enter_view(i);
        if let Some(v) = &mut self.commit {
            v.working = working;
        }
        self.start_show(request);
    }

    /// Load the open view's commit (on the worker when there is a sender).
    fn start_show(&mut self, request: u64) {
        let Some(id) = self.commit.as_ref().map(|v| v.commit.clone()) else {
            return;
        };
        match self.sender.clone() {
            Some(tx) => {
                let at = (
                    self.vcs.root().to_path_buf(),
                    self.vcs.open_options(),
                    self.vcs.pinned_op(),
                );
                let colour = self.config.render.highlight;
                spawn_show(at, id, self.context, request, colour, tx)
            }
            None => {
                let result = self
                    .vcs
                    .show(&id, self.context)
                    .map_err(|e| format!("{e:#}"));
                let diff = result.as_ref().ok().cloned();
                self.loaded(request, result);
                if let Some(diff) = diff {
                    self.highlight_later(request, diff);
                }
            }
        }
    }

    /// Read git's working copy into the open view, in the session's
    /// context (on the worker when there is a sender).
    fn start_working(&mut self, request: u64) {
        if let Some(v) = &mut self.commit {
            v.loading = true;
        }
        match self.sender.clone() {
            Some(tx) => {
                let at = (self.vcs.root().to_path_buf(), self.vcs.open_options());
                let colour = self.config.render.highlight;
                spawn_working(at, self.context, request, colour, tx)
            }
            None => {
                let result = self
                    .vcs
                    .working_copy(self.context)
                    .map_err(|e| format!("{e:#}"));
                let diff = result
                    .as_ref()
                    .ok()
                    .and_then(Option::as_ref)
                    .map(working_diff);
                self.working_loaded(request, result);
                if let Some(diff) = diff {
                    self.highlight_later(request, diff);
                }
            }
        }
    }

    /// The worker's git working copy: shown if its view is still open on
    /// that load; a clean tree closes the view (and drops the row).
    pub(super) fn working_loaded(
        &mut self,
        request: u64,
        result: Result<Option<WorkingCopy>, String>,
    ) {
        if !self
            .commit
            .as_ref()
            .is_some_and(|v| v.request == request && v.working)
        {
            return;
        }
        // A follow load that lands on a V selection or a draft would move
        // their rows: drop it, and reload once they close.
        if self.follow_request.take_if(|r| *r == request).is_some() && self.follow_held() {
            if let Some(v) = &mut self.commit {
                v.loading = false;
                v.keep_row = None;
                v.keep_anchor = None;
            }
            self.follow_stale = true;
            return;
        }
        match result {
            Ok(Some(w)) => {
                if let Some(v) = &mut self.commit {
                    v.base = w.base.clone();
                }
                let diff = working_diff(&w);
                // Fresher than the row: it and its stat follow (line
                // counts are the same in either context). Under a path
                // filter the row shows only while a file under it changed.
                self.working_request += 1;
                self.working = (self.path_touched(&w.diff.files) && self.on_checked_out())
                    .then(|| (&w).into());
                self.stat_put_working(&w);
                self.working_rows();
                self.loaded(request, Ok(diff));
            }
            Ok(None) => {
                // Never leave the obsolete diff open to comments.
                self.close_commit();
                self.working_request += 1;
                self.working = None;
                self.stat_forget(super::WORKING_ID);
                self.working_rows();
                self.set_status("The working copy is clean");
            }
            Err(e) => {
                if let Some(v) = &mut self.commit {
                    v.loading = false;
                }
                self.set_status(format!("cannot read the working copy: {e}"));
            }
        }
    }

    /// Colour a diff loaded on the UI thread: on a thread of its own when
    /// there is a sender, else here.
    fn highlight_later(&mut self, request: u64, diff: CommitDiff) {
        if !self.config.render.highlight {
            return;
        }
        match self.sender.clone() {
            Some(tx) => {
                std::thread::spawn(move || {
                    if let Some(spans) = highlight::highlight_diff(&diff) {
                        let _ = tx.send(AppEvent::Highlighted { request, spans });
                    }
                });
            }
            None => {
                if let Some(spans) = highlight::highlight_diff(&diff) {
                    self.highlighted(request, spans);
                }
            }
        }
    }

    /// A diff's colours: kept if its view is still open on that load.
    pub(super) fn highlighted(&mut self, request: u64, spans: DiffSpans) {
        if let Some(v) = self
            .commit
            .as_mut()
            .filter(|v| v.request == request && !v.loading && v.diff.is_some())
        {
            v.spans = spans;
        }
    }

    /// `r` in the commit view, and a return from the editor or a launcher:
    /// reload the log (jj: snapshot first) and, on the working copy, its
    /// diff in place, keeping the cursor row; a clean git tree closes the
    /// view when the diff comes back empty. Other commits stay as shown.
    pub(super) fn reload_view(&mut self) {
        self.reload(true);
        let Some(v) = self.commit.as_ref().filter(|v| v.working) else {
            return;
        };
        let row = v.cur.row;
        match self.vcs.kind() {
            Kind::Git => {
                self.next_request += 1;
                let request = self.next_request;
                if let Some(v) = &mut self.commit {
                    v.request = request;
                    v.keep_row = Some(row);
                }
                self.start_working(request);
            }
            Kind::Jj => {
                let Some(id) = self
                    .working_index()
                    .map(|i| self.rows[i].info.commit.clone())
                else {
                    return self.set_status("@ is not in the log");
                };
                self.next_request += 1;
                let request = self.next_request;
                if let Some(v) = &mut self.commit {
                    v.request = request;
                    v.commit = id;
                    v.keep_row = Some(row);
                    v.loading = true;
                }
                self.commit_reindex();
                self.start_show(request);
            }
        }
    }

    /// git, the tree watcher under `[view] follow_working`: reload an open
    /// working-copy view in place,
    /// the cursor on its line at the same screen row. Not while a
    /// selection or a comment draft holds rows of the current diff. A
    /// load still running is superseded, so the last change wins.
    /// Whether it started a load.
    pub(super) fn working_follow(&mut self) -> bool {
        if !self.config.view.follow_working || !matches!(self.screen, Screen::Commit(_)) {
            return false;
        }
        if self.follow_held() {
            // Reloaded once V or the comment box closes.
            self.follow_stale = self.viewing_working_copy();
            return self.follow_stale;
        }
        let Some(v) = self
            .commit
            .as_ref()
            .filter(|v| v.working && v.diff.is_some())
        else {
            return false;
        };
        let anchor = v.anchor().map(|a| (a, v.cur.row.saturating_sub(v.scroll)));
        let row = v.cur.row;
        self.next_request += 1;
        let request = self.next_request;
        if let Some(v) = &mut self.commit {
            v.request = request;
            // The row is the fallback when the anchor's line is gone.
            v.keep_row = Some(row);
            v.keep_anchor = anchor;
        }
        self.follow_stale = false;
        self.follow_request = Some(request);
        self.start_working(request);
        true
    }

    /// V or the comment box holds rows of the open diff: a follow reload
    /// waits.
    fn follow_held(&self) -> bool {
        matches!(self.mode, Mode::Visual | Mode::Comment)
    }

    /// After a key: a tree change that waited on V or the comment box
    /// reloads now that they are closed.
    pub(super) fn follow_resume(&mut self) {
        if self.follow_stale && !self.follow_held() {
            self.follow_stale = false;
            self.working_follow();
        }
    }

    /// `T`: switch between hunks and the full file for this session and
    /// reload the open commit in that mode, the cursor kept on its line.
    pub(super) fn toggle_context(&mut self) {
        self.context = match self.context {
            Context::Default => Context::Full,
            Context::Full => Context::Default,
        };
        self.set_status(match self.context {
            Context::Full => "full file",
            Context::Default => "hunks",
        });
        let Some(v) = &self.commit else { return };
        let anchor = v.anchor().map(|a| (a, v.cur.row.saturating_sub(v.scroll)));
        let working = v.working;
        self.next_request += 1;
        let request = self.next_request;
        if let Some(v) = &mut self.commit {
            v.request = request;
            // While the diff loads there is no cursor yet: the pending
            // anchor or row (a restored session's) stays.
            if anchor.is_some() {
                v.keep_anchor = anchor;
                v.keep_row = None;
            }
        }
        if working && self.vcs.kind() == Kind::Git {
            return self.start_working(request);
        }
        if let Some(v) = &mut self.commit {
            v.loading = true;
        }
        self.start_show(request);
    }

    /// The session's context: hunks or the full file (`T`).
    pub fn context(&self) -> Context {
        self.context
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
                let keep = v.keep_row.take();
                let anchor = v.keep_anchor.take();
                let fresh = keep.is_none() && anchor.is_none() && v.restore.is_none();
                v.set_diff(diff);
                // Under a path filter a fresh view starts on the first
                // file under it (a kept row or a session wins).
                if fresh && let Some(row) = self.opts.path.as_deref().and_then(|p| v.path_row(p)) {
                    v.goto_row(row);
                    v.cursor_to_top();
                }
                // Side by side: the anchor's line is on its side.
                if let Some((Anchor::Line { side, .. }, _)) = &anchor
                    && v.split_side().is_some_and(|s| s != *side)
                {
                    v.swap_side();
                }
                if let Some(row) = keep {
                    v.goto_row(row.min(v.rows.len().saturating_sub(1)));
                }
                if let Some((row, on_screen)) =
                    anchor.and_then(|(a, y)| Some((v.anchor_row(&a)?, y)))
                {
                    v.goto_row(row.min(v.rows.len().saturating_sub(1)));
                    v.scroll = v.cur.row.saturating_sub(on_screen).min(v.max_scroll());
                }
                let pick = v.goto_comment.take();
                let restore = v.restore.take();
                // A search typed while loading saw no rows.
                self.refresh_search();
                self.commit_follow();
                if let Some(r) = restore {
                    self.session_put_back(r);
                }
                if let Some(c) = pick {
                    self.goto_comment(&c);
                }
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
        self.session_capture();
        if let Some(i) = self.commit.take().and_then(|v| self.view_index(&v)) {
            self.cursor = i;
        }
        self.screen = Screen::Log;
        self.mode = Mode::Normal;
        self.set_focus(Focus::Diff);
        self.keep_visible();
    }

    /// `]]` (`forward`, the next row in log order) / `[[`; with the open
    /// commit filtered out, from its place in the unfiltered log.
    pub(super) fn step_commit(&mut self, forward: bool) {
        let Screen::Commit(i) = self.screen else {
            return;
        };
        let to = if let Some(to) = self.step_past_hidden(forward) {
            to
        } else if forward {
            Some(i + 1).filter(|&j| j < self.rows.len())
        } else {
            i.checked_sub(1)
        };
        let Some(j) = to else {
            return self.set_status(if forward {
                "no next commit"
            } else {
                "no previous commit"
            });
        };
        // Stepped from the file list or the commit list: that list keeps
        // the keys, as Enter in the commit list does.
        match self.focus {
            Focus::Commits => self.commit_list_open_at(j),
            Focus::Files => {
                self.open_index(j);
                if matches!(self.screen, Screen::Commit(_)) && self.focus != Focus::Files {
                    self.focus_pane(Focus::Files);
                }
            }
            Focus::Diff => self.open_index(j),
        }
    }

    /// After a reload: keep the open commit's log index on its commit.
    pub(super) fn commit_reindex(&mut self) {
        if let (Screen::Commit(_), Some(v)) = (self.screen, &self.commit) {
            let i = self
                .view_index(v)
                .unwrap_or_else(|| self.cursor.min(self.rows.len().saturating_sub(1)));
            self.cursor = i;
            self.screen = Screen::Commit(i);
        }
    }

    /// The log row of view `v`: the working copy's row, else its commit's.
    pub(super) fn view_index(&self, v: &CommitView) -> Option<usize> {
        if v.working {
            return self.working_index();
        }
        self.rows.iter().position(|r| r.info.commit == v.commit)
    }

    /// Whether the open view is the working copy.
    pub fn viewing_working_copy(&self) -> bool {
        self.commit.as_ref().is_some_and(|v| v.working)
    }

    /// The diff rows of the open commit (empty while loading or on the log).
    pub fn commit_rows(&self) -> &[Row] {
        self.commit.as_ref().map_or(&[], |v| &v.rows)
    }

    /// The syntax colours of a Line row of the open commit, if it has any
    /// (yet).
    pub fn commit_spans(&self, r: &Row) -> Option<&Spans> {
        self.side_spans(r, None)
    }

    /// [`App::commit_spans`] on one side of side by side: a context line
    /// on the left takes the old side's colours.
    pub fn side_spans(&self, r: &Row, side: Option<Side>) -> Option<&Spans> {
        let v = self.commit.as_ref()?;
        let (f, h, l) = (r.file?, r.hunk?, r.line?);
        let RowKind::Line(kind) = r.kind else {
            return None;
        };
        let hunk = v.spans.get(f)?.get(h)?;
        if side == Some(Side::Old) && kind == LineKind::Context {
            return hunk.old.get(l)?.as_ref();
        }
        hunk.lines.get(l)?.as_ref()
    }

    /// The diff flattened into [`App::commit_rows`].
    pub fn commit_diff(&self) -> Option<&CommitDiff> {
        self.commit.as_ref()?.diff.as_ref()
    }

    /// The commit ID of the open commit.
    pub fn commit_id(&self) -> Option<&str> {
        self.commit.as_ref().map(|v| v.commit.as_str())
    }

    /// The header row's commit: its short ID (as the log shows it) and
    /// subject, from its log row, else the first line of the loaded
    /// message. `None` with no subject (git's working copy, a jj change
    /// without a description): the header row hides.
    pub fn commit_header(&self) -> Option<(String, Subject<'_>)> {
        let v = self.commit.as_ref()?;
        // git's working-copy row has a made-up summary, not a subject.
        let row = self
            .view_index(v)
            .and_then(|i| self.rows.get(i))
            .filter(|r| r.info.commit != super::WORKING_ID);
        let id = match row {
            Some(r) => super::short_id(&r.info).to_string(),
            None => v.commit.get(..8).unwrap_or(&v.commit).to_string(),
        };
        let line = row
            .map(|r| r.info.summary.as_str())
            .filter(|s| !s.trim().is_empty())
            .or_else(|| v.diff.as_ref()?.message.lines().next())
            .unwrap_or("");
        let subject = Subject::parse(line);
        (!subject.text.is_empty()).then_some((id, subject))
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
        self.commit_list_keep_visible();
        let area = Rect::new(0, 0, self.size.0, self.size.1.saturating_sub(1));
        self.sidebar_remember_width();
        self.files_pin_settle();
        let diff = self.commit_layout(area).diff;
        self.layout_sync(diff.width);
        let marks = self.review_gutter();
        let over = self.comment_overscroll();
        let side_w = self.side_pane(diff).width as usize;
        if let Some(v) = &mut self.commit {
            v.view_h = (diff.height as usize).max(1);
            v.overscroll = over;
            let gutter = if v.is_split() {
                side_gutter_width(v.num_w)
            } else {
                gutter_width(v.num_w)
            };
            v.text_w = side_w.saturating_sub(gutter + marks).max(1);
            v.keep_visible();
        }
        self.comment_fit();
    }
}

/// A commit subject, split as a conventional commit's
/// `type(scope)!: description` when it is one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Subject<'a> {
    /// `type(scope)!`, without the colon; `None` when the subject is not
    /// a conventional commit.
    pub kind: Option<&'a str>,
    /// The type alone, lowercase ASCII as written (`feat`).
    pub ty: &'a str,
    /// The rest: the description, or the whole subject.
    pub text: &'a str,
}

impl<'a> Subject<'a> {
    pub fn parse(line: &'a str) -> Self {
        let line = line.trim();
        let plain = Subject {
            kind: None,
            ty: "",
            text: line,
        };
        let Some((head, rest)) = line.split_once(": ") else {
            return plain;
        };
        let ty_end = head.find(['(', '!']).unwrap_or(head.len());
        let (ty, tail) = head.split_at(ty_end);
        let tail = tail.strip_suffix('!').unwrap_or(tail);
        let scope_ok = tail.is_empty()
            || (tail.starts_with('(')
                && tail.ends_with(')')
                && tail.len() > 2
                && !tail[1..tail.len() - 1].contains(['(', ')']));
        let ty_ok = !ty.is_empty()
            && ty.len() <= 16
            && ty.bytes().all(|b| b.is_ascii_alphabetic() || b == b'-')
            && ty.bytes().next().is_some_and(|b| b.is_ascii_lowercase());
        if !(ty_ok && scope_ok) || rest.trim().is_empty() {
            return plain;
        }
        Subject {
            kind: Some(head),
            ty,
            text: rest.trim(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vcs::{DiffLine, Hunk};

    #[test]
    fn subjects() {
        let s = Subject::parse("feat(launch): Enter opens ramble");
        assert_eq!(
            (s.kind, s.ty, s.text),
            (Some("feat(launch)"), "feat", "Enter opens ramble")
        );
        let s = Subject::parse("fix!: drop the old flag");
        assert_eq!(
            (s.kind, s.ty, s.text),
            (Some("fix!"), "fix", "drop the old flag")
        );
        let s = Subject::parse("refactor(ui)!: split");
        assert_eq!((s.kind, s.ty), (Some("refactor(ui)!"), "refactor"));
        for plain in [
            "Fix the thing",
            "Note: not a type",
            "see http://x: y",
            "feat(: broken",
            "feat(): empty scope",
            "feat: ",
            "",
        ] {
            let s = Subject::parse(plain);
            assert_eq!((s.kind, s.text), (None, plain.trim()), "{plain:?}");
        }
    }

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
                    note: None,
                    mode_change: None,
                    copied: false,
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
                    note: None,
                    mode_change: None,
                    copied: false,
                },
            ],
        };
        let rows = flatten_in(&diff, &[0, 1]);
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

    #[test]
    fn a_file_without_hunks_gets_a_placeholder_and_its_mode_change() {
        let diff = CommitDiff {
            message: String::new(),
            files: vec![FileDiff {
                old_path: Some("run.sh".into()),
                new_path: Some("run.sh".into()),
                binary: false,
                note: None,
                mode_change: Some(("100644".into(), "100755".into())),
                copied: false,
                hunks: vec![],
            }],
        };
        let rows = flatten_in(&diff, &[0]);
        let got: Vec<_> = rows.iter().map(|r| (r.kind, r.text.as_str())).collect();
        assert_eq!(
            got,
            [
                (RowKind::FileHeader, "── run.sh (mode 100644 → 100755) ──"),
                (RowKind::Binary, NO_CONTENT),
            ]
        );
    }
}
