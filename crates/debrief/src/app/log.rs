//! The log screen: loading and reloading the commit list, its cursor
//! motions, and marking commits seen.

use crate::seen::Status;
use crate::vcs::CommitInfo;

use super::{App, Row};

/// The commit ID of jj's root commit (and any all-zero ID).
pub fn is_zero_id(id: &str) -> bool {
    !id.is_empty() && id.bytes().all(|b| b == b'0')
}

/// The short ID shown in the log: the change ID for jj, else the commit
/// ID, first 8 characters.
pub fn short_id(c: &CommitInfo) -> &str {
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

impl App {
    /// Read the log from the VCS, without jj's root commit; a zero parent
    /// counts as no parent.
    pub(super) fn load(&self) -> anyhow::Result<Vec<CommitInfo>> {
        let mut commits = self.vcs.log(self.opts.revset.as_deref(), self.opts.limit)?;
        commits.retain(|c| !is_zero_id(&c.commit));
        for c in &mut commits {
            c.parents.retain(|p| !is_zero_id(p));
        }
        Ok(commits)
    }

    pub(super) fn set_rows(&mut self, commits: Vec<CommitInfo>) {
        self.rows = commits
            .into_iter()
            .map(|info| Row {
                status: self.seen.status(&info),
                info,
            })
            .collect();
        self.cursor = self.cursor.min(self.rows.len().saturating_sub(1));
    }

    fn restatus(&mut self) {
        for r in &mut self.rows {
            r.status = self.seen.status(&r.info);
        }
    }

    /// The oldest new or updated row (the log is newest first).
    pub(super) fn first_unseen(&self) -> Option<usize> {
        self.rows.iter().rposition(|r| r.status != Status::Seen)
    }

    /// `r` or a watcher event: re-read the log, keeping the cursor on the
    /// same commit when it is still listed.
    pub fn reload(&mut self) {
        let current = self.rows.get(self.cursor).map(|r| r.info.commit.clone());
        match self.load() {
            Ok(commits) => {
                self.set_rows(commits);
                if let Some(id) = current
                    && let Some(i) = self.rows.iter().position(|r| r.info.commit == id)
                {
                    self.cursor = i;
                }
                self.commit_reindex();
            }
            Err(e) => self.set_status(format!("reload failed: {e:#}")),
        }
    }

    /// `m`: every listed commit is seen.
    pub(super) fn mark_all(&mut self) {
        let commits: Vec<CommitInfo> = self.rows.iter().map(|r| r.info.clone()).collect();
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
