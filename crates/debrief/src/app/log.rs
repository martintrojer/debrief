//! The log screen: loading and reloading the commit list, its cursor
//! motions, and marking commits seen.
//!
//! The working copy: git gets a synthetic row on
//! top while the tree is dirty (under a path filter, while something
//! under the path is), never seen or new; jj marks the row of
//! this workspace's `@`. Loading snapshots jj first; a snapshot
//! error goes to the status line and the log loads from the last pin.
//!
//! git's row (dirty or not, and its file count) comes from
//! [`Vcs::working_status`](crate::vcs::Vcs::working_status), never the
//! full diff; with an event sender it is read on a worker thread and
//! comes back as a request-tagged [`AppEvent::WorkingRow`]; an answer to
//! an older request is dropped.
//!
//! Paging: the log asks the VCS for `limit` commits, the first page
//! `--limit` / `[log] limit`. When the VCS returns as many as asked, more
//! may exist: a `… more` row ends the log, and a move past the last row
//! (`j`, uncounted `G`, `C-d`, the wheel at the end; the same in the
//! commit list) asks for `limit` plus a page. With an event sender the
//! larger log is read on a worker thread at the same jj operation and
//! comes back as a request-tagged [`AppEvent::LogPage`]; one page loads
//! at a time, and a reload (which uses the grown limit) makes a reply in
//! flight stale. Once it lands the cursor stays on its commit and the
//! move that asked for it runs again, unless the cursor moved meanwhile.

use crate::seen::Status;
use crate::vcs::{CommitInfo, Kind, OpenOptions, WorkingStatus};

use super::{App, AppEvent, Row, Screen};

/// The commit ID of git's synthetic working-copy row (not a hex ID, so it
/// never names a commit).
pub const WORKING_ID: &str = "working-copy";

/// The commit ID prefix of a fold's `… more` row
/// (`fold-more:<merge>:<parent>`): not a hex ID, so it never names a
/// commit. Excluded wherever [`WORKING_ID`] is from seen marks, stats,
/// sessions and search.
pub const FOLD_MORE_PREFIX: &str = "fold-more:";

/// `id` is a fold's `… more` row (see [`FOLD_MORE_PREFIX`]).
pub fn is_fold_more(id: &str) -> bool {
    id.starts_with(FOLD_MORE_PREFIX)
}

/// The most commits of one bubble a seen mark reads (opening a merge, or
/// the first run); a larger bubble has only these marked.
const BUBBLE_CAP: usize = 10_000;

/// A synthetic row's ID: git's working copy or a fold's `… more`.
pub(super) fn is_synthetic(id: &str) -> bool {
    id == WORKING_ID || is_fold_more(id)
}

/// The commit ID of jj's root commit (and any all-zero ID).
pub fn is_zero_id(id: &str) -> bool {
    !id.is_empty() && id.bytes().all(|b| b == b'0')
}

/// The short ID shown in the log: the change ID for jj, else the commit
/// ID, first 8 characters.
pub fn short_id(c: &CommitInfo) -> &str {
    if is_synthetic(&c.commit) {
        return "";
    }
    let id = c.change.as_deref().unwrap_or(&c.commit);
    id.get(..8).unwrap_or(id)
}

/// `then` (unix seconds) relative to `now`: `42s`, `3m`, `2h`, `5d`, `3y`.
pub fn age(now: i64, then: i64) -> String {
    let s = (now - then).max(0);
    match s {
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m", s / 60),
        3600..86_400 => format!("{}h", s / 3600),
        86_400..31_536_000 => format!("{}d", s / 86_400),
        _ => format!("{}y", s / 31_536_000),
    }
}

/// Read git's working-copy row on a worker thread, as
/// [`App::load_working`] does without a sender.
fn spawn_working_status(
    (root, opts, path): (std::path::PathBuf, OpenOptions, Option<String>),
    request: u64,
    tx: std::sync::mpsc::Sender<AppEvent>,
) {
    std::thread::spawn(move || {
        let result = crate::vcs::reopen(&root, opts, None)
            .and_then(|v| v.working_status_in(path.as_deref()))
            .map_err(|e| format!("{e:#}"));
        let _ = tx.send(AppEvent::WorkingRow { request, result });
    });
}

/// The last log row while more commits may exist.
pub const MORE_ROW: &str = "… more";

/// Said when a search or filter finds nothing among the loaded commits
/// and more may exist.
pub const NOT_LOADED: &str = "… more not loaded";

/// A log row by identity: its commit ID, and whether it is the working
/// copy's row.
pub(super) type RowKey = (String, bool);

/// The log's paging state, owned by [`App`].
#[derive(Debug, Default)]
pub(super) struct Paging {
    /// Commits asked of the VCS by the last load.
    limit: usize,
    /// How many each page adds: the first limit.
    page: usize,
    /// The last load returned as many commits as it asked for.
    more: bool,
    /// The last page request number; a reload bumps it too.
    request: u64,
    /// The page being loaded.
    pending: Option<Pending>,
}

impl Paging {
    pub(super) fn new(limit: usize) -> Self {
        Paging {
            limit,
            page: limit,
            ..Paging::default()
        }
    }
}

#[derive(Debug)]
struct Pending {
    request: u64,
    limit: usize,
    replay: Replay,
}

/// The move that asked for a page, run again once the page is in: from
/// the row it started on, if the cursor it moved is still where it put it.
#[derive(Debug)]
enum Replay {
    None,
    Log(Move, Moved),
    List(Move, Moved),
}

#[derive(Debug, Clone, Copy)]
enum Move {
    By(isize),
    Bottom,
}

#[derive(Debug)]
struct Moved {
    from: Option<RowKey>,
    at: Option<RowKey>,
}

/// Read `limit` commits on a worker thread, as [`App::page_request`]
/// does without a sender.
fn spawn_page(
    (root, opts, pin): (std::path::PathBuf, OpenOptions, Option<String>),
    (revset, path, spine): (Option<String>, Option<String>, bool),
    (request, limit): (u64, usize),
    tx: std::sync::mpsc::Sender<AppEvent>,
) {
    std::thread::spawn(move || {
        let result = crate::vcs::reopen(&root, opts, pin)
            .and_then(|v| v.log_in(revset.as_deref(), path.as_deref(), limit, spine))
            .map_err(|e| format!("{e:#}"));
        let _ = tx.send(AppEvent::LogPage { request, result });
    });
}

/// The commits of `rows`, without synthetic rows (git's working copy, a
/// fold's `… more`).
fn infos(rows: &[Row]) -> Vec<CommitInfo> {
    rows.iter()
        .filter(|r| !is_synthetic(&r.info.commit))
        .map(|r| r.info.clone())
        .collect()
}

impl App {
    /// Read the log from the VCS, without jj's root commit; a zero parent
    /// counts as no parent. First brings the VCS up to date (`snapshot`:
    /// jj snapshots the working copy), then reads the working copy: git's
    /// changes, jj's `@`. Errors in either go to the status line.
    pub(super) fn load(&mut self, snapshot: bool) -> anyhow::Result<Vec<CommitInfo>> {
        self.refresh(snapshot);
        self.branch_refresh();
        self.load_refreshed()
    }

    /// Bring the VCS up to date (see [`App::load`]); an error goes to the
    /// status line.
    pub(super) fn refresh(&mut self, snapshot: bool) {
        if let Err(e) = self.vcs.refresh(snapshot) {
            self.set_status(format!("{e:#}"));
        }
    }

    /// [`App::load`] after the refresh and the branch read: the log and
    /// the working copy.
    pub(super) fn load_refreshed(&mut self) -> anyhow::Result<Vec<CommitInfo>> {
        let limit = self.page.limit;
        let commits = self.vcs.log_in(
            self.query.as_deref(),
            self.opts.path.as_deref(),
            limit,
            self.folding(),
        )?;
        self.folds_check_flat()?;
        let commits = self.trim(commits, limit);
        match self.vcs.kind() {
            Kind::Git if self.on_checked_out() => self.load_working(),
            Kind::Git => {
                // Another branch: its history has no uncommitted changes.
                self.working_request += 1;
                self.working = None;
            }
            Kind::Jj => match self.vcs.working_copy_commit() {
                Ok(id) => self.working_commit = id,
                Err(e) => {
                    self.working_commit = None;
                    self.set_status(format!("cannot resolve @: {e:#}"));
                }
            },
        }
        Ok(commits)
    }

    /// `commits` as asked with `limit`: notes whether more may exist (as
    /// many as asked, counted before jj's root commit is dropped), then
    /// drops zero IDs.
    fn trim(&mut self, mut commits: Vec<CommitInfo>, limit: usize) -> Vec<CommitInfo> {
        self.page.more = limit > 0 && commits.len() >= limit;
        commits.retain(|c| !is_zero_id(&c.commit));
        for c in &mut commits {
            c.parents.retain(|p| !is_zero_id(p));
        }
        commits
    }

    /// Back to the first page's size (a new query), dropping a page
    /// being loaded.
    pub(super) fn page_reset(&mut self) {
        self.page.request += 1;
        self.page.pending = None;
        self.page.limit = self.page.page;
    }

    /// More commits may exist past the loaded ones.
    pub fn log_more(&self) -> bool {
        self.page.more
    }

    /// How many commits a page adds (the first limit).
    pub(super) fn page_size(&self) -> usize {
        self.page.page
    }

    /// A page of the log is loading.
    pub fn loading_more(&self) -> bool {
        self.page.pending.is_some()
    }

    /// The `… more` row ends the log (and the commit list).
    pub fn more_row(&self) -> bool {
        self.page.more && !self.rows.is_empty()
    }

    /// Rows the log and the commit list draw, the `… more` row included.
    pub(super) fn log_len(&self) -> usize {
        self.rows.len() + usize::from(self.more_row())
    }

    /// The row `cursor` must keep on screen: the `… more` row under the
    /// last row.
    pub(super) fn bottom_of(&self, cursor: usize) -> usize {
        cursor + usize::from(self.more_row() && cursor == self.last_row())
    }

    /// Ask for the next page, unless none may exist or one is loading.
    /// `replay` runs once it is in.
    fn page_request(&mut self, replay: Replay) {
        if !self.page.more || self.page.pending.is_some() {
            return;
        }
        self.page.request += 1;
        let request = self.page.request;
        let limit = self.page.limit + self.page.page;
        self.page.pending = Some(Pending {
            request,
            limit,
            replay,
        });
        match self.sender.clone() {
            Some(tx) => {
                let at = (
                    self.vcs.root().to_path_buf(),
                    self.vcs.open_options(),
                    self.vcs.pinned_op(),
                );
                let q = (self.query.clone(), self.opts.path.clone(), self.folding());
                spawn_page(at, q, (request, limit), tx);
            }
            None => {
                let result = self
                    .vcs
                    .log_in(
                        self.query.as_deref(),
                        self.opts.path.as_deref(),
                        limit,
                        self.folding(),
                    )
                    .map_err(|e| format!("{e:#}"));
                self.page_loaded(request, result);
            }
        }
    }

    /// A page asked for by a move past the last row of the log (or of the
    /// commit list, `list`) that started on row `from`.
    fn page_past(&mut self, from: Option<RowKey>, mv: Move, list: bool) {
        let at = self.log_key(if list {
            self.commit_list.cursor
        } else {
            self.cursor
        });
        let moved = Moved { from, at };
        self.page_request(if list {
            Replay::List(mv, moved)
        } else {
            Replay::Log(mv, moved)
        });
    }

    /// The worker's page: kept if it answers the last request. The log
    /// cursor stays on its commit, the commit list's and the open commit
    /// on theirs; then the move that asked for it runs again.
    pub(super) fn page_loaded(&mut self, request: u64, result: Result<Vec<CommitInfo>, String>) {
        let Some(p) = self.page.pending.take_if(|p| p.request == request) else {
            return;
        };
        let commits = match result {
            Ok(c) => c,
            Err(e) => return self.set_status(format!("cannot load more: {e}")),
        };
        self.page.limit = p.limit;
        let commits = self.trim(commits, p.limit);
        let current = self.log_key(self.cursor);
        self.set_rows(commits);
        self.cursor_to(current);
        self.commit_reindex();
        self.replay(p.replay);
        self.keep_visible();
        self.commit_list_keep_visible();
    }

    fn replay(&mut self, replay: Replay) {
        match replay {
            Replay::None => {}
            Replay::Log(mv, m) => {
                if self.log_key(self.cursor) != m.at {
                    return;
                }
                self.cursor_to(m.from);
                match mv {
                    Move::By(d) => self.goto_row(self.cursor.saturating_add_signed(d)),
                    Move::Bottom => self.goto_row(self.last_row()),
                }
            }
            Replay::List(mv, m) => {
                if self.log_key(self.commit_list.cursor) != m.at {
                    return;
                }
                if let Some(i) = m.from.and_then(|k| self.row_of(&k)) {
                    self.commit_list.cursor = i;
                }
                match mv {
                    Move::By(d) => {
                        self.commit_list_goto(self.commit_list.cursor.saturating_add_signed(d));
                    }
                    Move::Bottom => self.commit_list_goto(usize::MAX),
                }
            }
        }
    }

    /// The commit list's cursor `delta` rows down (up when negative); past
    /// its last row, the next page.
    pub(super) fn commit_list_move(&mut self, delta: isize) {
        let from = self.log_key(self.commit_list.cursor);
        let to = self.commit_list.cursor.saturating_add_signed(delta);
        self.commit_list_goto(to);
        if to > self.last_row() {
            self.page_past(from, Move::By(delta), true);
        }
    }

    /// `G` in the commit list: row `count`, or the last row and then the
    /// next page.
    pub(super) fn commit_list_bottom(&mut self, count: Option<usize>) {
        let Some(c) = count else {
            let from = self.log_key(self.commit_list.cursor);
            self.commit_list_goto(usize::MAX);
            return self.page_past(from, Move::Bottom, true);
        };
        self.commit_list_goto(c.saturating_sub(1));
    }

    /// The wheel down at the end of the log or the commit list: the next
    /// page.
    pub(super) fn page_wheel(&mut self) {
        self.page_request(Replay::None);
    }

    /// git: re-read the working-copy row (on the worker when there is a
    /// sender: the row then changes when its [`AppEvent::WorkingRow`]
    /// arrives).
    fn load_working(&mut self) {
        self.working_request += 1;
        let request = self.working_request;
        match self.sender.clone() {
            Some(tx) => {
                let at = (
                    self.vcs.root().to_path_buf(),
                    self.vcs.open_options(),
                    self.opts.path.clone(),
                );
                spawn_working_status(at, request, tx);
            }
            None => {
                let result = self
                    .vcs
                    .working_status_in(self.opts.path.as_deref())
                    .map_err(|e| format!("{e:#}"));
                self.set_working(result);
            }
        }
    }

    /// The worker's working-copy row: kept if it answers the last request.
    pub(super) fn working_row_loaded(
        &mut self,
        request: u64,
        result: Result<Option<WorkingStatus>, String>,
    ) {
        if request != self.working_request {
            return;
        }
        self.set_working(result);
        self.working_rows();
        self.session_working_arrived(request);
    }

    /// git: a working-copy row read is in flight on a worker.
    pub(super) fn working_pending(&self) -> bool {
        self.vcs.kind() == Kind::Git && self.sender.is_some() && self.on_checked_out()
    }

    fn set_working(&mut self, result: Result<Option<WorkingStatus>, String>) {
        match result {
            Ok(w) => self.working = w.filter(|_| self.on_checked_out()),
            Err(e) => {
                self.working = None;
                self.set_status(format!("cannot read the working copy: {e}"));
            }
        }
        // Same count, other contents: the row's stat is read again.
        self.stat_forget(WORKING_ID);
    }

    /// git's synthetic row for `self.working`.
    pub(super) fn working_row(&self) -> Option<Row> {
        let w = self.working.as_ref()?;
        let time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64);
        Some(Row {
            info: CommitInfo {
                commit: WORKING_ID.to_string(),
                change: None,
                author: String::new(),
                time,
                summary: "Working copy".to_string(),
                parents: w.base.iter().map(|b| b.commit.clone()).collect(),
            },
            status: Status::Seen,
            working: true,
            depth: 0,
            group: None,
            groups: Vec::new(),
            fold: None,
            merge: None,
        })
    }

    /// The rows from the log query's `commits` (the spine while folding;
    /// see `fold`), open folds spliced in. Then loads what the new rows
    /// lack: open folds' contents and merge sizes.
    pub(super) fn set_rows(&mut self, commits: Vec<CommitInfo>) {
        self.all_rows = self.splice(commits);
        // A kept filter narrows the new list too; the caller places the
        // cursor. The commit list's cursor stays on its commit.
        let keep = self.rows.get(self.cursor).map(|r| r.info.commit.clone());
        let cursor = self.cursor;
        self.refilter(keep);
        self.cursor = cursor.min(self.rows.len().saturating_sub(1));
        self.folds_follow();
    }

    fn restatus(&mut self) {
        for r in self.rows.iter_mut().chain(self.all_rows.iter_mut()) {
            if !is_synthetic(&r.info.commit) {
                r.status = self.seen.status(&r.info);
            }
        }
    }

    /// The log query's commits (the spine while folding), for rebuilds:
    /// never the spliced rows.
    fn commits(&self) -> Vec<CommitInfo> {
        self.folds.spine.clone()
    }

    /// The oldest new or updated row (the log is newest first).
    pub(super) fn first_unseen(&self) -> Option<usize> {
        self.rows
            .iter()
            .rposition(|r| r.status != Status::Seen && !is_fold_more(&r.info.commit))
    }

    /// The log row of the working copy (git's row, jj's `@`), if listed.
    pub fn working_row_index(&self) -> Option<usize> {
        self.working_index()
    }

    pub(super) fn working_index(&self) -> Option<usize> {
        self.rows.iter().position(|r| r.working)
    }

    /// `r` (`snapshot`), a watcher event or a return from a launcher:
    /// re-read the log, keeping the cursor on the same commit when it is
    /// still listed (on the working copy when it was there).
    /// A page loading is dropped: the reload asks for its count.
    pub fn reload(&mut self, snapshot: bool) {
        if let Err(e) = self.reload_strict(snapshot) {
            self.set_status(format!("reload failed: {e:#}"));
        }
    }

    /// [`App::reload`], returning a failed read (the rows stay as they
    /// were).
    pub(super) fn reload_strict(&mut self, snapshot: bool) -> anyhow::Result<()> {
        self.stat_reload();
        self.page.request += 1;
        if let Some(p) = self.page.pending.take() {
            self.page.limit = p.limit;
        }
        let current = self.log_key(self.cursor);
        let commits = self.load(snapshot)?;
        self.set_rows(commits);
        self.cursor_to(current);
        self.commit_reindex();
        Ok(())
    }

    /// Under a path filter, whether one of `files` (a diff's) is at or
    /// under it; without one, whether there are any.
    pub(super) fn path_touched(&self, files: &[crate::vcs::FileDiff]) -> bool {
        let Some(p) = self.opts.path.as_deref() else {
            return !files.is_empty();
        };
        files.iter().any(|f| {
            [&f.new_path, &f.old_path]
                .into_iter()
                .flatten()
                .any(|f| crate::vcs::under(f, p))
        })
    }

    pub(super) fn log_key(&self, i: usize) -> Option<RowKey> {
        self.rows.get(i).map(|r| (r.info.commit.clone(), r.working))
    }

    fn cursor_to(&mut self, current: Option<RowKey>) {
        if let Some(i) = current.and_then(|c| self.row_of(&c)) {
            self.cursor = i;
        }
    }

    /// The row of a commit ID, or of the working copy when `working`.
    pub(super) fn row_of(&self, (id, working): &RowKey) -> Option<usize> {
        if *working {
            self.working_index()
        } else {
            self.rows.iter().position(|r| r.info.commit == *id)
        }
    }

    /// git's tree watcher: re-read only the working-copy row.
    pub(super) fn working_changed(&mut self) {
        if self.vcs.kind() != Kind::Git || !self.on_checked_out() {
            return;
        }
        // An open working-copy view follows the tree; its load updates
        // the row too.
        if self.working_follow() {
            return;
        }
        self.load_working();
        if self.sender.is_none() {
            self.working_rows();
        }
    }

    /// git: rebuild the rows around `self.working`, the cursor kept.
    pub(super) fn working_rows(&mut self) {
        let current = self
            .rows
            .get(self.cursor)
            .map(|r| (r.info.commit.clone(), r.working));
        let commits = self.commits();
        self.set_rows(commits);
        self.cursor_to(current);
        if !matches!(self.screen, Screen::Commit(_)) {
            return;
        }
        // An open view stays as it is; only its log index follows.
        self.commit_reindex();
    }

    /// `m`: every listed commit (the filtered list) is seen, with the
    /// loaded fold commits of the listed merges.
    pub(super) fn mark_all(&mut self) {
        let commits = self.with_bubbles(infos(&self.rows));
        match self.seen.mark_all(&commits) {
            Ok(()) => {
                self.restatus();
                self.set_status(format!("marked {} commits seen", commits.len()));
            }
            Err(e) => self.set_status(format!("cannot write seen state: {e}")),
        }
    }

    /// `w`: the watermark at log row `i`. That commit and every loaded
    /// commit older than it are seen, every newer one is new again, so
    /// the next start puts the cursor just above it. Positions come from
    /// the whole log (the spine while folding: a row inside a fold splits
    /// at its merge, and a merge's loaded fold commits go with it), not
    /// the filtered rows; git's working-copy row is skipped (on it,
    /// everything loaded is seen).
    pub fn set_watermark(&mut self, i: usize) {
        let Some(row) = self.rows.get(i) else { return };
        if is_fold_more(&row.info.commit) {
            return;
        }
        let spine = &self.folds.spine;
        let at = if row.info.commit == WORKING_ID {
            0
        } else {
            let Some(id) = self.spine_of(i) else { return };
            match spine.iter().position(|c| c.commit == id) {
                Some(at) => at,
                None => return,
            }
        };
        let unseen = self.with_bubbles(spine[..at].to_vec());
        let seen = self.with_bubbles(spine[at..].to_vec());
        match self.seen.set_watermark(&seen, &unseen) {
            Ok(()) => {
                self.restatus();
                self.set_status(format!(
                    "watermark set: {} newer unseen, {} seen",
                    unseen.len(),
                    seen.len()
                ));
            }
            Err(e) => self.set_status(format!("cannot write seen state: {e}")),
        }
    }

    /// Mark one row seen.
    pub fn mark_row(&mut self, i: usize) {
        let Some(info) = self.rows.get(i).map(|r| r.info.clone()) else {
            return;
        };
        if is_synthetic(&info.commit) {
            return;
        }
        match self.seen.mark(&info) {
            Ok(()) => self.restatus(),
            Err(e) => self.set_status(format!("cannot write seen state: {e}")),
        }
    }

    /// Read the bubbles of `merges` to mark them seen: on a worker with
    /// an event sender (one [`AppEvent::BubbleLoaded`] per merge, in
    /// order), inline without.
    pub(super) fn bubbles_request(&mut self, merges: Vec<String>) {
        if merges.is_empty() {
            return;
        }
        let Some(tx) = self.sender.clone() else {
            for merge in merges {
                let result = self
                    .vcs
                    .bubble(&merge, BUBBLE_CAP)
                    .map_err(|e| format!("{e:#}"));
                self.bubble_loaded(&merge, result);
            }
            return;
        };
        let (root, opts, pin) = (
            self.vcs.root().to_path_buf(),
            self.vcs.open_options(),
            self.vcs.pinned_op(),
        );
        std::thread::spawn(move || {
            let v = crate::vcs::reopen(&root, opts, pin);
            for merge in merges {
                let result = match &v {
                    Ok(v) => v.bubble(&merge, BUBBLE_CAP),
                    Err(e) => Err(anyhow::anyhow!("{e:#}")),
                };
                let result = result.map_err(|e| format!("{e:#}"));
                if tx.send(AppEvent::BubbleLoaded { merge, result }).is_err() {
                    return;
                }
            }
        });
    }

    /// A merge's bubble from [`App::bubbles_request`]: marked seen.
    pub(super) fn bubble_loaded(&mut self, merge: &str, result: Result<Vec<CommitInfo>, String>) {
        match result {
            Ok(commits) => self.mark_bubble(&commits),
            Err(e) => self.set_status(format!(
                "cannot mark {}'s commits seen: {e}",
                &merge[..merge.len().min(8)]
            )),
        }
    }

    /// Mark a merge's bubble (`commits`) seen.
    pub(super) fn mark_bubble(&mut self, commits: &[CommitInfo]) {
        if commits.is_empty() {
            return;
        }
        match self.seen.mark_all(commits) {
            Ok(()) => self.restatus(),
            Err(e) => self.set_status(format!("cannot write seen state: {e}")),
        }
    }

    /// `Enter`: open the commit under the cursor (marks it seen).
    pub(super) fn open_commit(&mut self) {
        if self.rows.is_empty() {
            return;
        }
        self.open_index(self.cursor);
    }

    pub(super) fn last_row(&self) -> usize {
        self.rows.len().saturating_sub(1)
    }

    pub(super) fn goto_row(&mut self, row: usize) {
        self.cursor = row.min(self.last_row());
    }

    /// `j` / `k`; past the last row, the next page.
    pub(super) fn vertical(&mut self, delta: isize) {
        let from = self.log_key(self.cursor);
        let to = self.cursor.saturating_add_signed(delta);
        self.goto_row(to);
        if to > self.last_row() {
            self.page_past(from, Move::By(delta), false);
        }
    }

    /// `G`: row `count`, or the last row and then the next page.
    pub(super) fn goto_bottom(&mut self, count: Option<usize>) {
        let Some(c) = count else {
            let from = self.log_key(self.cursor);
            self.goto_row(self.last_row());
            return self.page_past(from, Move::Bottom, false);
        };
        self.goto_row(c.saturating_sub(1));
    }

    /// `C-d` / `C-u`; down past the last row, the next page.
    pub(super) fn half_page(&mut self, down: bool) {
        let half = (self.view_h / 2).max(1) as isize;
        let delta = if down { half } else { -half };
        let from = self.log_key(self.cursor);
        let past = self.cursor.saturating_add_signed(delta) > self.last_row();
        self.goto_row(self.cursor.saturating_add_signed(delta));
        self.scroll = self.scroll.saturating_add_signed(delta);
        self.keep_visible();
        if down && past {
            self.page_past(from, Move::By(delta), false);
        }
    }

    /// Reload the log after its query changed (the path filter, the
    /// branch). The page size goes back to the first page's; the cursor
    /// stays on its commit when it is still listed, else goes to the first
    /// row; an open commit the new query drops stays open, and `]]` / `[[`
    /// step from its old place. On `Err` the rows and the working copy are
    /// as before; the caller puts its query back.
    pub(super) fn requery(&mut self) -> anyhow::Result<()> {
        let at = self.cursor_id();
        let before: Vec<String> = self
            .all_rows
            .iter()
            .map(|r| r.info.commit.clone())
            .collect();
        self.page_reset();
        self.folds_requery();
        let working = self.working.take();
        if let Err(e) = self.reload_strict(false) {
            self.working = working;
            return Err(e);
        }
        if at.is_none_or(|id| !self.rows.iter().any(|r| r.info.commit == id)) {
            self.cursor = 0;
        }
        self.commit_reindex();
        if let Some(v) = &self.commit
            && matches!(self.screen, Screen::Commit(_))
            && self.view_index(v).is_none()
        {
            self.hidden_open = Some((v.commit.clone(), before));
        }
        self.keep_visible();
        Ok(())
    }

    /// Scroll so the cursor row is on screen (on the last row, the
    /// `… more` row under it too).
    pub(super) fn keep_visible(&mut self) {
        let h = self.view_h.max(1);
        let max_scroll = self.log_len().saturating_sub(h);
        let bottom = self.bottom_of(self.cursor).min(self.cursor + h - 1);
        if self.cursor < self.scroll {
            self.scroll = self.cursor;
        } else if bottom >= self.scroll + h {
            self.scroll = bottom + 1 - h;
        }
        self.scroll = self.scroll.min(max_scroll);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ages() {
        assert_eq!(age(100, 100), "0s");
        assert_eq!(age(100, 200), "0s", "future is now");
        assert_eq!(age(59, 0), "59s");
        assert_eq!(age(3 * 60 + 5, 0), "3m");
        assert_eq!(age(2 * 3600, 0), "2h");
        assert_eq!(age(5 * 86_400 + 7, 0), "5d");
        assert_eq!(age(3 * 31_536_000, 0), "3y");
    }

    #[test]
    fn short_ids() {
        let mut c = CommitInfo {
            commit: "0123456789abcdef".into(),
            change: None,
            author: String::new(),
            time: 0,
            summary: String::new(),
            parents: vec![],
        };
        assert_eq!(short_id(&c), "01234567");
        c.change = Some("zyxwvutsrqpo".into());
        assert_eq!(short_id(&c), "zyxwvuts");
        c.change = Some("abc".into());
        assert_eq!(short_id(&c), "abc");
    }

    #[test]
    fn zero_ids() {
        assert!(is_zero_id(&"0".repeat(40)));
        assert!(!is_zero_id(""));
        assert!(!is_zero_id("00a0"));
    }
}
