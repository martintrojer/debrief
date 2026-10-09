//! Live reload (spec §3 "Log view"): a `notify` watcher on the files that
//! change when the repository does, debounced into one callback.
//!
//! git: `HEAD`, `packed-refs` and `refs/` as `git rev-parse --git-path`
//! names them (a linked worktree keeps `HEAD` apart from the shared refs).
//! jj: `op_heads/heads` in the repo dir, following `.jj/repo` when it is a
//! file (a secondary workspace). Parent dirs are watched, since tools
//! replace files by rename.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use notify::{RecursiveMode, Watcher as _};

use crate::vcs::{Kind, Vcs};

/// Quiet period before a burst of events becomes one reload.
pub const DEBOUNCE: Duration = Duration::from_millis(300);

/// What to watch: directories to register, and the paths whose changes
/// count (an event path equal to or under one of them).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchTargets {
    pub dirs: Vec<(PathBuf, RecursiveMode)>,
    pub relevant: Vec<PathBuf>,
}

impl WatchTargets {
    /// Whether a change to `path` should reload.
    pub fn is_relevant(&self, path: &Path) -> bool {
        self.relevant.iter().any(|r| path.starts_with(r))
    }
}

/// Canonical form of `p`, canonicalising the parent when `p` itself is
/// missing (a ref file between delete and rename).
fn canon(p: &Path) -> PathBuf {
    if let Ok(c) = p.canonicalize() {
        return c;
    }
    match (p.parent(), p.file_name()) {
        (Some(dir), Some(name)) => dir
            .canonicalize()
            .unwrap_or_else(|_| dir.to_path_buf())
            .join(name),
        _ => p.to_path_buf(),
    }
}

/// The watch targets for `vcs`.
pub fn watch_targets(vcs: &dyn Vcs) -> Result<WatchTargets> {
    match vcs.kind() {
        Kind::Git => git_targets(vcs.root()),
        Kind::Jj => jj_targets(vcs.root()),
    }
}

fn git_targets(root: &Path) -> Result<WatchTargets> {
    let out = Command::new("git")
        .arg("--no-optional-locks")
        .arg("-C")
        .arg(root)
        .args([
            "rev-parse",
            "--path-format=absolute",
            "--git-path",
            "HEAD",
            "--git-path",
            "refs",
            "--git-path",
            "packed-refs",
        ])
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .output()
        .context("failed to run git")?;
    if !out.status.success() {
        bail!(
            "git rev-parse --git-path failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let paths: Vec<PathBuf> = text.lines().map(|l| canon(Path::new(l))).collect();
    let [head, refs, packed] = &paths[..] else {
        bail!("unexpected git rev-parse output: {text:?}");
    };
    let mut dirs: Vec<(PathBuf, RecursiveMode)> = Vec::new();
    for d in [head.parent(), packed.parent()].into_iter().flatten() {
        if !dirs.iter().any(|(x, _)| x == d) {
            dirs.push((d.to_path_buf(), RecursiveMode::NonRecursive));
        }
    }
    dirs.push((refs.clone(), RecursiveMode::Recursive));
    Ok(WatchTargets {
        dirs,
        relevant: vec![head.clone(), refs.clone(), packed.clone()],
    })
}

fn jj_targets(root: &Path) -> Result<WatchTargets> {
    let mut repo = root.join(".jj/repo");
    if repo.is_file() {
        let target = std::fs::read_to_string(&repo)
            .with_context(|| format!("cannot read {}", repo.display()))?;
        repo = root.join(".jj").join(target.trim_end_matches('\n'));
    }
    let heads = repo.join("op_heads/heads");
    let heads = heads
        .canonicalize()
        .with_context(|| format!("cannot open {}", heads.display()))?;
    Ok(WatchTargets {
        dirs: vec![(heads.clone(), RecursiveMode::NonRecursive)],
        relevant: vec![heads],
    })
}

/// A running watcher; dropping it stops the events.
pub struct RepoWatcher {
    _inner: notify::RecommendedWatcher,
}

impl RepoWatcher {
    /// Watch `targets`; `emit` runs once per debounced burst of relevant
    /// changes, on a background thread.
    pub fn start(targets: WatchTargets, emit: impl Fn() + Send + 'static) -> Result<RepoWatcher> {
        let (tx, rx) = mpsc::channel::<()>();
        let filter = targets.clone();
        let mut inner = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if let Ok(ev) = res
                && (ev.paths.is_empty() || ev.paths.iter().any(|p| filter.is_relevant(p)))
            {
                let _ = tx.send(());
            }
        })?;
        for (dir, mode) in &targets.dirs {
            inner
                .watch(dir, *mode)
                .with_context(|| format!("cannot watch {}", dir.display()))?;
        }
        std::thread::spawn(move || debounce(rx, emit));
        Ok(RepoWatcher { _inner: inner })
    }
}

fn debounce(rx: mpsc::Receiver<()>, emit: impl Fn()) {
    while rx.recv().is_ok() {
        loop {
            match rx.recv_timeout(DEBOUNCE) {
                Ok(()) => continue,
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
        emit();
    }
}
