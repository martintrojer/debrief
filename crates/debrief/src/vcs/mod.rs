//! Read-only access to git and jj repositories through their CLIs.

mod diff;

use std::path::Path;

use anyhow::Result;

pub use diff::parse_unified;

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
