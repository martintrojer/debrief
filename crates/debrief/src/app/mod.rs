//! Application state. Screens add their own files here; `keys` dispatches,
//! `ui` draws. A new screen adds a [`Screen`] variant, its keymap arm in
//! `keys`, its rows in `help::BINDINGS` and one draw call in `ui`.

mod clue;
pub mod comment;
pub mod commit;
pub mod config;
mod help;
mod keys;
pub mod launch;
mod log;
mod motion;
pub mod picker;
pub mod review;
pub mod run;
mod search;
pub mod sidebar;
mod visual;
pub mod watch;

use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::time::Instant;

use crossterm::event::KeyEvent;
use ratatui::Frame;

use crate::seen::{Seen, Status};
use crate::vcs::{CommitDiff, CommitInfo, Kind, Vcs};

pub use clue::{CLUE_DELAY, ClueRow};
pub use comment::{CommentAction, Draft};
pub use commit::{Focus, Row as DiffRow, RowKind};
pub use help::{HelpLine, HelpView, help_rect};
pub use keys::LEADER;
pub use keys::{Action, KeyResult};
pub use launch::{Effect, Exit, LaunchCommand};
pub use log::{age, is_zero_id, short_id};
pub use motion::Cursor;
pub use picker::{PickerAction, PickerItem, PickerView};
pub use visual::osc52;

/// Default number of commits listed.
pub const DEFAULT_LIMIT: usize = 200;

/// What the binary was asked to show.
#[derive(Debug, Clone)]
pub struct StartOptions {
    /// The debrief cache dir (seen state and, later, reviews live under it).
    pub cache_dir: PathBuf,
    pub revset: Option<String>,
    pub limit: usize,
    /// `[send]` and `[[launch]]`.
    pub config: config::Config,
}

impl StartOptions {
    pub fn new(cache_dir: PathBuf) -> Self {
        StartOptions {
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
    /// The repository changed on disk (from the watcher).
    Reload,
    /// A commit's diff from the loader thread, tagged with its request.
    Loaded {
        request: u64,
        result: Result<CommitDiff, String>,
    },
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
    /// Typing a `/` or `?` pattern in the commit view.
    Search,
    /// Line-wise selection in the commit view.
    Visual,
    /// Typing a comment's body.
    Comment,
    /// The `K` popup.
    Popup,
    /// The `<leader>rl` picker.
    Picker,
}

/// One log row: the commit and how it shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub info: CommitInfo,
    pub status: Status,
}

pub struct App {
    vcs: Box<dyn Vcs>,
    opts: StartOptions,
    seen: Seen,
    rows: Vec<Row>,
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
    /// The open commit (on `Screen::Commit`).
    commit: Option<commit::CommitView>,
    /// The last commit-load request number.
    next_request: u64,
    focus: Focus,
    /// `<leader>e` state of the file list.
    sidebar_on: bool,
    /// Bytes for the terminal (OSC 52).
    output: Vec<u8>,
    config: config::Config,
    /// Launcher key sequences after the leader, and their index.
    launch_keys: Vec<(Vec<char>, usize)>,
    /// A side effect waiting for the suspended TUI.
    effect: Option<Effect>,
    runner: Box<launch::Runner>,
    env: Box<launch::EnvLookup>,
    /// The review batch for `vcs.root()`, shared with ramble.
    review: Option<debrief_review::Review>,
    /// When the batch was last re-read.
    review_polled: Option<Instant>,
    /// The comment being typed.
    draft: Option<comment::Draft>,
    /// The `K` popup's text.
    popup: Option<String>,
    /// The `<leader>rl` picker.
    picker: Option<picker::Picker>,
    /// Test hook between a send's export and its removal.
    after_export: Option<Box<dyn FnMut()>>,
    /// Writes and flushes bytes to the terminal (stdout; a send's OSC 52
    /// clipboard copy goes through it so its IO error is seen).
    terminal: Box<TerminalWriter>,
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
        let mut app = App {
            vcs,
            opts,
            seen,
            rows: Vec::new(),
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
            commit: None,
            next_request: 0,
            focus: Focus::Diff,
            sidebar_on: true,
            output: Vec::new(),
            config: config::Config::default(),
            launch_keys: Vec::new(),
            effect: None,
            runner: Box::new(launch::system_run),
            env: Box::new(|k| std::env::var(k).ok()),
            review: None,
            review_polled: None,
            draft: None,
            popup: None,
            picker: None,
            after_export: None,
            terminal: Box::new(stdout_write),
        };
        app.review_open();
        app.config = app.opts.config.clone();
        let (keys, errors) = launch::bindings(&app.config.launch);
        app.launch_keys = keys;
        if !errors.is_empty() {
            app.status = errors.join("; ");
        }
        let commits = app.load()?;
        if app.seen.is_fresh() {
            app.seen
                .mark_all(&commits)
                .context("cannot write seen state")?;
        }
        app.set_rows(commits);
        app.cursor = app.first_unseen().unwrap_or(0);
        Ok(app)
    }

    /// Feed one event.
    pub fn event(&mut self, ev: AppEvent) {
        match ev {
            AppEvent::Key(k) => self.key(k),
            AppEvent::Resize(c, r) => {
                self.size = (c, r);
                self.commit_follow();
            }
            AppEvent::Reload => self.reload(),
            AppEvent::Loaded { request, result } => self.loaded(request, result),
        }
    }

    /// Time passes: the key clue opens after a pause; the review batch
    /// is re-read at most once a second.
    pub fn tick(&mut self, now: Instant) {
        self.clue_tick(now);
        self.review_tick(now);
    }

    /// Draw the whole screen.
    pub fn render(&mut self, frame: &mut Frame) {
        let area = frame.area();
        self.size = (area.width, area.height);
        self.view_h = area.height.saturating_sub(1).max(1) as usize;
        self.keep_visible();
        self.commit_follow();
        crate::ui::draw(frame, self);
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

    /// Rows that are new, and rows that are updated.
    pub fn counts(&self) -> (usize, usize) {
        let n = |s| self.rows.iter().filter(|r| r.status == s).count();
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

    /// Watch the repository and send [`AppEvent::Reload`] on changes.
    pub fn start_watcher(&mut self) -> anyhow::Result<()> {
        let Some(tx) = self.sender.clone() else {
            anyhow::bail!("no event sender");
        };
        let targets = watch::watch_targets(self.vcs.as_ref())?;
        self.watcher = Some(watch::RepoWatcher::start(targets, move || {
            let _ = tx.send(AppEvent::Reload);
        })?);
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
        let vcs = crate::vcs::open(&root).unwrap();
        let app = App::new(vcs, StartOptions::new(tmp.path().join("cache"))).unwrap();
        (tmp, app)
    }
}
