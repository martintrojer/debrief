//! Application state. Screens add their own files here; `keys` dispatches,
//! `ui` draws. A new screen adds a [`Screen`] variant, its keymap arm in
//! `keys`, its rows in `help::BINDINGS` and one draw call in `ui`.

pub mod branch;
mod clue;
pub mod comment;
pub mod commit;
pub mod commit_list;
pub mod config;
pub mod display;
pub mod file_tree;
mod fold;
mod help;
pub mod highlight;
mod keys;
pub mod launch;
mod log;
mod log_search;
mod motion;
pub mod mouse;
pub mod pane_nav;
mod path_filter;
pub mod picker;
pub mod review;
pub mod run;
pub mod search;
pub mod session;
pub mod sidebar;
pub mod split;
pub mod stat;
pub mod textbox;
mod visual;
pub mod watch;
pub mod zoom;

use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::time::Instant;

use crossterm::event::{KeyEvent, MouseEvent};
use ratatui::Frame;

use crate::seen::{Seen, Status};
use crate::vcs::{CommitDiff, CommitInfo, CommitStat, Kind, Vcs, WorkingCopy, WorkingStatus};

pub use clue::{CLUE_DELAY, ClueRow};
pub use comment::{CommentAction, CommentBox, Draft};
pub use commit::{Focus, Row as DiffRow, RowKind, Subject};
pub use fold::{FOLD_CLOSED, FOLD_OPEN, GROUP_BAR, GROUP_LAST, GROUP_MID};
pub use help::{HelpLine, HelpView, help_rect};
pub use keys::{Action, KeyResult};
pub use keys::{LEADER, TOGGLES};
pub use launch::{Effect, Exit, LaunchCommand};
pub use log::{
    FOLD_MORE_PREFIX, MORE_ROW, NOT_LOADED, WORKING_ID, age, is_fold_more, is_zero_id, short_id,
};
pub use motion::Cursor;
pub use mouse::{Hit, MULTI_CLICK, WHEEL_ROWS};
pub use picker::{PickerAction, PickerItem, PickerKind, PickerView};
pub use session::QuitChoice;
pub use stat::{LogLayout, StatPane};
pub use visual::osc52;

/// Default number of commits listed.
pub const DEFAULT_LIMIT: usize = 200;

/// What the binary was asked to show.
#[derive(Debug, Clone)]
pub struct StartOptions {
    /// The debrief cache dir (seen state and the review batch live under it).
    pub cache_dir: PathBuf,
    /// The debrief state dir (saved review sessions live under it).
    pub state_dir: PathBuf,
    /// `--revset` / `[log] revset`: what the log lists at start unless a
    /// `branch` is given; the `<leader>b` picker offers it either way.
    pub revset: Option<String>,
    /// `--branch`: the branch (jj bookmark) to list at start (before a
    /// revset); `None` is the revset, else the current branch.
    pub branch: Option<String>,
    /// The log's path filter: only commits touching this repo-relative
    /// file or directory load (`F` changes it).
    pub path: Option<String>,
    pub limit: usize,
    /// `~/.config/debrief/config.toml` over the defaults.
    pub config: config::Config,
}

impl StartOptions {
    /// Options with `cache_dir`, and the state dir inside it (so tests
    /// never touch the user's).
    pub fn new(cache_dir: PathBuf) -> Self {
        StartOptions {
            state_dir: cache_dir.join("state"),
            cache_dir,
            revset: None,
            branch: None,
            path: None,
            limit: DEFAULT_LIMIT,
            config: config::Config::defaults(),
        }
    }
}

/// Input to the app.
#[derive(Debug, Clone)]
pub enum AppEvent {
    Key(KeyEvent),
    Resize(u16, u16),
    /// Text pasted into the terminal (bracketed paste).
    Paste(String),
    /// The repository's refs or jj operations changed (from the watcher).
    Reload,
    /// git: a file in the working tree changed (from the tree watcher).
    WorkingChanged,
    /// A commit's diff from the loader thread, tagged with its request.
    Loaded {
        request: u64,
        result: Result<CommitDiff, String>,
    },
    /// git's working copy from the loader thread (`None`: clean), tagged
    /// with its request.
    WorkingLoaded {
        request: u64,
        result: Result<Option<WorkingCopy>, String>,
    },
    /// git's working-copy row from the loader thread (`None`: clean),
    /// tagged with its request.
    WorkingRow {
        request: u64,
        result: Result<Option<WorkingStatus>, String>,
    },
    /// The syntax colours of a loaded diff, tagged with the same request;
    /// they follow its [`AppEvent::Loaded`] or [`AppEvent::WorkingLoaded`].
    Highlighted {
        request: u64,
        spans: highlight::DiffSpans,
    },
    /// A click, drag or wheel notch (only arrives when the mouse is
    /// captured, `[mouse] enabled`).
    Mouse(MouseEvent),
    /// A commit's stat for the log's stats pane, tagged with its request.
    Stat {
        request: u64,
        result: Result<CommitStat, String>,
    },
    /// The log re-read with a larger limit (the next page) from the
    /// loader thread, tagged with its request.
    LogPage {
        request: u64,
        result: Result<Vec<CommitInfo>, String>,
    },
    /// A merge's fold contents from the loader thread, one list per
    /// group (non-first parent, in parent order), tagged with its request.
    FoldLoaded {
        request: u64,
        merge: String,
        result: Result<Vec<Vec<CommitInfo>>, String>,
    },
    /// One group of a merge's fold re-read with a larger limit (its next
    /// page), tagged with its request.
    FoldPage {
        request: u64,
        merge: String,
        parent: String,
        result: Result<Vec<CommitInfo>, String>,
    },
    /// The sizes (commits brought in) of merges on the log, tagged with
    /// their request.
    MergeSizes {
        request: u64,
        result: Result<Vec<(String, usize)>, String>,
    },
    /// Every commit `merge` brought in, to mark seen (opening a merge, or
    /// the first run). Keyed by the merge, never stale: commit IDs stay
    /// valid whatever the log shows now.
    BubbleLoaded {
        merge: String,
        result: Result<Vec<CommitInfo>, String>,
    },
}

/// `text` as one prompt line: each line break (`\r\n`, `\n`, `\r`) and
/// tab becomes a space; other control characters are dropped.
fn paste_line(text: &str) -> String {
    text.replace("\r\n", "\n")
        .chars()
        .filter_map(|c| match c {
            '\n' | '\r' | '\t' => Some(' '),
            c if c.is_control() => None,
            c => Some(c),
        })
        .collect()
}

/// The main view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Log,
    /// The commit at this index of the log.
    Commit(usize),
}

/// Input modes on top of the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    /// The `g?` overlay.
    Help,
    /// Typing a `/` or `?` pattern (commit view or log).
    Search,
    /// Typing the log's `f` filter.
    Filter,
    /// Typing the log's `F` path filter.
    PathFilter,
    /// Line-wise selection in the commit view.
    Visual,
    /// Typing a comment's body.
    Comment,
    /// The `K` popup.
    Popup,
    /// The `<leader>rl` / `<leader>rs` picker.
    Picker,
    /// "Unsaved review: s save · y copy · q quit · Esc cancel".
    QuitPrompt,
}

/// One log row: the commit and how it shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub info: CommitInfo,
    pub status: Status,
    /// The working copy: git's synthetic row (`info.commit` is
    /// [`WORKING_ID`]) or jj's `@`.
    pub working: bool,
    /// Fold nesting: 0 on the spine, one more per enclosing open fold.
    pub depth: u8,
    /// The innermost enclosing octopus group's marker (`groups.last()`).
    pub group: Option<Group>,
    /// The group marker of every enclosing octopus fold, outermost first
    /// (empty on the spine and inside 2-parent folds).
    pub groups: Vec<Group>,
    /// A merge row's fold while folding is on.
    pub fold: Option<Fold>,
    /// The merge whose fold holds the row; `None` on the spine.
    pub merge: Option<String>,
}

/// Which group of an octopus fold a row is in: every group but the last
/// is `Mid`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Mid,
    Last,
}

/// A merge row's fold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fold {
    /// Commits the merge brought in, nested ones included; `None` until
    /// its count is in.
    pub size: Option<usize>,
    pub open: bool,
}

pub struct App {
    vcs: Box<dyn Vcs>,
    opts: StartOptions,
    seen: Seen,
    /// The spine, open folds and loaded fold contents (`fold`).
    folds: fold::Folds,
    /// The log rows shown: `all_rows` narrowed by the filter.
    rows: Vec<Row>,
    /// Every loaded log row: the spine with open folds spliced in.
    all_rows: Vec<Row>,
    /// The log's `/` search and `f` filter.
    log_search: log_search::LogSearch,
    /// The `F` prompt's text while it is open.
    path_edit: Option<String>,
    /// The open commit a path change dropped from the log, and the log's
    /// commits before the change (for `]]` / `[[`).
    hidden_open: Option<(String, Vec<String>)>,
    /// What the log lists (`<leader>b`).
    selection: branch::Selection,
    /// The selection's query: `None` is the default history.
    query: Option<String>,
    /// The checked-out branch (jj: the bookmark on `@` or `@-`).
    branch_current: Option<String>,
    /// The log's stats pane.
    stat: stat::StatState,
    /// How many commits the log asks for, and the next page loading.
    page: log::Paging,
    cursor: usize,
    /// First row drawn.
    scroll: usize,
    /// Log rows that fit on screen, from the last draw.
    view_h: usize,
    screen: Screen,
    mode: Mode,
    pending: Vec<KeyEvent>,
    /// The typed count.
    count: Option<usize>,
    status: String,
    quit: bool,
    help: help::HelpState,
    clue: clue::ClueState,
    /// Terminal size (columns, rows).
    size: (u16, u16),
    sender: Option<Sender<AppEvent>>,
    watcher: Option<watch::RepoWatcher>,
    /// git: the working-tree watcher.
    tree_watcher: Option<watch::RepoWatcher>,
    /// git: the working-copy row, `None` when clean (or jj).
    working: Option<WorkingStatus>,
    /// The last working-copy row request number.
    working_request: u64,
    /// `[view] follow_working`: the request of the follow load in flight.
    follow_request: Option<u64>,
    /// `[view] follow_working`: a tree change waits for V or the comment
    /// box to close.
    follow_stale: bool,
    /// jj: the commit ID of `@` at the pinned operation.
    working_commit: Option<String>,
    /// The open commit (on `Screen::Commit`).
    commit: Option<commit::CommitView>,
    /// The last commit-load request number.
    next_request: u64,
    focus: Focus,
    /// The file list: always / never / auto, pin and peek.
    sidebar: sidebar::SidebarState,
    /// The commit list under the file list's tree.
    commit_list: commit_list::CommitListState,
    /// `T`: hunks or the full file, for every commit opened this session.
    context: crate::vcs::Context,
    /// `t`: side by side wanted for this session (shown when the diff pane
    /// is `[view] split_min_width` wide).
    split: bool,
    /// `` ` ``: the file list as a tree (else flat), for every commit
    /// opened this session.
    files_tree: bool,
    /// Bytes for the terminal (OSC 52).
    output: Vec<u8>,
    config: config::Config,
    /// The resolved `[theme]`: the palette the UI draws with.
    theme: crate::theme::Theme,
    /// `theme.syntax`, shared with the highlighting threads.
    syntax: std::sync::Arc<syntect::highlighting::Theme>,
    /// Launcher key sequences after the leader, and their index.
    launch_keys: Vec<(Vec<char>, usize)>,
    /// A side effect waiting for the suspended TUI.
    effect: Option<Effect>,
    runner: Box<launch::Runner>,
    /// Runs `tmux select-pane` for a pane move at the edge
    /// (`pane_nav::spawn_capped` by default: never blocks long).
    pane_runner: Box<launch::Runner>,
    /// Environment lookup for `${editor}` and `$TMUX`.
    env: Box<launch::EnvLookup>,
    /// The review batch for `vcs.root()`, shared with ramble.
    review: Option<debrief_review::Review>,
    /// When the batch was last re-read.
    review_polled: Option<Instant>,
    /// The comment being typed.
    draft: Option<comment::Draft>,
    /// The `K` popup's text.
    popup: Option<String>,
    /// The `K` popup's first visible wrapped line.
    popup_scroll: usize,
    /// The `<leader>rl` picker.
    picker: Option<picker::Picker>,
    /// The last click and a drag in progress.
    mouse: mouse::MouseState,
    /// Test hook between a send's export and its removal.
    after_export: Option<Box<dyn FnMut()>>,
    /// Writes and flushes bytes to the terminal (stdout; a send's OSC 52
    /// clipboard copy goes through it so its IO error is seen).
    terminal: Box<TerminalWriter>,
    /// Saved review sessions and the commits visited this run.
    sessions: session::Sessions,
    /// The first run's merges whose bubbles are marked seen once there is
    /// an event sender (off the UI thread, not at start).
    bubbles_due: Vec<String>,
}

/// Writes bytes straight to the terminal and flushes them.
pub type TerminalWriter = dyn FnMut(&[u8]) -> std::io::Result<()>;

fn stdout_write(bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut out = std::io::stdout();
    out.write_all(bytes)?;
    out.flush()
}

impl App {
    /// Open the seen state and load the log. A repo opened for the first
    /// time starts with every listed commit marked seen.
    pub fn new(vcs: Box<dyn Vcs>, opts: StartOptions) -> anyhow::Result<App> {
        use anyhow::Context;
        let seen = Seen::open(&opts.cache_dir, vcs.root())
            .with_context(|| format!("cannot read seen state in {}", opts.cache_dir.display()))?;
        let opts_state = opts.state_dir.clone();
        let limit = opts.limit;
        let root = vcs.root().to_path_buf();
        let theme = opts
            .config
            .resolve_theme()
            .map(|r| crate::theme::Theme::new(&r))
            .map_err(|e| anyhow::anyhow!("theme.{}: {}", e.key, e.msg))?;
        let syntax = std::sync::Arc::new(theme.syntax.clone());
        let mut app = App {
            vcs,
            opts,
            seen,
            folds: fold::Folds::default(),
            rows: Vec::new(),
            all_rows: Vec::new(),
            log_search: log_search::LogSearch::default(),
            path_edit: None,
            hidden_open: None,
            selection: branch::Selection::Default,
            query: None,
            branch_current: None,
            stat: stat::StatState::default(),
            page: log::Paging::new(limit),
            cursor: 0,
            scroll: 0,
            view_h: 20,
            screen: Screen::Log,
            mode: Mode::Normal,
            pending: Vec::new(),
            count: None,
            status: String::new(),
            quit: false,
            help: help::HelpState::default(),
            clue: clue::ClueState::default(),
            size: (80, 24),
            sender: None,
            watcher: None,
            tree_watcher: None,
            working: None,
            working_request: 0,
            follow_request: None,
            follow_stale: false,
            working_commit: None,
            commit: None,
            next_request: 0,
            focus: Focus::Diff,
            sidebar: sidebar::SidebarState::default(),
            commit_list: commit_list::CommitListState::default(),
            context: crate::vcs::Context::Default,
            split: false,
            files_tree: true,
            output: Vec::new(),
            config: config::Config::default(),
            theme,
            syntax,
            launch_keys: Vec::new(),
            effect: None,
            runner: Box::new(launch::system_run),
            pane_runner: Box::new(pane_nav::spawn_capped),
            env: Box::new(|k| std::env::var(k).ok()),
            review: None,
            review_polled: None,
            draft: None,
            popup: None,
            popup_scroll: 0,
            picker: None,
            mouse: mouse::MouseState::default(),
            after_export: None,
            terminal: Box::new(stdout_write),
            sessions: session::Sessions::new(crate::session::Store::new(&opts_state, &root)),
            bubbles_due: Vec::new(),
        };
        app.review_open();
        app.config = app.opts.config.clone();
        app.sidebar.show = app.config.view.sidebar;
        app.commit_list.on = app.config.view.sidebar_commits;
        app.stat.on = app.config.view.stats;
        app.context = app.config.view.context;
        app.split = app.config.view.layout == config::Layout::Split;
        app.files_tree = app.config.view.files == config::FilesLayout::Tree;
        app.folds.on = app.config.view.fold_merges;
        let (keys, mut errors) = launch::bindings(&app.config.launch);
        app.launch_keys = keys;
        errors.extend(app.config.warnings.iter().cloned());
        if !errors.is_empty() {
            app.status = errors.join("; ");
        }
        // One refresh and one branch read: `branch_start` reads jj state
        // after the snapshot, and has just read what `load` would re-read.
        app.refresh(true);
        app.branch_start()?;
        let commits = app.load_refreshed()?;
        if app.seen.is_fresh() {
            // The first run marks what it lists seen and, under a path
            // filter, the unfiltered log's first page too, so a later
            // unfiltered run starts caught up.
            let mut all = commits.clone();
            if app.opts.path.is_some() {
                let spine = app.folding();
                let q = app.query.as_deref();
                all.extend(app.vcs.log_in(q, None, app.opts.limit, spine)?);
            }
            app.seen.mark_all(&all).context("cannot write seen state")?;
            // Folded, the listed merges' bubbles are not listed: they are
            // marked too, so old folds don't show new once loaded. That
            // is a read per merge, left to the worker once there is a
            // sender ([`App::set_sender`]): App::new has none in the
            // binary, and start must not wait on them.
            if app.folds_shown() {
                let mut due: Vec<String> = Vec::new();
                for c in all.iter().filter(|c| c.parents.len() > 1) {
                    if !due.contains(&c.commit) {
                        due.push(c.commit.clone());
                    }
                }
                app.bubbles_due = due;
            }
        }
        app.set_rows(commits);
        app.cursor = app.first_unseen().unwrap_or(0);
        app.sessions_load();
        Ok(app)
    }

    /// Feed one event.
    pub fn event(&mut self, ev: AppEvent) {
        match ev {
            AppEvent::Key(k) => self.key(k),
            AppEvent::Resize(c, r) => {
                self.sidebar_resized(c);
                self.size = (c, r);
                self.commit_list_refocus();
                self.commit_follow();
            }
            AppEvent::Paste(text) => self.paste(&text),
            AppEvent::Reload => self.reload(false),
            AppEvent::WorkingChanged => self.working_changed(),
            AppEvent::Loaded { request, result } => self.loaded(request, result),
            AppEvent::WorkingLoaded { request, result } => self.working_loaded(request, result),
            AppEvent::WorkingRow { request, result } => self.working_row_loaded(request, result),
            AppEvent::Highlighted { request, spans } => self.highlighted(request, spans),
            AppEvent::Stat { request, result } => self.stat_loaded(request, result),
            AppEvent::LogPage { request, result } => self.page_loaded(request, result),
            AppEvent::FoldLoaded {
                request,
                merge,
                result,
            } => self.fold_loaded(request, &merge, result),
            AppEvent::FoldPage {
                request,
                merge,
                parent,
                result,
            } => self.fold_page_loaded(request, &merge, &parent, result),
            AppEvent::MergeSizes { request, result } => self.sizes_loaded(request, result),
            AppEvent::BubbleLoaded { merge, result } => self.bubble_loaded(&merge, result),
            AppEvent::Mouse(m) => {
                self.mouse(m, Instant::now());
                self.follow_resume();
            }
        }
    }

    /// A bracketed paste: typed into the comment box with its newlines,
    /// or into the open prompt (`/` `?` search, the log's filter, picker
    /// filter) as one line, newlines and tabs as spaces; the commit
    /// list's search prompt is the log's. Anywhere else it is dropped,
    /// never replayed as keys.
    pub fn paste(&mut self, text: &str) {
        match self.mode {
            Mode::Comment => self.comment_paste(text),
            Mode::Search if self.screen == Screen::Log || self.log_searching() => {
                self.log_search_insert(&paste_line(text));
            }
            Mode::Search => self.search_insert(&paste_line(text)),
            Mode::Filter => self.filter_insert(&paste_line(text)),
            Mode::PathFilter => self.path_insert(&paste_line(text)),
            Mode::Picker => self.picker_insert(&paste_line(text)),
            _ => {}
        }
    }

    /// Time passes: the key clue opens after a pause; the review batch
    /// is re-read at most once a second; the stats pane loads once the
    /// cursor rests.
    pub fn tick(&mut self, now: Instant) {
        self.clue_tick(now);
        self.review_tick(now);
        self.stat_tick(now);
    }

    /// Draw the whole screen.
    pub fn render(&mut self, frame: &mut Frame) {
        let area = frame.area();
        self.size = (area.width, area.height);
        let main = ratatui::layout::Rect {
            height: area.height.saturating_sub(1),
            ..area
        };
        self.view_h = self.log_layout(main).log.height.max(1) as usize;
        self.keep_visible();
        self.commit_follow();
        self.stat_follow();
        crate::ui::draw(frame, self);
    }

    /// The resolved theme: palette, syntax colours, `fill`.
    pub fn theme(&self) -> &crate::theme::Theme {
        &self.theme
    }

    /// The config in effect.
    pub fn config(&self) -> &config::Config {
        &self.config
    }

    /// The terminal size (columns, rows), from the last resize or draw.
    pub fn size(&self) -> (u16, u16) {
        self.size
    }

    pub fn should_quit(&self) -> bool {
        self.quit
    }

    pub fn screen(&self) -> Screen {
        self.screen
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn scroll(&self) -> usize {
        self.scroll
    }

    pub fn status(&self) -> &str {
        &self.status
    }

    pub fn set_status(&mut self, s: impl Into<String>) {
        self.status = s.into();
    }

    pub fn vcs(&self) -> &dyn Vcs {
        self.vcs.as_ref()
    }

    pub fn kind(&self) -> Kind {
        self.vcs.kind()
    }

    pub fn seen(&self) -> &Seen {
        &self.seen
    }

    /// git: the uncommitted changes the log's top row stands for.
    pub fn working_copy(&self) -> Option<&WorkingStatus> {
        self.working.as_ref()
    }

    /// Loaded rows that are new, and rows that are updated.
    pub fn counts(&self) -> (usize, usize) {
        let n = |s| self.all_rows.iter().filter(|r| r.status == s).count();
        (n(Status::New), n(Status::Updated))
    }

    /// The repo root for the status line, `~` for `$HOME`.
    pub fn root_label(&self) -> String {
        let root = self.vcs.root();
        if let Some(home) = dirs::home_dir()
            && let Ok(rest) = root.strip_prefix(&home)
        {
            return if rest.as_os_str().is_empty() {
                "~".to_string()
            } else {
                format!("~/{}", rest.display())
            };
        }
        root.display().to_string()
    }

    /// Where watcher events go; call before [`App::start_watcher`]. A
    /// first run's bubble marks start here (see [`AppEvent::BubbleLoaded`]);
    /// an app that never gets a sender leaves them unmarked.
    pub fn set_sender(&mut self, tx: Sender<AppEvent>) {
        self.sender = Some(tx);
        let due = std::mem::take(&mut self.bubbles_due);
        self.bubbles_request(due);
    }

    /// Watch the repository and send [`AppEvent::Reload`] on changes; for
    /// git also the working tree, sending [`AppEvent::WorkingChanged`].
    pub fn start_watcher(&mut self) -> anyhow::Result<()> {
        let Some(tx) = self.sender.clone() else {
            anyhow::bail!("no event sender");
        };
        let targets = watch::watch_targets(self.vcs.as_ref())?;
        let refs = tx.clone();
        self.watcher = Some(watch::RepoWatcher::start(targets, move || {
            let _ = refs.send(AppEvent::Reload);
        })?);
        if self.vcs.kind() == Kind::Git {
            self.tree_watcher = Some(watch::tree_watcher(self.vcs.root(), move || {
                let _ = tx.send(AppEvent::WorkingChanged);
            })?);
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::io::Write;
    use std::process::{Command, Stdio};

    use super::{App, StartOptions};

    fn git(dir: &std::path::Path, args: &[&str], stdin: &[u8]) {
        let mut child = Command::new(crate::vcs::git_program())
            .current_dir(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(["-c", "maintenance.auto=false", "-c", "gc.auto=0"])
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(stdin).unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success(), "{out:?}");
    }

    /// An app on a fresh git repo with two commits.
    pub(crate) fn app() -> (tempfile::TempDir, App) {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("repo");
        std::fs::create_dir(&root).unwrap();
        git(&root, &["init", "-q", "-b", "main"], b"");
        // Two commits (c0, c1) writing a.txt, dated just before now.
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let mut s = String::new();
        for i in 0..2u64 {
            let date = now - 2 + i;
            s += &format!("commit refs/heads/main\ncommitter t <t@t> {date} +0000\n");
            s += &format!("data 3\nc{i}\nM 100644 inline a.txt\ndata 2\n{i}\n\n\n");
        }
        git(&root, &["fast-import", "--quiet"], s.as_bytes());
        git(&root, &["read-tree", "-u", "--reset", "HEAD"], b"");
        let vcs = crate::vcs::open(&root, crate::vcs::OpenOptions::default()).unwrap();
        let app = App::new(vcs, StartOptions::new(tmp.path().join("cache"))).unwrap();
        (tmp, app)
    }

    /// A git-kind [`Vcs`](crate::vcs::Vcs) without a process: [`app`]'s
    /// shape (a log of two commits, each showing one file with one hunk)
    /// served from memory. Its root is a real directory, as `Review::open`
    /// canonicalizes it.
    struct FakeVcs {
        root: std::path::PathBuf,
    }

    impl crate::vcs::Vcs for FakeVcs {
        fn kind(&self) -> crate::vcs::Kind {
            crate::vcs::Kind::Git
        }
        fn root(&self) -> &std::path::Path {
            &self.root
        }
        fn open_options(&self) -> crate::vcs::OpenOptions {
            crate::vcs::OpenOptions::default()
        }
        fn log(&self, _: Option<&str>, _: usize) -> anyhow::Result<Vec<crate::vcs::CommitInfo>> {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64;
            let commit = |i: i64, parents: Vec<String>| crate::vcs::CommitInfo {
                commit: format!("{i:040x}"),
                change: None,
                author: "t".into(),
                time: now - 2 + i,
                summary: format!("c{i}"),
                parents,
            };
            Ok(vec![
                commit(2, vec![format!("{:040x}", 1)]),
                commit(1, vec![]),
            ])
        }
        fn show(&self, _: &str, _: crate::vcs::Context) -> anyhow::Result<crate::vcs::CommitDiff> {
            use crate::vcs::{DiffLine, FileDiff, Hunk, LineKind};
            let line = |kind, old, new, text: &str| DiffLine {
                kind,
                old,
                new,
                text: text.into(),
                no_newline: false,
            };
            Ok(crate::vcs::CommitDiff {
                message: "c1\n".into(),
                files: vec![FileDiff {
                    old_path: Some("a.txt".into()),
                    new_path: Some("a.txt".into()),
                    binary: false,
                    hunks: vec![Hunk {
                        header: "@@ -1 +1 @@".into(),
                        old_start: 1,
                        new_start: 1,
                        lines: vec![
                            line(LineKind::Removed, Some(1), None, "0"),
                            line(LineKind::Added, None, Some(1), "1"),
                        ],
                    }],
                    note: None,
                    mode_change: None,
                    copied: false,
                }],
            })
        }
        fn stat(&self, _: &str) -> anyhow::Result<crate::vcs::CommitStat> {
            anyhow::bail!("no stat")
        }
    }

    /// An app like [`app`]'s on a [`FakeVcs`]: no git process, for tests
    /// that need only the app's state (keymaps, help, clues).
    pub(crate) fn fake_app() -> (tempfile::TempDir, App) {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("repo");
        std::fs::create_dir(&root).unwrap();
        let vcs = Box::new(FakeVcs { root });
        let app = App::new(vcs, StartOptions::new(tmp.path().join("cache"))).unwrap();
        (tmp, app)
    }
}
