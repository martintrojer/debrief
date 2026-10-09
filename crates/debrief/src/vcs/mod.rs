//! Read-only access to git and jj repositories through their CLIs.
//!
//! Every read runs with flags that stop the tool from writing to the
//! repository (no optional locks, no working-copy snapshot, no op-head
//! merge, no textconv cache). The one exception is jj's snapshot
//! ([`Vcs::refresh`], spec Q21): it leaves files alone but records the
//! working copy as a new operation, as any jj command does;
//! [`OpenOptions::snapshot`] turns it off.

mod diff;
mod git;
mod jj;

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

pub use diff::parse_unified;
pub use git::{Git, MAX_UNTRACKED, SPECIAL_FILE, TOO_LARGE, untracked_file};
pub use jj::Jj;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Git,
    Jj,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitInfo {
    pub commit: String,
    /// jj change ID; `None` for git.
    pub change: Option<String>,
    pub author: String,
    /// Author time, unix seconds.
    pub time: i64,
    pub summary: String,
    pub parents: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitDiff {
    pub message: String,
    pub files: Vec<FileDiff>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDiff {
    /// `None` when the file was added.
    pub old_path: Option<String>,
    /// `None` when the file was deleted.
    pub new_path: Option<String>,
    pub binary: bool,
    pub hunks: Vec<Hunk>,
    /// Shown instead of hunks when set: why there is no diff text (an
    /// untracked file that is too large, or not a regular file).
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    /// The full `@@` line.
    pub header: String,
    pub old_start: u32,
    pub new_start: u32,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: LineKind,
    pub old: Option<u32>,
    pub new: Option<u32>,
    /// Without the `+`/`-`/space prefix and the newline.
    pub text: String,
    /// Followed by `\ No newline at end of file`.
    pub no_newline: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Context,
    Added,
    Removed,
}

/// How a repository is opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenOptions {
    /// jj: snapshot the working copy on [`Vcs::refresh`] (default on;
    /// `--no-snapshot` turns it off). git ignores it.
    pub snapshot: bool,
}

impl Default for OpenOptions {
    fn default() -> Self {
        OpenOptions { snapshot: true }
    }
}

/// git's uncommitted changes (spec §3 "Working copy").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkingCopy {
    /// `HEAD`; `None` before the first commit.
    pub base: Option<CommitInfo>,
    /// Tracked changes against `base`, and untracked files as added.
    /// `message` is empty.
    pub diff: CommitDiff,
    /// Number of changed files.
    pub files: usize,
}

pub trait Vcs {
    fn kind(&self) -> Kind;
    fn root(&self) -> &Path;
    /// The options it was opened with (to reopen it on another thread).
    fn open_options(&self) -> OpenOptions;
    /// Bring later reads up to date. jj with [`OpenOptions::snapshot`]:
    /// `snapshot` snapshots the working copy first; then reads are pinned
    /// to the current operation. On failure reads stay on the last good
    /// pin and the error is returned. git, or jj with `--no-snapshot`:
    /// nothing (every read is current).
    fn refresh(&self, _snapshot: bool) -> Result<()> {
        Ok(())
    }
    /// git: the uncommitted changes, `None` when the tree is clean.
    /// jj: always `None` (`@` is the working copy).
    fn working_copy(&self) -> Result<Option<WorkingCopy>> {
        Ok(None)
    }
    /// jj: the operation reads are pinned to (to reopen on another
    /// thread with [`reopen`]). git: `None`.
    fn pinned_op(&self) -> Option<String> {
        None
    }
    /// jj: the commit ID of this workspace's `@`. git: `None`.
    fn working_copy_commit(&self) -> Result<Option<String>> {
        Ok(None)
    }
    /// Newest first. `None` uses the default: `HEAD` for git, the user's
    /// `revsets.log` for jj.
    fn log(&self, revset: Option<&str>, limit: usize) -> Result<Vec<CommitInfo>>;
    /// `commit` is a commit ID (as returned by `log`) or, for git, any
    /// revision naming one commit.
    fn show(&self, commit: &str) -> Result<CommitDiff>;
}

/// Find the repository holding `path`: the nearest directory with `.jj`
/// (jj wins in a colocated repo), else the nearest with `.git`.
pub fn open(path: &Path, opts: OpenOptions) -> Result<Box<dyn Vcs>> {
    let canon = path
        .canonicalize()
        .with_context(|| format!("cannot open {}", path.display()))?;
    let start = if canon.is_dir() {
        canon.clone()
    } else {
        canon
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| canon.clone())
    };
    if let Some(root) = find_up(&start, ".jj") {
        return Ok(Box::new(Jj::new(root, opts)));
    }
    if find_up(&start, ".git").is_some() {
        return Ok(Box::new(Git::open(&start)?));
    }
    bail!("not a git or jj repository: {}", path.display())
}

/// Open `root` again (a worker thread's handle), reading at `pin` when
/// it is a jj repo pinned there.
pub fn reopen(root: &Path, opts: OpenOptions, pin: Option<String>) -> Result<Box<dyn Vcs>> {
    if let Some(op) = pin {
        return Ok(Box::new(Jj::pinned(root.to_path_buf(), opts, op)));
    }
    open(root, opts)
}

fn find_up(start: &Path, marker: &str) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|d| d.join(marker).exists())
        .map(Path::to_path_buf)
}

/// Run a VCS command and return its stdout; a failure carries stderr.
fn run<I, S>(program: &str, args: I) -> Result<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut cmd = Command::new(program);
    cmd.args(args)
        .env("GIT_PAGER", "cat")
        .env("PAGER", "cat")
        .env("LC_ALL", "C")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(std::process::Stdio::null());
    let out = cmd
        .output()
        .with_context(|| format!("failed to run {program}"))?;
    if !out.status.success() {
        let args: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        bail!(
            "{program} {} failed ({}): {}",
            args.join(" "),
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}
