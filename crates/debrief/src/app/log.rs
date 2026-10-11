//! The log screen: loading and reloading the commit list, its cursor
//! motions, and marking commits seen.
//!
//! The working copy: git gets a synthetic row on
//! top while the tree is dirty, never seen or new; jj marks the row of
//! this workspace's `@`. Loading snapshots jj first; a snapshot
//! error goes to the status line and the log loads from the last pin.
//!
//! git's row (dirty or not, and its file count) comes from
//! [`Vcs::working_status`](crate::vcs::Vcs::working_status), never the
//! full diff; with an event sender it is read on a worker thread and
//! comes back as a request-tagged [`AppEvent::WorkingRow`]; an answer to
//! an older request is dropped.

use crate::seen::Status;
use crate::vcs::{CommitInfo, Kind, OpenOptions, WorkingStatus};

use super::{App, AppEvent, Row, Screen};

/// The commit ID of git's synthetic working-copy row (not a hex ID, so it
/// never names a commit).
pub const WORKING_ID: &str = "working-copy";

/// The commit ID of jj's root commit (and any all-zero ID).
pub fn is_zero_id(id: &str) -> bool {
    !id.is_empty() && id.bytes().all(|b| b == b'0')
}

/// The short ID shown in the log: the change ID for jj, else the commit
/// ID, first 8 characters.
pub fn short_id(c: &CommitInfo) -> &str {
    if c.commit == WORKING_ID {
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
    (root, opts): (std::path::PathBuf, OpenOptions),
    request: u64,
    tx: std::sync::mpsc::Sender<AppEvent>,
) {
    std::thread::spawn(move || {
        let result = crate::vcs::reopen(&root, opts, None)
            .and_then(|v| v.working_status())
            .map_err(|e| format!("{e:#}"));
        let _ = tx.send(AppEvent::WorkingRow { request, result });
    });
}

fn infos(rows: &[Row]) -> Vec<CommitInfo> {
    rows.iter()
        .filter(|r| r.info.commit != WORKING_ID)
        .map(|r| r.info.clone())
        .collect()
}

impl App {
    /// Read the log from the VCS, without jj's root commit; a zero parent
    /// counts as no parent. First brings the VCS up to date (`snapshot`:
    /// jj snapshots the working copy), then reads the working copy: git's
    /// changes, jj's `@`. Errors in either go to the status line.
    pub(super) fn load(&mut self, snapshot: bool) -> anyhow::Result<Vec<CommitInfo>> {
        if let Err(e) = self.vcs.refresh(snapshot) {
            self.set_status(format!("{e:#}"));
        }
        let mut commits = self.vcs.log(self.opts.revset.as_deref(), self.opts.limit)?;
        commits.retain(|c| !is_zero_id(&c.commit));
        for c in &mut commits {
            c.parents.retain(|p| !is_zero_id(p));
        }
        match self.vcs.kind() {
            Kind::Git => self.load_working(),
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

    /// git: re-read the working-copy row (on the worker when there is a
    /// sender: the row then changes when its [`AppEvent::WorkingRow`]
    /// arrives).
    fn load_working(&mut self) {
        self.working_request += 1;
        let request = self.working_request;
        match self.sender.clone() {
            Some(tx) => {
                let at = (self.vcs.root().to_path_buf(), self.vcs.open_options());
                spawn_working_status(at, request, tx);
            }
            None => {
                let result = self.vcs.working_status().map_err(|e| format!("{e:#}"));
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
    }

    fn set_working(&mut self, result: Result<Option<WorkingStatus>, String>) {
        match result {
            Ok(w) => self.working = w,
            Err(e) => {
                self.working = None;
                self.set_status(format!("cannot read the working copy: {e}"));
            }
        }
        // Same count, other contents: the row's stat is read again.
        self.stat_forget(WORKING_ID);
    }

    /// git's synthetic row for `self.working`.
    fn working_row(&self) -> Option<Row> {
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
        })
    }

    pub(super) fn set_rows(&mut self, commits: Vec<CommitInfo>) {
        let mut rows: Vec<Row> = self.working_row().into_iter().collect();
        rows.extend(commits.into_iter().map(|info| Row {
            status: self.seen.status(&info),
            working: self.working_commit.as_deref() == Some(info.commit.as_str()),
            info,
        }));
        self.all_rows = rows;
        // A kept filter narrows the new list too; the caller places the
        // cursor. The commit list's cursor stays on its commit.
        let keep = self.rows.get(self.cursor).map(|r| r.info.commit.clone());
        let listed = self
            .rows
            .get(self.commit_list.cursor)
            .map(|r| (r.info.commit.clone(), r.working));
        let cursor = self.cursor;
        self.refilter(keep);
        self.cursor = cursor.min(self.rows.len().saturating_sub(1));
        if let Some(i) = listed.and_then(|c| self.row_of(&c)) {
            self.commit_list.cursor = i;
        }
    }

    fn restatus(&mut self) {
        for r in self.rows.iter_mut().chain(self.all_rows.iter_mut()) {
            if r.info.commit != WORKING_ID {
                r.status = self.seen.status(&r.info);
            }
        }
    }

    /// The loaded commits, without git's synthetic row.
    fn commits(&self) -> Vec<CommitInfo> {
        infos(&self.all_rows)
    }

    /// The oldest new or updated row (the log is newest first).
    pub(super) fn first_unseen(&self) -> Option<usize> {
        self.rows.iter().rposition(|r| r.status != Status::Seen)
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
    pub fn reload(&mut self, snapshot: bool) {
        self.stat_reload();
        let current = self
            .rows
            .get(self.cursor)
            .map(|r| (r.info.commit.clone(), r.working));
        match self.load(snapshot) {
            Ok(commits) => {
                self.set_rows(commits);
                self.cursor_to(current);
                self.commit_reindex();
            }
            Err(e) => self.set_status(format!("reload failed: {e:#}")),
        }
    }

    fn cursor_to(&mut self, current: Option<(String, bool)>) {
        if let Some(i) = current.and_then(|c| self.row_of(&c)) {
            self.cursor = i;
        }
    }

    /// The row of a commit ID, or of the working copy when `working`.
    fn row_of(&self, (id, working): &(String, bool)) -> Option<usize> {
        if *working {
            self.working_index()
        } else {
            self.rows.iter().position(|r| r.info.commit == *id)
        }
    }

    /// git's tree watcher: re-read only the working-copy row.
    pub(super) fn working_changed(&mut self) {
        if self.vcs.kind() != Kind::Git {
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

    /// `m`: every listed commit (the filtered list) is seen.
    pub(super) fn mark_all(&mut self) {
        let commits = infos(&self.rows);
        match self.seen.mark_all(&commits) {
            Ok(()) => {
                self.restatus();
                self.set_status(format!("marked {} commits seen", commits.len()));
            }
            Err(e) => self.set_status(format!("cannot write seen state: {e}")),
        }
    }

    /// Mark one row seen.
    pub fn mark_row(&mut self, i: usize) {
        let Some(info) = self.rows.get(i).map(|r| r.info.clone()) else {
            return;
        };
        if info.commit == WORKING_ID {
            return;
        }
        match self.seen.mark(&info) {
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

    pub(super) fn vertical(&mut self, delta: isize) {
        let to = self.cursor.saturating_add_signed(delta);
        self.goto_row(to);
    }

    pub(super) fn half_page(&mut self, down: bool) {
        let half = (self.view_h / 2).max(1) as isize;
        let delta = if down { half } else { -half };
        self.vertical(delta);
        self.scroll = self.scroll.saturating_add_signed(delta);
        self.keep_visible();
    }

    /// Scroll so the cursor row is on screen.
    pub(super) fn keep_visible(&mut self) {
        let h = self.view_h.max(1);
        let max_scroll = self.rows.len().saturating_sub(h);
        if self.cursor < self.scroll {
            self.scroll = self.cursor;
        } else if self.cursor >= self.scroll + h {
            self.scroll = self.cursor + 1 - h;
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
