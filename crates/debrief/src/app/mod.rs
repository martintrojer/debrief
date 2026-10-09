//! Application state. Screens add their own files here; `keys` dispatches,
//! `ui` draws. A new screen adds a [`Screen`] variant, its keymap arm in
//! `keys`, its rows in `help::BINDINGS` and one draw call in `ui`.

mod clue;
mod help;
mod keys;
mod log;
pub mod run;
pub mod watch;

use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::time::Instant;

use crossterm::event::KeyEvent;
use ratatui::Frame;

use crate::seen::{Seen, Status};
use crate::vcs::{CommitInfo, Kind, Vcs};

pub use clue::{CLUE_DELAY, ClueRow};
pub use help::{HelpLine, HelpView, help_rect};
pub use keys::{Action, KeyResult};
pub use log::{age, is_zero_id, short_id};

/// Default number of commits listed.
pub const DEFAULT_LIMIT: usize = 200;

/// What the binary was asked to show.
#[derive(Debug, Clone)]
pub struct StartOptions {
    /// The debrief cache dir (seen state and, later, reviews live under it).
    pub cache_dir: PathBuf,
    pub revset: Option<String>,
    pub limit: usize,
}

impl StartOptions {
    pub fn new(cache_dir: PathBuf) -> Self {
        StartOptions {
            cache_dir,
            revset: None,
            limit: DEFAULT_LIMIT,
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
        };
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
            AppEvent::Resize(c, r) => self.size = (c, r),
            AppEvent::Reload => self.reload(),
        }
    }

    /// Time passes: the key clue opens after a pause.
    pub fn tick(&mut self, now: Instant) {
        self.clue_tick(now);
    }

    /// Draw the whole screen.
    pub fn render(&mut self, frame: &mut Frame) {
        let area = frame.area();
        self.size = (area.width, area.height);
        self.view_h = area.height.saturating_sub(1).max(1) as usize;
        self.keep_visible();
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
