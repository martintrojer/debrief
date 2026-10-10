//! The log's stats pane: the commit under the
//! cursor as `git log --stat` shows it.
//!
//! A stat loads on a worker thread (`vcs::reopen(...).stat(id)`, like the
//! commit loader) once the cursor has rested on a commit for
//! [`STAT_DELAY`], tagged with a request number; an answer for an older
//! request is dropped. Results are kept per commit ID, the
//! [`CACHE_SIZE`] most recently used. git's working-copy row loads the
//! same way, from the working copy's diff in hunks; each new answer for
//! the row drops its stat, and a working copy opened in the commit view
//! leaves its stat behind. With no event sender (unit tests) a stat loads
//! in [`App::tick`].
//!
//! `J` / `K` (and the wheel over the pane, when the mouse is captured)
//! scroll the pane; the scroll belongs to the commit under the cursor, so
//! it starts at the top again on another commit.

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use ratatui::layout::{Constraint, Layout, Rect};

use crate::vcs::{CommitStat, Context, Vcs, WorkingCopy, file_stats};

use super::{App, AppEvent, Screen, WORKING_ID};

/// How long the cursor rests on a commit before its stat loads.
pub const STAT_DELAY: Duration = Duration::from_millis(100);
/// Stats kept, most recently used first.
pub const CACHE_SIZE: usize = 256;
/// From this terminal width the pane sits beside the log, else below it.
pub const SIDE_BY_SIDE_MIN: u16 = 120;
/// Fewest rows the stacked pane gets (border included).
pub const STACKED_MIN_H: u16 = 5;

/// Where the log screen's panes go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LogLayout {
    pub log: Rect,
    /// The stats pane, its border (left or top edge) included.
    pub stats: Option<Rect>,
    /// The pane is beside the log (else below it).
    pub side_by_side: bool,
}

/// What the stats pane shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatPane {
    Loading,
    Error(String),
    Ready(Rc<CommitStat>),
}

/// Stats-pane state owned by [`App`].
#[derive(Debug, Default)]
pub(super) struct StatState {
    /// `<leader>e` on the log.
    pub(super) on: bool,
    /// Most recently used first.
    cache: Vec<(String, Rc<CommitStat>)>,
    /// The commit under the cursor and since when it has been there.
    resting: Option<(String, Instant)>,
    /// The last request and its commit.
    request: u64,
    inflight: Option<(u64, String)>,
    /// The last failure, for its commit.
    error: Option<(String, String)>,
    /// Lines scrolled, for the commit (row ID) they were scrolled on.
    scroll: Option<(String, usize)>,
    /// The furthest the pane can scroll, as last drawn.
    max_scroll: Cell<usize>,
}

impl StatState {
    fn get(&mut self, id: &str) -> Option<Rc<CommitStat>> {
        let i = self.cache.iter().position(|(c, _)| c == id)?;
        let e = self.cache.remove(i);
        let s = e.1.clone();
        self.cache.insert(0, e);
        Some(s)
    }

    fn peek(&self, id: &str) -> Option<Rc<CommitStat>> {
        self.cache
            .iter()
            .find(|(c, _)| c == id)
            .map(|(_, s)| s.clone())
    }

    fn put(&mut self, id: String, s: CommitStat) {
        self.cache.retain(|(c, _)| *c != id);
        self.cache.insert(0, (id, Rc::new(s)));
        self.cache.truncate(CACHE_SIZE);
    }
}

/// Split the log screen's main area (above the status line).
pub fn log_layout(area: Rect, stats: bool) -> LogLayout {
    let none = LogLayout {
        log: area,
        stats: None,
        side_by_side: false,
    };
    if !stats {
        return none;
    }
    if area.width >= SIDE_BY_SIDE_MIN {
        let [log, pane] =
            Layout::horizontal([Constraint::Percentage(60), Constraint::Percentage(40)])
                .areas(area);
        return LogLayout {
            log,
            stats: Some(pane),
            side_by_side: true,
        };
    }
    let h = (area.height * 2 / 5).max(STACKED_MIN_H);
    // Keep a few log rows; a terminal too short for both shows the log.
    if area.height < h + 3 {
        return none;
    }
    let [log, pane] = Layout::vertical([Constraint::Min(0), Constraint::Length(h)]).areas(area);
    LogLayout {
        log,
        stats: Some(pane),
        side_by_side: false,
    }
}

impl App {
    /// The log screen's panes for `area`.
    pub fn log_layout(&self, area: Rect) -> LogLayout {
        log_layout(area, self.stats_visible())
    }

    /// The stats pane is on and the log is showing.
    pub fn stats_visible(&self) -> bool {
        self.stat.on && self.screen == Screen::Log
    }

    /// `<leader>e` on the log.
    pub(super) fn toggle_stats(&mut self) {
        self.stat.on = !self.stat.on;
    }

    /// Lines the stats pane is scrolled for the cursor row.
    pub fn stat_scroll(&self) -> usize {
        match (&self.stat.scroll, self.rows.get(self.cursor)) {
            (Some((id, n)), Some(r)) if *id == r.info.commit => *n,
            _ => 0,
        }
    }

    /// Forget the scroll once the cursor is on another row.
    pub(super) fn stat_follow(&mut self) {
        let here = self.rows.get(self.cursor).map(|r| r.info.commit.as_str());
        if self.stat.scroll.as_ref().map(|(id, _)| id.as_str()) != here {
            self.stat.scroll = None;
        }
    }

    /// The drawn pane can scroll `n` lines (set by the renderer).
    pub fn set_stat_max_scroll(&self, n: usize) {
        self.stat.max_scroll.set(n);
    }

    /// `J` / `K`: scroll the stats pane `by` lines.
    pub(super) fn stat_scroll_by(&mut self, by: isize) {
        let Some(id) = self.rows.get(self.cursor).map(|r| r.info.commit.clone()) else {
            return;
        };
        if !self.stats_visible() {
            return;
        }
        let n = self
            .stat_scroll()
            .saturating_add_signed(by)
            .min(self.stat.max_scroll.get());
        self.stat.scroll = Some((id, n));
    }

    /// The mouse wheel at (`col`, `row`): scrolls the stats pane when over it.
    pub fn stat_wheel(&mut self, col: u16, row: u16, down: bool) {
        let (w, h) = self.size;
        let main = Rect::new(0, 0, w, h.saturating_sub(1));
        let over = self
            .log_layout(main)
            .stats
            .is_some_and(|p| p.contains(ratatui::layout::Position::new(col, row)));
        if over {
            self.stat_scroll_by(if down { 3 } else { -3 });
        }
    }

    /// The commit under the log cursor ([`WORKING_ID`] on git's
    /// working-copy row).
    fn cursor_commit(&self) -> Option<&str> {
        self.rows.get(self.cursor).map(|r| r.info.commit.as_str())
    }

    /// What the pane shows for the cursor row; `None` with no rows.
    pub fn stat_pane(&self) -> Option<StatPane> {
        let row = self.rows.get(self.cursor)?;
        let id = row.info.commit.as_str();
        if let Some(s) = self.stat.peek(id) {
            return Some(StatPane::Ready(s));
        }
        if let Some((c, e)) = &self.stat.error
            && c == id
        {
            return Some(StatPane::Error(e.clone()));
        }
        Some(StatPane::Loading)
    }

    /// Time passes: once the cursor has rested on a commit for
    /// [`STAT_DELAY`], load its stat unless it is cached or loading.
    pub(super) fn stat_tick(&mut self, now: Instant) {
        if !self.stats_visible() {
            self.stat.resting = None;
            return;
        }
        let Some(id) = self.cursor_commit().map(str::to_string) else {
            self.stat.resting = None;
            return;
        };
        let since = match &self.stat.resting {
            Some((c, t)) if *c == id => *t,
            _ => {
                self.stat.resting = Some((id, now));
                return;
            }
        };
        if now.saturating_duration_since(since) < STAT_DELAY {
            return;
        }
        let busy = self.stat.inflight.as_ref().is_some_and(|(_, c)| *c == id);
        let failed = self.stat.error.as_ref().is_some_and(|(c, _)| *c == id);
        if busy || failed || self.stat.get(&id).is_some() {
            return;
        }
        self.stat.request += 1;
        let request = self.stat.request;
        self.stat.inflight = Some((request, id.clone()));
        match self.sender.clone() {
            Some(tx) => {
                let (root, opts, pin) = (
                    self.vcs.root().to_path_buf(),
                    self.vcs.open_options(),
                    self.vcs.pinned_op(),
                );
                std::thread::spawn(move || {
                    let result = crate::vcs::reopen(&root, opts, pin)
                        .and_then(|v| stat_of(v.as_ref(), &id))
                        .map_err(|e| format!("{e:#}"));
                    let _ = tx.send(AppEvent::Stat { request, result });
                });
            }
            None => {
                let result = stat_of(self.vcs.as_ref(), &id).map_err(|e| format!("{e:#}"));
                self.stat_loaded(request, result);
            }
        }
    }

    /// A worker's stat: kept if it answers the last request.
    pub(super) fn stat_loaded(&mut self, request: u64, result: Result<CommitStat, String>) {
        let Some((_, id)) = self.stat.inflight.take_if(|(r, _)| *r == request) else {
            return;
        };
        match result {
            Ok(s) => self.stat.put(id, s),
            Err(e) => self.stat.error = Some((id, e)),
        }
    }

    /// Drop `id`'s stat, cached, loading or failed: it is read again.
    pub(super) fn stat_forget(&mut self, id: &str) {
        self.stat.cache.retain(|(c, _)| c != id);
        self.stat.inflight.take_if(|(_, c)| c == id);
        self.stat.error.take_if(|(c, _)| c == id);
    }

    /// The working copy the commit view read: the row's stat.
    pub(super) fn stat_put_working(&mut self, w: &WorkingCopy) {
        self.stat_forget(WORKING_ID);
        self.stat.put(WORKING_ID.to_string(), working_stat(w));
    }

    /// A reload: failed stats are tried again.
    pub(super) fn stat_reload(&mut self) {
        self.stat.error = None;
    }

    /// Stats cached now (tests).
    pub fn stat_cached(&self) -> usize {
        self.stat.cache.len()
    }
}

/// `id`'s stat; git's working copy's from its diff in hunks.
fn stat_of(vcs: &dyn Vcs, id: &str) -> anyhow::Result<CommitStat> {
    if id != WORKING_ID {
        return vcs.stat(id);
    }
    match vcs.working_copy(Context::Default)? {
        Some(w) => Ok(working_stat(&w)),
        None => anyhow::bail!("The working copy is clean"),
    }
}

/// The working copy as a stat (the pane shows its own header for it).
fn working_stat(w: &WorkingCopy) -> CommitStat {
    CommitStat {
        message: String::new(),
        author: String::new(),
        email: String::new(),
        time: 0,
        files: file_stats(&w.diff.files),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stat() -> CommitStat {
        CommitStat {
            message: String::new(),
            author: String::new(),
            email: String::new(),
            time: 0,
            files: Vec::new(),
        }
    }

    #[test]
    fn cache_drops_the_least_recently_used() {
        let mut s = StatState::default();
        for i in 0..CACHE_SIZE {
            s.put(i.to_string(), stat());
        }
        assert!(s.get("0").is_some());
        s.put("new".into(), stat());
        assert_eq!(s.cache.len(), CACHE_SIZE);
        assert!(s.peek("0").is_some(), "used recently");
        assert!(s.peek("1").is_none(), "least recently used");
    }

    #[test]
    fn layout_side_by_side_from_120_columns() {
        let l = log_layout(Rect::new(0, 0, 120, 30), true);
        assert!(l.side_by_side);
        assert_eq!(l.log.width, 72);
        assert_eq!(l.stats.unwrap().width, 48);
        let l = log_layout(Rect::new(0, 0, 119, 30), true);
        assert!(!l.side_by_side);
        assert_eq!(l.stats.unwrap().height, 12);
        assert_eq!(l.log.height, 18);
        let l = log_layout(Rect::new(0, 0, 80, 10), true);
        assert_eq!(l.stats.unwrap().height, STACKED_MIN_H);
        assert_eq!(log_layout(Rect::new(0, 0, 80, 7), true).stats, None);
        assert_eq!(log_layout(Rect::new(0, 0, 200, 30), false).stats, None);
    }
}
