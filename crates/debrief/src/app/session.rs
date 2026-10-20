//! Saved review sessions in the app:
//! `<leader>ss` saves the open commit's comments and view;
//! opening a commit with a session restores its view (and, until that
//! succeeds once this run, its unsent comments into the batch); the log
//! marks such rows `N`; quitting with unsaved review work asks first;
//! `<leader>rs` lists the sessions.
//!
//! A session file is re-read before its comments are restored, before a
//! save merges its sent comments, and before a send or copy marks
//! comments sent: another debrief may have written it since.
//!
//! Every commit visited this run has a [`Visit`]: its view as last seen,
//! whether its session's comments are in the batch, and a dirty bit. The
//! bit is set by user actions on the commit (motions, `T`, `t`,
//! `<leader>e`, adding, editing or removing its comments) and cleared by
//! saving it and by a successful send or copy of the batch.
//! Quitting warns about a dirty commit that has a session or comments, and
//! about a commit whose comments predate this run and that has no session.
//! Editing or removing a comment in the `<leader>rl` list visits the
//! commits it belongs to that have a session, restoring their comments
//! first.

use std::collections::HashMap;
use std::io;

use debrief_review::{Comment, Review, Side};

use super::commit::Focus;
use super::keys::{Action, KeyResult, plain};
use super::{App, Mode, Screen};
use crate::session::{
    self, SavedComment, Session, Store, View, ViewContext, ViewFiles, ViewFocus, ViewLayout,
    WORKING,
};
use crate::vcs::{Context, Kind};

/// The log marker of a commit with a session.
pub const SESSION_MARKER: char = '\u{f02e}'; // nf-fa-bookmark
/// The quit prompt, in the status row.
pub const QUIT_PROMPT: &str = "Unsaved review: s save · y copy · q quit · Esc cancel";
/// Status of `<leader>ss` on the log.
pub const OPEN_TO_SAVE: &str = "Open a commit to save its session";
/// Status when `<leader>rs` picks a session whose commit isn't listed.
pub const SESSION_NOT_IN_LOG: &str = "Commit not in the log";
/// Status after `C-d` in the session list.
pub const SESSION_DELETED: &str = "Session deleted";

/// A commit visited this run.
#[derive(Debug, Clone, Default)]
pub(super) struct Visit {
    /// The view as last captured (`None` until its diff loaded).
    view: Option<View>,
    summary: String,
    change: Option<String>,
    /// The branch the log listed when the commit was last opened or
    /// changed (`Some(None)`: the default history or a revset); `None`
    /// until then, when a save takes the comments' branch.
    branch: Option<Option<String>>,
    /// The session's unsent comments were put in the batch (or there were
    /// none); until then a save restores them first.
    restored: bool,
    /// Changed by the user since it was last saved, sent or copied.
    dirty: bool,
}

/// A view to put back once the diff loads (see [`App::session_put_back`]).
#[derive(Debug, Clone)]
pub(super) struct Restore {
    col: usize,
    /// Side by side: the side the cursor was on.
    side: Option<Side>,
    focus: ViewFocus,
    /// The restore status message (a session read from disk).
    status: Option<String>,
}

/// Session state owned by [`App`].
#[derive(Debug)]
pub(super) struct Sessions {
    store: Store,
    /// The sessions on disk, by key (commit ID or [`WORKING`]).
    saved: HashMap<String, Session>,
    visits: HashMap<String, Visit>,
    /// Mode to go back to when the quit prompt is cancelled.
    quit_from: Mode,
    /// Unreadable session files already reported (once per run).
    reported: std::collections::HashSet<String>,
    /// git: the working copy's session waits for its row, read on a worker
    /// after a branch switch; the working-copy request it waits on.
    pending_working: Option<u64>,
}

impl Sessions {
    pub(super) fn new(store: Store) -> Self {
        Sessions {
            store,
            saved: HashMap::new(),
            visits: HashMap::new(),
            quit_from: Mode::Normal,
            reported: Default::default(),
            pending_working: None,
        }
    }
}

fn to_view_context(c: Context) -> ViewContext {
    match c {
        Context::Default => ViewContext::Hunks,
        Context::Full => ViewContext::Full,
    }
}

/// The session key of a commit: [`WORKING`] for the working copy.
fn session_key(working: bool, commit: &str) -> &str {
    if working { WORKING } else { commit }
}

/// `e` prefixed with `review batch: `.
fn batch_error(e: io::Error) -> io::Error {
    io::Error::new(e.kind(), format!("review batch: {e}"))
}

fn from_view_context(c: ViewContext) -> Context {
    match c {
        ViewContext::Hunks => Context::Default,
        ViewContext::Full => Context::Full,
    }
}

/// Comment `c` belongs to the session of `key`: it was made on that
/// commit, or for [`WORKING`] on the working copy with base `base`.
fn belongs(key: &str, c: &Comment, base: Option<&str>) -> bool {
    super::review::made_on(c, key, key == WORKING, base)
}

/// `3 comments`, `1 comment`.
fn comments_label(n: usize) -> String {
    match n {
        1 => "1 comment".to_string(),
        n => format!("{n} comments"),
    }
}

/// The age of unix time `then`, as the log shows it (`2h`).
fn ago(then: u64) -> String {
    super::age(session::now() as i64, then as i64)
}

/// Actions that change the open commit's view (and so its session).
fn touches_view(a: Action) -> bool {
    use Action as A;
    matches!(
        a,
        A::Down(_)
            | A::Up(_)
            | A::GotoTop(_)
            | A::GotoBottom(_)
            | A::HalfPageDown
            | A::HalfPageUp
            | A::Left(_)
            | A::Right(_)
            | A::LineStart
            | A::LineEnd(_)
            | A::WordForward(_)
            | A::WordBackward(_)
            | A::WordEnd(_)
            | A::ParagraphForward(_)
            | A::ParagraphBackward(_)
            | A::PageDown
            | A::PageUp
            | A::ScrollDown
            | A::ScrollUp
            | A::ScreenTop(_)
            | A::ScreenMiddle
            | A::ScreenBottom(_)
            | A::Center
            | A::CursorTop
            | A::CursorBottom
            | A::File(..)
            | A::Hunk(..)
            | A::ToggleContext
            | A::ToggleLayout
            | A::SwitchSide
            | A::SearchNext(..)
            | A::SearchCommit
            | A::ToggleSidebar
            | A::FocusDiff
            | A::Window(_)
            | A::PaneNav(_)
            | A::FileDown(_)
            | A::FileUp(_)
            | A::FileTop
            | A::FileBottom
            | A::FileOpen
            | A::FileHalfDown
            | A::FileHalfUp
            | A::FileCollapse
            | A::FileExpand
            | A::FileAll(_)
            | A::ToggleFiles
            | A::ReviewJump(..)
    )
}

/// Keys of the quit prompt: `s` save, `y` copy, `q` quit, Esc cancel
/// (`C-c` quits before the keymap).
pub(super) fn quit_keymap(keys: &[KeyEvent]) -> KeyResult {
    use crossterm::event::KeyCode;
    let Some(k) = keys.last() else {
        return KeyResult::None;
    };
    let a = match (k.code, plain(k)) {
        (_, Some('s')) => QuitChoice::Save,
        (_, Some('y')) => QuitChoice::Copy,
        (_, Some('q')) => QuitChoice::Quit,
        (KeyCode::Esc, _) => QuitChoice::Cancel,
        _ => return KeyResult::None,
    };
    KeyResult::Action(Action::QuitPrompt(a))
}

use crossterm::event::KeyEvent;

/// A key in the quit prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuitChoice {
    Save,
    Copy,
    Quit,
    Cancel,
}

impl App {
    /// Read the repo's sessions; each unreadable file is skipped and
    /// reported once per run.
    pub(super) fn sessions_load(&mut self) {
        let (list, errors) = self.sessions.store.list();
        self.sessions.saved = list.into_iter().map(|s| (s.commit.clone(), s)).collect();
        let errors: Vec<String> = errors
            .into_iter()
            .map(|e| e.to_string())
            .filter(|e| self.sessions.reported.insert(e.clone()))
            .collect();
        if let Some(e) = errors.first() {
            let more = match errors.len() {
                1 => String::new(),
                n => format!(" (and {} more)", n - 1),
            };
            self.set_status(format!("session skipped: {e}{more}"));
        }
    }

    /// The session key of log row `i`: [`WORKING`] for the working copy.
    fn row_key(&self, i: usize) -> Option<String> {
        let r = self.rows.get(i)?;
        Some(session_key(r.working, &r.info.commit).to_string())
    }

    /// The open view's session key.
    fn open_key(&self) -> Option<String> {
        let v = self.commit.as_ref()?;
        Some(session_key(v.working, &v.commit).to_string())
    }

    /// The marker of a log row with a session: ``, or `3` with
    /// unsent comments. The glyph comes last, so a terminal drawing it
    /// wider than one cell covers the blank after it, not the count.
    pub fn session_marker(&self, row: &super::Row) -> Option<String> {
        let s = self
            .sessions
            .saved
            .get(session_key(row.working, &row.info.commit))?;
        Some(match s.unsent_count() {
            0 => SESSION_MARKER.to_string(),
            n => format!("{n}{SESSION_MARKER}"),
        })
    }

    /// The saved session of `key` (commit ID or `working-copy`), if any.
    pub fn saved_session(&self, key: &str) -> Option<&Session> {
        self.sessions.saved.get(key)
    }

    /// The working copy's base commit ID: git's `HEAD`, jj's single `@-`.
    fn working_base_id(&self) -> Option<String> {
        match self.vcs.kind() {
            Kind::Git => Some(self.working.as_ref()?.base.as_ref()?.commit.clone()),
            Kind::Jj => match &self.all_rows.iter().find(|r| r.working)?.info.parents[..] {
                [one] => Some(one.clone()),
                _ => None,
            },
        }
    }

    /// The batch's comments of commit `key`: rev.commit == key; for
    /// the working copy every comment without a revision and removed-line
    /// comments on its base.
    fn batch_comments(&self, key: &str) -> Vec<Comment> {
        let Some(r) = &self.review else {
            return Vec::new();
        };
        let base = (key == WORKING).then(|| self.working_base_id()).flatten();
        r.comments()
            .iter()
            .filter(|c| belongs(key, c, base.as_deref()))
            .cloned()
            .collect()
    }

    /// The open view as a [`View`]. The app-wide context, layout, file
    /// list and focus are always current; before the diff loads the
    /// cursor (anchor, row, column) is `prev`'s.
    fn current_view(&self, prev: Option<&View>) -> Option<View> {
        let v = self.commit.as_ref()?;
        let mut view = View {
            context: to_view_context(self.context),
            layout: if self.split {
                ViewLayout::Split
            } else {
                ViewLayout::Unified
            },
            sidebar: self.sidebar_show(),
            files: Some(if self.files_tree {
                ViewFiles::Tree
            } else {
                ViewFiles::Flat
            }),
            collapsed: v.collapsed.iter().cloned().collect(),
            // The commit list is part of the file list's pane; browsing
            // it wins. Else a restore still waiting for the diff holds
            // the wanted focus.
            focus: match (&v.restore, self.focus) {
                (_, Focus::Commits) => ViewFocus::Files,
                (Some(r), _) if v.diff.is_none() => r.focus,
                (_, Focus::Diff) => ViewFocus::Diff,
                (_, Focus::Files) => ViewFocus::Files,
            },
            ..prev.cloned().unwrap_or_default()
        };
        if v.diff.is_some() {
            view.anchor = v.anchor();
            view.row = v.cur.row;
            view.col = v.cur.col;
        }
        Some(view)
    }

    /// Remember the open view for its session, before it is replaced or
    /// closed; while its diff loads, the cursor captured earlier this run
    /// (else the saved one) stays.
    pub(super) fn session_capture(&mut self) {
        let Some(key) = self.open_key() else { return };
        let Some(visit) = self.sessions.visits.get(&key) else {
            return;
        };
        let prev = visit
            .view
            .clone()
            .or_else(|| self.sessions.saved.get(&key).map(|s| s.view.clone()));
        let Some(view) = self.current_view(prev.as_ref()) else {
            return;
        };
        if let Some(visit) = self.sessions.visits.get_mut(&key) {
            visit.view = Some(view);
        }
    }

    /// Entering log row `i`: for a commit with a session, before its
    /// diff loads, put back the view captured this run, else the saved
    /// one: context, layout and file list app-wide, as if `T` / `t` /
    /// `<leader>e` were pressed; the cursor (and focus) once it loads.
    /// Until a restore succeeds this run, entering also re-reads the
    /// session and restores its unsent comments the batch lacks.
    pub(super) fn session_enter(&mut self, i: usize) {
        let Some(key) = self.row_key(i) else { return };
        let info = self.rows[i].info.clone();
        let branch = self.selection_branch();
        let visit = self.sessions.visits.entry(key.clone()).or_default();
        visit.summary = info.summary.clone();
        visit.change = info.change.clone();
        visit.branch = Some(branch);
        let captured = visit.view.clone();
        let restored = visit.restored;
        let mut status = None;
        if !restored {
            status = self.session_restore(&key);
        }
        // A commit without a session opens as before.
        let Some(saved) = self.sessions.saved.get(&key) else {
            return;
        };
        let view = captured.unwrap_or_else(|| saved.view.clone());
        self.context = from_view_context(view.context);
        self.split = view.layout == ViewLayout::Split;
        self.set_sidebar_show(view.sidebar);
        // Sessions saved before `files` open as `[view] files` says,
        // not in the mode the last commit left.
        self.files_tree = match view.files {
            Some(f) => f == ViewFiles::Tree,
            None => self.config.view.files == super::config::FilesLayout::Tree,
        };
        let Some(v) = &mut self.commit else { return };
        v.collapsed = view.collapsed.iter().cloned().collect();
        let side = match &view.anchor {
            Some(session::Anchor::Line { side, .. }) => Some(*side),
            _ => None,
        };
        match view.anchor {
            Some(a) => v.keep_anchor = Some((a, 0)),
            None => v.keep_row = Some(view.row),
        }
        v.restore = Some(Restore {
            col: view.col,
            side,
            focus: view.focus,
            status,
        });
    }

    /// Re-read the session of `key` into the cache (`None`, and out of
    /// the cache, when its file is gone). A file that isn't a session
    /// (not JSON, another version or root) counts as none, as in
    /// [`App::sessions_load`]: a save replaces it.
    fn session_reread(&mut self, key: &str) -> io::Result<Option<Session>> {
        let s = match self.sessions.store.load(key) {
            Err(e) if e.kind() == io::ErrorKind::InvalidData => None,
            r => r?,
        };
        match &s {
            Some(s) => self.sessions.saved.insert(key.to_string(), s.clone()),
            None => self.sessions.saved.remove(key),
        };
        Ok(s)
    }

    /// The batch, opened now if it couldn't be before.
    fn batch(&mut self) -> io::Result<&mut Review> {
        let r = match self.review.take() {
            Some(r) => r,
            None => Review::open(&self.opts.cache_dir, self.vcs.root())?,
        };
        Ok(self.review.insert(r))
    }

    /// Re-read the session of `key` and restore its comments (see
    /// [`App::session_restore_comments`]). Returns the restore status;
    /// `None` without a session.
    fn session_restore(&mut self, key: &str) -> Option<String> {
        let s = match self.session_reread(key) {
            Ok(s) => s,
            Err(e) => return Some(format!("session not restored: {e}")),
        };
        let Some(s) = s else {
            self.session_mark_restored(key);
            return None;
        };
        let saved = ago(s.saved);
        let n = s.unsent_count();
        Some(match self.session_restore_comments(&s) {
            Ok(note) => format!(
                "Restored session (saved {saved} ago, {}){note}",
                comments_label(n)
            ),
            Err(e) => format!(
                "Restored session view (saved {saved} ago); {} not restored: {e}",
                comments_label(n)
            ),
        })
    }

    fn session_mark_restored(&mut self, key: &str) {
        if let Some(v) = self.sessions.visits.get_mut(key) {
            v.restored = true;
        }
    }

    /// Add the unsent comments of `s` the batch lacks and mark its visit
    /// restored; a remapped id is written back to the session (`Ok` holds
    /// a note when that write fails). `Err` when the batch can't be read
    /// or written: nothing is restored.
    fn session_restore_comments(&mut self, s: &Session) -> io::Result<String> {
        let unsent: Vec<Comment> = s.unsent().cloned().collect();
        if unsent.is_empty() {
            self.session_mark_restored(&s.commit);
            return Ok(String::new());
        }
        let map = self
            .batch()
            .and_then(|r| r.restore(&unsent))
            .map_err(batch_error)?;
        self.session_mark_restored(&s.commit);
        let remapped: Vec<_> = map.iter().filter(|(a, b)| a != b).collect();
        if remapped.is_empty() {
            return Ok(String::new());
        }
        let mut s = s.clone();
        for c in s.comments.iter_mut().filter(|c| c.sent.is_none()) {
            if let Some((_, new)) = remapped.iter().find(|(old, _)| *old == c.comment.id) {
                c.comment.id = *new;
            }
        }
        let note = match self.sessions.store.save(&s) {
            Ok(()) => String::new(),
            Err(e) => format!("; session not updated: {e}"),
        };
        self.sessions.saved.insert(s.commit.clone(), s);
        Ok(note)
    }

    /// After a restored view's diff loads: the cursor column, the focus
    /// and the restore message.
    pub(super) fn session_put_back(&mut self, r: Restore) {
        if let Some(v) = &mut self.commit {
            // The layout follows the load (`commit_follow`): side by side
            // puts the cursor back on its side.
            if let (Some(want), Some(have)) = (r.side, v.split_side())
                && want != have
            {
                v.swap_side();
            }
            v.set_col(r.col);
            v.center_cursor();
        }
        self.refresh_search();
        // An auto list peeks for a saved file-list focus; browsing the
        // commit list keeps the keys there.
        if r.focus == ViewFocus::Files && self.files_reachable() && self.focus != Focus::Commits {
            self.focus_pane(Focus::Files);
        }
        if let Some(msg) = r.status {
            let s = if self.status.is_empty() {
                msg
            } else {
                format!("{msg}; {}", self.status)
            };
            self.set_status(s);
        }
        self.commit_follow();
    }

    /// An action ran: one that changes the open commit's view marks it.
    pub(super) fn session_after(&mut self, a: Action) {
        if matches!(self.screen, Screen::Commit(_)) && touches_view(a) {
            self.session_touch_open();
        }
    }

    /// The open commit changed (its view or comments).
    pub(super) fn session_touch_open(&mut self) {
        if let Some(key) = self.open_key() {
            self.session_touch(&key);
        }
    }

    fn session_touch(&mut self, key: &str) {
        let branch = self.selection_branch();
        let open = self.open_key().as_deref() == Some(key);
        if let Some(v) = self.sessions.visits.get_mut(key) {
            v.dirty = true;
            // Changed in the open view: reviewed under this log's branch.
            if open {
                v.branch = Some(branch);
            }
        }
    }

    /// A comment is about to be edited or removed outside the open view
    /// (the `<leader>rl` list): mark the commits it belongs to that were
    /// visited or have a session. One not visited yet gets a visit and,
    /// like opening it, its session's comments restored, so the change
    /// is saved and not undone by a later restore.
    pub(super) fn session_touch_comment(&mut self, c: &Comment) {
        let base = self.working_base_id();
        let own = c.rev.as_ref().map_or(WORKING, |r| r.commit.as_str());
        let mut keys = vec![own.to_string()];
        if own != WORKING && belongs(WORKING, c, base.as_deref()) {
            keys.push(WORKING.to_string());
        }
        for k in keys {
            if !self.sessions.visits.contains_key(&k) {
                if !matches!(self.session_reread(&k), Ok(Some(_))) {
                    continue;
                }
                self.sessions.visits.insert(k.clone(), Visit::default());
            }
            if !self.sessions.visits[&k].restored {
                let _ = self.session_restore(&k);
            }
            self.session_touch(&k);
        }
    }

    /// Commits with unsaved review work, in no particular order.
    fn unsaved(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .sessions
            .visits
            .iter()
            .filter(|(k, v)| {
                let has_session = self.sessions.saved.contains_key(*k);
                let has_comments = !self.batch_comments(k).is_empty();
                (v.dirty && (has_session || has_comments)) || (has_comments && !has_session)
            })
            .map(|(k, _)| k.clone())
            .collect();
        out.sort();
        out
    }

    /// `q` on the log, `C-c` anywhere: quit, or ask first when there is
    /// unsaved review work.
    pub(super) fn quit_requested(&mut self) {
        self.session_capture();
        self.review_reload();
        if self.unsaved().is_empty() {
            self.quit = true;
            return;
        }
        self.sessions.quit_from = self.mode;
        self.pending.clear();
        self.count = None;
        self.mode = Mode::QuitPrompt;
        self.set_status(QUIT_PROMPT);
    }

    /// A key in the quit prompt.
    pub(super) fn quit_choice(&mut self, c: QuitChoice) {
        match c {
            QuitChoice::Quit => self.quit = true,
            QuitChoice::Cancel => {
                self.mode = self.sessions.quit_from;
                self.status.clear();
            }
            QuitChoice::Save => {
                self.mode = self.sessions.quit_from;
                // A batch that can't be read can't be saved.
                if let Err(e) = self.batch_reload() {
                    return self.set_status(format!("session not saved: {e} (not quitting)"));
                }
                let keys = self.unsaved();
                let mut saved = 0;
                for k in &keys {
                    match self.session_save_key(k) {
                        Ok(_) => saved += 1,
                        Err(e) => {
                            return self.set_status(format!(
                                "session not saved: {e} ({saved} of {} saved; not quitting)",
                                keys.len()
                            ));
                        }
                    }
                }
                self.quit = true;
            }
            QuitChoice::Copy => {
                self.mode = self.sessions.quit_from;
                match self.copy_batch() {
                    Ok(()) => self.quit = true,
                    Err(e) => self.set_status(format!("Review not copied: {e}")),
                }
            }
        }
    }

    /// `y` in the quit prompt: the batch to the clipboard (OSC 52,
    /// written now), then the copied comments leave the batch and are
    /// marked sent, as a send with the clipboard does.
    fn copy_batch(&mut self) -> std::io::Result<()> {
        let Some(r) = &mut self.review else {
            return Ok(());
        };
        r.reload()?;
        let ids: Vec<u64> = r.comments().iter().map(|c| c.id).collect();
        if ids.is_empty() {
            return Ok(());
        }
        let md = r.to_markdown(
            self.config.send.preamble.as_deref(),
            &self.config.review.kinds,
        );
        (self.terminal)(super::osc52(&md).as_bytes())?;
        // Mark sent first: a comment left unmarked would come back.
        self.session_sent(&ids)
            .map_err(|e| std::io::Error::new(e.kind(), format!("session not updated: {e}")))?;
        if let Some(r) = &mut self.review {
            r.remove_all(&ids)?;
        }
        Ok(())
    }

    /// Comments `ids` were sent or copied by debrief: the sessions, re-read
    /// from the store, mark them sent, and every dirty bit clears. Called
    /// before the comments leave the batch; when the sessions can't be
    /// read (a file that isn't a session is skipped) or written it stops,
    /// the dirty bits stay and the caller keeps the batch.
    pub(super) fn session_sent(&mut self, ids: &[u64]) -> std::io::Result<()> {
        let now = session::now();
        let (list, errors) = self.sessions.store.list();
        if let Some(e) = errors
            .into_iter()
            .find(|e| e.kind() != io::ErrorKind::InvalidData)
        {
            return Err(e);
        }
        self.sessions.saved = list.iter().map(|s| (s.commit.clone(), s.clone())).collect();
        for mut s in list {
            let k = s.commit.clone();
            let mut changed = false;
            for c in s.comments.iter_mut() {
                if c.sent.is_none() && ids.contains(&c.comment.id) {
                    c.sent = Some(now);
                    changed = true;
                }
            }
            if changed {
                self.sessions.store.save(&s)?;
                self.sessions.saved.insert(k, s);
            }
        }
        for v in self.sessions.visits.values_mut() {
            v.dirty = false;
        }
        Ok(())
    }

    /// Re-read the batch before saving from it; `Err` also when it
    /// can't be opened.
    fn batch_reload(&mut self) -> std::io::Result<()> {
        self.batch()
            .and_then(|r| r.reload().map(|_| ()))
            .map_err(batch_error)
    }

    /// Write the session of `key` from the batch and its view (captured,
    /// or the open one), keeping the comments its file (re-read) holds as
    /// sent; returns how many comments it holds unsent. Comments of a
    /// session not restored yet are restored first; when that fails
    /// nothing is written.
    fn session_save_key(&mut self, key: &str) -> std::io::Result<usize> {
        if self.open_key().as_deref() == Some(key) {
            self.session_capture();
        }
        let visit = self.sessions.visits.get(key).cloned().unwrap_or_default();
        if let Some(s) = self.session_reread(key)?
            && !visit.restored
        {
            self.session_restore_comments(&s)?;
        }
        let old = self.sessions.saved.get(key);
        let view = visit
            .view
            .clone()
            .or_else(|| old.map(|s| s.view.clone()))
            .unwrap_or_default();
        let batch = self.batch_comments(key);
        let mut comments: Vec<SavedComment> = old
            .map(|s| {
                s.comments
                    .iter()
                    .filter(|c| c.sent.is_some())
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        comments.retain(|c| !batch.iter().any(|b| b.id == c.comment.id));
        let n = batch.len();
        let batch_branch = batch
            .iter()
            .find_map(|c| c.rev.as_ref().and_then(|r| r.branch.clone()));
        comments.extend(batch.into_iter().map(|comment| SavedComment {
            comment,
            sent: None,
        }));
        let s = Session {
            version: session::VERSION,
            root: self.sessions.store.root().to_path_buf(),
            commit: key.to_string(),
            change: visit
                .change
                .clone()
                .or_else(|| old.and_then(|s| s.change.clone())),
            summary: if visit.summary.is_empty() {
                old.map(|s| s.summary.clone()).unwrap_or_default()
            } else {
                visit.summary.clone()
            },
            // The branch it was reviewed under: the visit's, else its
            // comments', else what was saved before.
            branch: match &visit.branch {
                Some(b) => b.clone(),
                None => batch_branch.or_else(|| old.and_then(|s| s.branch.clone())),
            },
            saved: session::now(),
            comments,
            view,
        };
        self.sessions.store.save(&s)?;
        self.sessions.saved.insert(key.to_string(), s);
        if let Some(v) = self.sessions.visits.get_mut(key) {
            v.dirty = false;
        }
        Ok(n)
    }

    /// `<leader>ss`: save the open commit's session.
    pub(super) fn session_save(&mut self) {
        let Some(key) = self
            .open_key()
            .filter(|_| matches!(self.screen, Screen::Commit(_)))
        else {
            return self.set_status(OPEN_TO_SAVE);
        };
        let msg = match self
            .batch_reload()
            .and_then(|()| self.session_save_key(&key))
        {
            Ok(n) => format!("Session saved ({})", comments_label(n)),
            Err(e) => format!("session not saved: {e}"),
        };
        self.set_status(msg);
    }

    /// The repo's sessions for the `<leader>rs` list, newest save first.
    pub(super) fn session_items(&self) -> Vec<super::PickerItem> {
        let mut list: Vec<&Session> = self.sessions.saved.values().collect();
        list.sort_by(|a, b| b.saved.cmp(&a.saved).then_with(|| a.commit.cmp(&b.commit)));
        list.into_iter()
            .map(|s| {
                let short = if s.commit == WORKING {
                    "working copy".to_string()
                } else {
                    let id = s.change.as_deref().unwrap_or(&s.commit);
                    id.chars().take(8).collect()
                };
                let sent = match s.sent_count() {
                    0 => String::new(),
                    n => format!(" ({n} sent)"),
                };
                let branch = s
                    .branch
                    .as_deref()
                    .map_or(String::new(), |b| format!(" on {b}"));
                super::PickerItem {
                    id: 0,
                    key: s.commit.clone(),
                    label: format!("{short} {}", s.summary),
                    detail: format!(
                        "{} ago{branch}, {}{sent}",
                        ago(s.saved),
                        comments_label(s.comments.len())
                    ),
                }
            })
            .collect()
    }

    /// `<leader>rs`: re-read the sessions and list them.
    pub(super) fn session_list(&mut self) {
        self.sessions_load();
        self.picker_open_kind(super::picker::PickerKind::Sessions);
    }

    /// Enter in the session list: open its commit, if listed.
    pub(super) fn session_goto(&mut self, key: &str) {
        // The session's branch first; a gone one opens in this log.
        let branch = self.sessions.saved.get(key).and_then(|s| s.branch.clone());
        let mut note = None;
        if let Some(b) = &branch
            && !self.branch_switch(b)
        {
            note = Some(super::branch::gone(b));
        }
        // The working copy's row: git's synthetic one, jj's `@`.
        let id = match key {
            WORKING => self
                .all_rows
                .iter()
                .find(|r| r.working)
                .map(|r| r.info.commit.clone()),
            _ => Some(key.to_string()),
        };
        if let Some(id) = &id
            && self.unfilter_for(id)
        {
            self.commit_reindex();
        }
        let i = if key == WORKING {
            self.working_index()
        } else {
            self.rows.iter().position(|r| r.info.commit == key)
        };
        match i {
            Some(i) => self.open_index(i),
            // git's row is on its way (read off the UI thread): open it
            // when it lands.
            None if key == WORKING && self.working_pending() => {
                self.sessions.pending_working = Some(self.working_request);
            }
            None => self.set_status(SESSION_NOT_IN_LOG),
        }
        if let Some(n) = note {
            let s = if self.status.is_empty() {
                n
            } else {
                format!("{n}; {}", self.status)
            };
            self.set_status(s);
        }
    }

    /// git's working-copy row for `request` arrived: open the working
    /// copy's session if it waits on it (and the row is there).
    pub(super) fn session_working_arrived(&mut self, request: u64) {
        if self
            .sessions
            .pending_working
            .take_if(|r| *r == request)
            .is_none()
        {
            return;
        }
        match self.working_index() {
            Some(i) => self.open_index(i),
            None => self.set_status(SESSION_NOT_IN_LOG),
        }
    }

    /// `C-d` in the session list.
    pub(super) fn session_delete(&mut self, key: &str) {
        let msg = match self.sessions.store.delete(key) {
            Ok(()) => {
                self.sessions.saved.remove(key);
                SESSION_DELETED.to_string()
            }
            Err(e) => format!("session not deleted: {e}"),
        };
        self.set_status(msg);
    }
}
