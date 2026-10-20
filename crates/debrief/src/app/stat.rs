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
//!
//! Side by side (from [`SIDE_BY_SIDE_MIN`] columns) the log is as wide as
//! most loaded rows need ([`App::log_fit`], bounded by [`log_width`]) and
//! the pane takes the rest; stacked, the pane takes 40% of the rows, half
//! from [`STACKED_HALF_FROM`].

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
/// Side by side, the log's narrowest (60% of [`SIDE_BY_SIDE_MIN`]).
pub const LOG_MIN_W: u16 = 72;
/// Side by side, the log's widest share of the terminal, in percent.
pub const LOG_MAX_PCT: u32 = 60;
/// A log row's columns before its summary: marker, ID, age, author.
pub const LOG_ROW_PREFIX: u16 = 29;
/// From this many rows the stacked pane takes half the height, not 40%:
/// the log keeps 20 rows or more, and the pane's message and files get
/// the rest.
pub const STACKED_HALF_FROM: u16 = 40;

/// Where the log screen's panes go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LogLayout {
    /// The branch and path filter row across the top.
    pub chip: Option<Rect>,
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

/// The log's width beside the stats pane: `fit` (what most rows need,
/// [`App::log_fit`]) within [`LOG_MIN_W`] and [`LOG_MAX_PCT`] of `cols`;
/// the pane gets the rest. 60% at 120 columns, and on a wide terminal
/// the log stops at its rows so the pane takes the room.
pub fn log_width(fit: u16, cols: u16) -> u16 {
    let max = (u32::from(cols) * LOG_MAX_PCT / 100) as u16;
    fit.max(LOG_MIN_W).min(max)
}

/// Split the log screen's main area (above the status line); `fit` is
/// the log's width side by side before [`log_width`] bounds it.
pub fn log_layout(area: Rect, stats: bool, fit: u16) -> LogLayout {
    let none = LogLayout {
        chip: None,
        log: area,
        stats: None,
        side_by_side: false,
    };
    if !stats {
        return none;
    }
    if area.width >= SIDE_BY_SIDE_MIN {
        let [log, pane] = Layout::horizontal([
            Constraint::Length(log_width(fit, area.width)),
            Constraint::Min(0),
        ])
        .areas(area);
        return LogLayout {
            chip: None,
            log,
            stats: Some(pane),
            side_by_side: true,
        };
    }
    let share = if area.height >= STACKED_HALF_FROM {
        area.height / 2
    } else {
        area.height * 2 / 5
    };
    let h = share.max(STACKED_MIN_H);
    // Keep a few log rows; a terminal too short for both shows the log.
    if area.height < h + 3 {
        return none;
    }
    let [log, pane] = Layout::vertical([Constraint::Min(0), Constraint::Length(h)]).areas(area);
    LogLayout {
        chip: None,
        log,
        stats: Some(pane),
        side_by_side: false,
    }
}

impl App {
    /// The log screen's panes for `area`.
    /// The branch (and path filter) row takes the top row (when there are
    /// rows to spare).
    pub fn log_layout(&self, area: Rect) -> LogLayout {
        let fit = if self.stats_visible() && area.width >= SIDE_BY_SIDE_MIN {
            self.log_fit()
        } else {
            0
        };
        // The branch row is always there (when there are rows to spare).
        let (chip, area) = match () {
            () if area.height > 1 => (
                Some(Rect { height: 1, ..area }),
                Rect {
                    y: area.y + 1,
                    height: area.height - 1,
                    ..area
                },
            ),
            _ => (None, area),
        };
        LogLayout {
            chip,
            ..log_layout(area, self.stats_visible(), fit)
        }
    }

    /// The columns most log rows need: the 90th percentile of the loaded
    /// rows (all of them, so a filter doesn't move the split), plus one
    /// column of padding, as the file list's auto width does.
    pub fn log_fit(&self) -> u16 {
        use unicode_width::UnicodeWidthStr;
        let need: Vec<u16> = self
            .all_rows
            .iter()
            .map(|r| {
                let summary = r.info.summary.width().min(usize::from(u16::MAX)) as u16;
                LOG_ROW_PREFIX.saturating_add(summary)
            })
            .collect();
        super::sidebar::fit_width(&need)
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

    /// `J` / `K` and the wheel: scroll the stats pane `by` lines.
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
        let l = log_layout(Rect::new(0, 0, 120, 30), true, 100);
        assert!(l.side_by_side);
        assert_eq!(l.log.width, 72);
        assert_eq!(l.stats.unwrap().width, 48);
        let l = log_layout(Rect::new(0, 0, 119, 30), true, 100);
        assert!(!l.side_by_side);
        assert_eq!(l.stats.unwrap().height, 12);
        assert_eq!(l.log.height, 18);
        let l = log_layout(Rect::new(0, 0, 80, 10), true, 0);
        assert_eq!(l.stats.unwrap().height, STACKED_MIN_H);
        assert_eq!(log_layout(Rect::new(0, 0, 80, 7), true, 0).stats, None);
        assert_eq!(log_layout(Rect::new(0, 0, 200, 30), false, 0).stats, None);
    }

    #[test]
    fn wide_terminals_give_the_stats_pane_the_room() {
        // The log's rows need 100: at 250 columns the pane gets 150.
        let l = log_layout(Rect::new(0, 0, 250, 30), true, 100);
        assert_eq!((l.log.width, l.stats.unwrap().width), (100, 150));
        // Short rows still get LOG_MIN_W; long ones stop at 60%.
        assert_eq!(log_width(40, 250), LOG_MIN_W);
        assert_eq!(log_width(400, 250), 150);
        // 60% wins over the floor just above the side-by-side cutoff.
        assert_eq!(log_width(0, 120), 72);
        assert_eq!(log_width(100, 150), 90);
    }

    #[test]
    fn tall_stacked_terminals_split_in_half() {
        let l = log_layout(Rect::new(0, 0, 100, 39), true, 0);
        assert_eq!((l.log.height, l.stats.unwrap().height), (24, 15));
        let l = log_layout(Rect::new(0, 0, 100, 60), true, 0);
        assert_eq!((l.log.height, l.stats.unwrap().height), (30, 30));
    }
}
