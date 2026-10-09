//! Read-only access to git and jj repositories through their CLIs.
//!
//! Every command is run with flags that stop the tool from writing to the
//! repository (no optional locks, no working-copy snapshot, no op-head
//! merge, no textconv cache).

mod diff;
mod git;

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

pub use diff::parse_unified;
pub use git::Git;

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

pub trait Vcs {
    fn kind(&self) -> Kind;
    fn root(&self) -> &Path;
    /// Newest first. `None` uses the default: `HEAD` for git, the user's
    /// `revsets.log` for jj.
    fn log(&self, revset: Option<&str>, limit: usize) -> Result<Vec<CommitInfo>>;
    /// `commit` is a commit ID (as returned by `log`) or, for git, any
    /// revision naming one commit.
    fn show(&self, commit: &str) -> Result<CommitDiff>;
}

/// Find the repository holding `path`: the nearest directory with `.jj`
/// (jj wins in a colocated repo), else the nearest with `.git`.
pub fn open(path: &Path) -> Result<Box<dyn Vcs>> {
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
    if find_up(&start, ".git").is_some() {
        return Ok(Box::new(Git::open(&start)?));
    }
    bail!("not a git or jj repository: {}", path.display())
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
