//! Application state. Screens add their own files here; `keys` dispatches,
//! `ui` draws. A new screen adds a [`Screen`] variant, its keymap arm in
//! `keys`, its rows in `help::BINDINGS` and one draw call in `ui`.

mod clue;
pub mod comment;
pub mod commit;
pub mod commit_list;
pub mod config;
pub mod display;
pub mod file_tree;
mod help;
pub mod highlight;
mod keys;
pub mod launch;
mod log;
mod log_search;
mod motion;
pub mod mouse;
pub mod pane_nav;
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

use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::time::Instant;

use crossterm::event::{KeyEvent, MouseEvent};
use ratatui::Frame;

use crate::seen::{Seen, Status};
use crate::vcs::{CommitDiff, CommitInfo, CommitStat, Kind, Vcs, WorkingCopy, WorkingStatus};

pub use clue::{CLUE_DELAY, ClueRow};
pub use comment::{CommentAction, CommentBox, Draft};
pub use commit::{Focus, Row as DiffRow, RowKind};
pub use help::{HelpLine, HelpView, help_rect};
pub use keys::{Action, KeyResult};
pub use keys::{LEADER, TOGGLES};
pub use launch::{Effect, Exit, LaunchCommand};
pub use log::{MORE_ROW, NOT_LOADED, WORKING_ID, age, is_zero_id, short_id};
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
    pub revset: Option<String>,
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
}

pub struct App {
    vcs: Box<dyn Vcs>,
    opts: StartOptions,
    seen: Seen,
    /// The log rows shown: `all_rows` narrowed by the filter.
    rows: Vec<Row>,
    /// Every loaded log row.
    all_rows: Vec<Row>,
    /// The log's `/` search and `f` filter.
    log_search: log_search::LogSearch,
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
        let mut app = App {
            vcs,
            opts,
            seen,
            rows: Vec::new(),
            all_rows: Vec::new(),
            log_search: log_search::LogSearch::default(),
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
        };
        app.review_open();
        app.config = app.opts.config.clone();
        app.sidebar.show = app.config.view.sidebar;
        app.commit_list.on = app.config.view.sidebar_commits;
        app.stat.on = app.config.view.stats;
        app.context = app.config.view.context;
        app.split = app.config.view.layout == config::Layout::Split;
        app.files_tree = app.config.view.files == config::FilesLayout::Tree;
        let (keys, mut errors) = launch::bindings(&app.config.launch);
        app.launch_keys = keys;
        errors.extend(app.config.warnings.iter().cloned());
        if !errors.is_empty() {
            app.status = errors.join("; ");
        }
        let commits = app.load(true)?;
        if app.seen.is_fresh() {
            app.seen
                .mark_all(&commits)
                .context("cannot write seen state")?;
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
            AppEvent::Mouse(m) => self.mouse(m, Instant::now()),
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

    /// Where watcher events go; call before [`App::start_watcher`].
    pub fn set_sender(&mut self, tx: Sender<AppEvent>) {
        self.sender = Some(tx);
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
    use std::process::Command;

    use super::{App, StartOptions};

    fn git(dir: &std::path::Path, args: &[&str]) {
        let out = Command::new("git")
            .current_dir(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(["-c", "maintenance.auto=false", "-c", "gc.auto=0"])
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
    }

    /// An app on a fresh git repo with two commits.
    pub(crate) fn app() -> (tempfile::TempDir, App) {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("repo");
        std::fs::create_dir(&root).unwrap();
        git(&root, &["init", "-q"]);
        for i in 0..2 {
            std::fs::write(root.join("a.txt"), format!("{i}\n")).unwrap();
            git(&root, &["add", "-A"]);
            git(&root, &["commit", "-q", "-m", &format!("c{i}")]);
        }
        let vcs = crate::vcs::open(&root, crate::vcs::OpenOptions::default()).unwrap();
        let app = App::new(vcs, StartOptions::new(tmp.path().join("cache"))).unwrap();
        (tmp, app)
    }
}
