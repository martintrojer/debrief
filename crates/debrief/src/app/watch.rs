//! Live reload (spec §3 "Log view"): a `notify` watcher on the files that
//! change when the repository does, debounced into one callback.
//!
//! git: `HEAD`, `packed-refs` and `refs/` as `git rev-parse --git-path`
//! names them (a linked worktree keeps `HEAD` apart from the shared refs).
//! jj: `op_heads/heads` in the repo dir, following `.jj/repo` when it is a
//! file (a secondary workspace). Parent dirs are watched, since tools
//! replace files by rename.
//!
//! git's working tree (spec Q24, K9): [`tree_watcher`] watches the root
//! recursively, plus the dirs of `info/exclude` and `core.excludesFile`
//! when they lie outside it. Events in the git dir never count. An
//! untracked path counts unless git ignores it (nested `.gitignore`s,
//! `info/exclude`, `core.excludesFile`, via the `ignore` crate); a tracked
//! path always counts. Paths are judged as named, never through a final
//! symlink. A change to any ignore file rebuilds the rules and counts; a
//! change to the index marks the tracked set stale.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use ignore::Match;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
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

/// `p` with its parent dir canonicalised but its last component kept as
/// named, so a symlink stays the link and not its target.
fn canon_parent(p: &Path) -> PathBuf {
    match (p.parent(), p.file_name()) {
        (Some(dir), Some(name)) if !dir.as_os_str().is_empty() => dir
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

/// `git rev-parse --git-path <name>` as an absolute path. One path per call:
/// output lines cannot be split when the path itself may contain newlines,
/// so strip exactly the one trailing newline git appends.
fn git_path(root: &Path, name: &str) -> Result<PathBuf> {
    let out = Command::new("git")
        .arg("--no-optional-locks")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "--path-format=absolute", "--git-path", name])
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .output()
        .context("failed to run git")?;
    if !out.status.success() {
        bail!(
            "git rev-parse --git-path {name} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let text = String::from_utf8(out.stdout).context("git path is not UTF-8")?;
    let Some(path) = text.strip_suffix('\n') else {
        bail!("unexpected git rev-parse output: {text:?}");
    };
    Ok(canon(Path::new(path)))
}

fn git_targets(root: &Path) -> Result<WatchTargets> {
    let head = &git_path(root, "HEAD")?;
    let refs = &git_path(root, "refs")?;
    let packed = &git_path(root, "packed-refs")?;
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
        let filter = targets.clone();
        Self::with_filter(&targets.dirs, move |p| filter.is_relevant(p), emit)
    }

    /// Watch `dirs`; `emit` runs once per debounced burst of events with
    /// a path `relevant` accepts (or with no path).
    pub fn with_filter(
        dirs: &[(PathBuf, RecursiveMode)],
        mut relevant: impl FnMut(&Path) -> bool + Send + 'static,
        emit: impl Fn() + Send + 'static,
    ) -> Result<RepoWatcher> {
        let (tx, rx) = mpsc::channel::<()>();
        let mut inner = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            let Ok(ev) = res else { return };
            if matches!(ev.kind, notify::EventKind::Access(_)) {
                return;
            }
            // Every path goes through the filter: it also notes rule changes.
            let hits = ev.paths.iter().filter(|p| relevant(p)).count();
            if ev.paths.is_empty() || hits > 0 {
                let _ = tx.send(());
            }
        })?;
        for (dir, mode) in dirs {
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

/// Watch git's working tree at `root`; `emit` runs once per debounced
/// burst of relevant changes (see [`TreeFilter`]).
pub fn tree_watcher(root: &Path, emit: impl Fn() + Send + 'static) -> Result<RepoWatcher> {
    let mut filter = TreeFilter::new(root)?;
    let mut dirs = vec![(filter.root.clone(), RecursiveMode::Recursive)];
    // Ignore files outside the root: watch their dirs (editors replace
    // files by rename). A dir that doesn't exist yet is skipped.
    for g in &filter.global {
        let Some(d) = g.parent() else { continue };
        if d.starts_with(&filter.root) || !d.is_dir() || dirs.iter().any(|(x, _)| x == d) {
            continue;
        }
        dirs.push((d.to_path_buf(), RecursiveMode::NonRecursive));
    }
    RepoWatcher::with_filter(&dirs, move |p| filter.is_relevant(p), emit)
}

/// Which working-tree paths refresh git's working copy (spec K9).
pub struct TreeFilter {
    root: PathBuf,
    git_dir: PathBuf,
    index: PathBuf,
    /// `info/exclude` and `core.excludesFile` as named, and the targets
    /// when they are symlinks: rooted at `root`.
    global: Vec<PathBuf>,
    rules: Rules,
    /// Tracked files and every directory holding one (absolute).
    tracked: HashSet<PathBuf>,
    tracked_stale: bool,
}

/// Ignore rules: one matcher per `.gitignore` (rooted at its dir), deepest
/// first, then the repo-wide ones.
struct Rules {
    nested: Vec<(PathBuf, Gitignore)>,
    global: Gitignore,
}

impl TreeFilter {
    pub fn new(root: &Path) -> Result<TreeFilter> {
        let root = canon(root);
        let git_dir = PathBuf::from(
            git_line(&root, &["rev-parse", "--absolute-git-dir"])?
                .context("git rev-parse --absolute-git-dir printed nothing")?,
        );
        let git_dir = canon(&git_dir);
        let index = git_path(&root, "index")?;
        let mut sources = vec![git_path(&root, "info/exclude")?];
        match git_line(&root, &["config", "--path", "core.excludesFile"]) {
            // git reads a relative path from the top of the work tree.
            Ok(Some(p)) => sources.push(root.join(p)),
            _ => {
                if let Some(p) = default_excludes() {
                    sources.push(p);
                }
            }
        }
        let mut global = Vec::new();
        for s in sources {
            let named = canon_parent(&s);
            let target = canon(&s);
            global.push(named.clone());
            if target != named {
                global.push(target);
            }
        }
        let mut f = TreeFilter {
            rules: Rules::build(&root, &git_dir, &global),
            tracked: HashSet::new(),
            tracked_stale: true,
            root,
            git_dir,
            index,
            global,
        };
        f.refresh_tracked();
        Ok(f)
    }

    fn refresh_tracked(&mut self) {
        // `ls-files` reads the index and never writes it.
        let out = Command::new("git")
            .arg("--no-optional-locks")
            .arg("-C")
            .arg(&self.root)
            .args(["ls-files", "-z", "--cached"])
            .env("GIT_OPTIONAL_LOCKS", "0")
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output();
        if let Ok(o) = out
            && o.status.success()
        {
            let mut tracked = HashSet::new();
            for p in String::from_utf8_lossy(&o.stdout).split('\0') {
                if p.is_empty() {
                    continue;
                }
                let abs = self.root.join(p);
                for a in abs.ancestors() {
                    if a == self.root || !tracked.insert(a.to_path_buf()) {
                        break;
                    }
                }
            }
            self.tracked = tracked;
            self.tracked_stale = false;
        }
    }

    fn is_tracked(&mut self, path: &Path) -> bool {
        if self.tracked_stale {
            self.refresh_tracked();
        }
        self.tracked.contains(path)
    }

    fn is_ignore_file(&self, path: &Path) -> bool {
        let nested = path.file_name().is_some_and(|n| n == ".gitignore")
            && path.starts_with(&self.root)
            && !path.starts_with(&self.git_dir);
        nested || self.global.iter().any(|g| g == path)
    }

    /// Whether a change to `path` should refresh the working copy. The
    /// path is judged as named: a final symlink is not followed.
    pub fn is_relevant(&mut self, path: &Path) -> bool {
        // notify names paths under the (canonical) dirs it watches; only
        // a path outside them gets its parent resolved.
        let path = &if path.starts_with(&self.root) || path.starts_with(&self.git_dir) {
            path.to_path_buf()
        } else {
            canon_parent(path)
        };
        if self.is_ignore_file(path) {
            // New rules may hide or reveal untracked files: refresh too.
            self.rules = Rules::build(&self.root, &self.git_dir, &self.global);
            return true;
        }
        if path.starts_with(&self.git_dir) {
            if path == &self.index {
                self.tracked_stale = true;
            }
            return false;
        }
        if !path.starts_with(&self.root) {
            return false;
        }
        // An event for a `.gitignore` can be lost: inotify watches a new
        // dir only once it sees the dir appear, so a file written straight
        // after `mkdir` goes unseen. Check the dirs on the way to `path`.
        if self.rules.stale_for(&self.root, path) {
            self.rules = Rules::build(&self.root, &self.git_dir, &self.global);
        }
        if !self.rules.ignored(path) {
            return true;
        }
        self.is_tracked(path)
    }
}

impl Rules {
    fn build(root: &Path, git_dir: &Path, global: &[PathBuf]) -> Rules {
        let mut nested = Vec::new();
        // The walk skips what is already ignored, as git does.
        let walk = ignore::WalkBuilder::new(root)
            .hidden(false)
            .parents(false)
            .require_git(false)
            .filter_entry({
                let git_dir = git_dir.to_path_buf();
                move |e| !e.path().starts_with(&git_dir) && e.file_name() != ".git"
            })
            .build();
        for e in walk.flatten() {
            if e.file_name() == ".gitignore" && e.file_type().is_some_and(|t| t.is_file()) {
                let (gi, _) = Gitignore::new(e.path());
                let dir = e.path().parent().unwrap_or(root).to_path_buf();
                nested.push((dir, gi));
            }
        }
        nested.sort_by_key(|(d, _)| std::cmp::Reverse(d.components().count()));
        let mut b = GitignoreBuilder::new(root);
        for g in global {
            if g.is_file() {
                let _ = b.add(g);
            }
        }
        Rules {
            nested,
            global: b.build().unwrap_or_else(|_| Gitignore::empty()),
        }
    }

    /// Whether a `.gitignore` between `root` and `path` appeared or went
    /// away since these rules were built.
    fn stale_for(&self, root: &Path, path: &Path) -> bool {
        path.ancestors()
            .skip(1)
            .take_while(|d| d.starts_with(root))
            .any(|d| {
                let known = self.nested.iter().any(|(n, _)| n == d);
                known != d.join(".gitignore").is_file()
            })
    }

    /// git's verdict on `path` (or a parent dir of it): the deepest
    /// `.gitignore` with an opinion wins, then the repo-wide rules.
    fn ignored(&self, path: &Path) -> bool {
        // git sees a symlink as a file, wherever it points.
        let is_dir = path.symlink_metadata().is_ok_and(|m| m.is_dir());
        for (dir, gi) in &self.nested {
            if !path.starts_with(dir) || path == dir {
                continue;
            }
            match gi.matched_path_or_any_parents(path, is_dir) {
                Match::Ignore(_) => return true,
                Match::Whitelist(_) => return false,
                Match::None => {}
            }
        }
        let Ok(rel) = path.strip_prefix(self.global.path()) else {
            return false;
        };
        if rel.as_os_str().is_empty() {
            return false;
        }
        self.global
            .matched_path_or_any_parents(path, is_dir)
            .is_ignore()
    }
}

/// git's default `core.excludesFile`: `$XDG_CONFIG_HOME/git/ignore`, else
/// `~/.config/git/ignore`.
fn default_excludes() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .or_else(|| dirs::home_dir().map(|h| h.join(".config")))
        .map(|d| d.join("git/ignore"))
}

/// One line of git output (`None` when empty; `Err` when git fails).
fn git_line(root: &Path, args: &[&str]) -> Result<Option<String>> {
    let out = Command::new("git")
        .arg("--no-optional-locks")
        .arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .output()
        .context("failed to run git")?;
    if !out.status.success() {
        bail!("git {} failed", args.join(" "));
    }
    let text = String::from_utf8(out.stdout).context("git output is not UTF-8")?;
    let line = text.strip_suffix('\n').unwrap_or(&text);
    Ok((!line.is_empty()).then(|| line.to_string()))
}
