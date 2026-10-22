//! Read-only access to git and jj repositories through their CLIs.
//!
//! Every read runs with flags that stop the tool from writing to the
//! repository (no optional locks, no working-copy snapshot, no op-head
//! merge, no textconv cache). The one exception is jj's snapshot
//! ([`Vcs::refresh`]): it leaves files alone but records the
//! working copy as a new operation, as any jj command does;
//! [`OpenOptions::snapshot`] turns it off.

mod diff;
mod git;
mod jj;

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use anyhow::{Context as _, Result, bail};

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
    /// `(old, new)` file modes when the mode changed (`old mode` /
    /// `new mode` lines), e.g. `("100644", "100755")`.
    pub mode_change: Option<(String, String)>,
    /// A copy (`copy from` / `copy to`), not a rename.
    pub copied: bool,
}

/// A commit as `git log --stat` shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitStat {
    pub message: String,
    pub author: String,
    pub email: String,
    /// Author time, unix seconds.
    pub time: i64,
    pub files: Vec<FileStat>,
}

/// One file of a [`CommitStat`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileStat {
    /// The new path (the old one when the file was deleted).
    pub path: String,
    /// The old path of a rename or copy.
    pub old_path: Option<String>,
    /// Added lines; `None` for a binary file.
    pub added: Option<u32>,
    /// Removed lines; `None` for a binary file.
    pub removed: Option<u32>,
}

/// Per-file line counts of a parsed diff (jj's stat, git's working copy).
pub fn file_stats(files: &[FileDiff]) -> Vec<FileStat> {
    files
        .iter()
        .map(|f| {
            let path = f
                .new_path
                .clone()
                .or_else(|| f.old_path.clone())
                .unwrap_or_default();
            let old_path = match (&f.old_path, &f.new_path) {
                (Some(o), Some(n)) if o != n => Some(o.clone()),
                _ => None,
            };
            let (mut add, mut del) = (0u32, 0u32);
            for l in f.hunks.iter().flat_map(|h| &h.lines) {
                match l.kind {
                    LineKind::Added => add += 1,
                    LineKind::Removed => del += 1,
                    LineKind::Context => {}
                }
            }
            let binary = f.binary && f.hunks.is_empty();
            FileStat {
                path,
                old_path,
                added: (!binary).then_some(add),
                removed: (!binary).then_some(del),
            }
        })
        .collect()
}

/// What happened to a file, as git's status letters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
    Copied,
}

impl FileStatus {
    /// `A`, `M`, `D`, `R` or `C`.
    pub fn letter(self) -> char {
        match self {
            FileStatus::Added => 'A',
            FileStatus::Modified => 'M',
            FileStatus::Deleted => 'D',
            FileStatus::Renamed => 'R',
            FileStatus::Copied => 'C',
        }
    }
}

impl FileDiff {
    pub fn status(&self) -> FileStatus {
        match (&self.old_path, &self.new_path) {
            (None, _) => FileStatus::Added,
            (_, None) => FileStatus::Deleted,
            _ if self.copied => FileStatus::Copied,
            (Some(o), Some(n)) if o != n => FileStatus::Renamed,
            _ => FileStatus::Modified,
        }
    }
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

/// How much unchanged text a diff shows around its changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Context {
    /// The tool's default context (hunks).
    #[default]
    Default,
    /// The whole file: a diff with [`FULL_CONTEXT`] lines of context.
    Full,
}

/// Lines of context of a [`Context::Full`] diff (more than any file has).
pub const FULL_CONTEXT: u32 = 1_000_000;

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

/// git's uncommitted changes.
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

/// git's working-copy row: whether the tree is dirty, and how many files,
/// from cheap commands (no patch text, no untracked file read).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkingStatus {
    /// `HEAD`; `None` before the first commit.
    pub base: Option<CommitInfo>,
    /// Number of changed files, untracked ones included.
    pub files: usize,
}

impl From<&WorkingCopy> for WorkingStatus {
    fn from(w: &WorkingCopy) -> Self {
        WorkingStatus {
            base: w.base.clone(),
            files: w.files,
        }
    }
}

/// [`Vcs::branch_revset`]'s error when the branch itself does not exist
/// (as opposed to a trunk that can't be read): the caller falls back.
#[derive(Debug)]
pub struct NoSuchBranch(pub String);

impl std::fmt::Display for NoSuchBranch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "no such branch: {}", self.0)
    }
}

impl std::error::Error for NoSuchBranch {}

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
    fn working_copy(&self, _context: Context) -> Result<Option<WorkingCopy>> {
        Ok(None)
    }
    /// git: [`Vcs::working_copy`]'s row, `None` when the tree is clean,
    /// without reading the diff or any untracked file. jj: `None`.
    fn working_status(&self) -> Result<Option<WorkingStatus>> {
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
    /// Newest first. `None` uses the default, the current commit and its
    /// ancestors: `HEAD` for git, `::@` for jj (jj's root commit included).
    fn log(&self, revset: Option<&str>, limit: usize) -> Result<Vec<CommitInfo>>;
    /// [`Vcs::log`] narrowed to the commits touching `path` (repo-relative,
    /// `/`-separated: a file, or everything under a directory). Renames
    /// are not followed. A backend without path support refuses a path.
    ///
    /// `spine`: only the first-parent spine of what it lists (a merge
    /// stands for its bubble; see [`Vcs::fold`]). With a path, git lists a
    /// spine merge when its first-parent diff touches the path
    /// (`--first-parent -- <path>`); jj when a commit in its bubble does,
    /// so jj also lists a bubble that changes the path and back (no net
    /// change). A backend without merges ignores it.
    fn log_in(
        &self,
        revset: Option<&str>,
        path: Option<&str>,
        limit: usize,
        _spine: bool,
    ) -> Result<Vec<CommitInfo>> {
        match path {
            None => self.log(revset, limit),
            Some(p) => bail!("cannot filter the log by path: {p}"),
        }
    }
    /// One group of merge `merge`'s fold: its parent `parent`'s own
    /// first-parent spine, down to where it meets `merge`'s first parent,
    /// newest first, narrowed to `path` as [`Vcs::log_in`] narrows the
    /// spine. IDs as returned by `log`. A backend without merges lists
    /// none.
    fn fold(
        &self,
        _merge: &str,
        _parent: &str,
        _path: Option<&str>,
        _limit: usize,
    ) -> Result<Vec<CommitInfo>> {
        Ok(Vec::new())
    }
    /// How many commits `merge` brought in (`p1..merge`), nested bubbles
    /// included, the merge itself excluded. A backend without merges: 0.
    fn merge_size(&self, _merge: &str) -> Result<usize> {
        Ok(0)
    }
    /// Every commit `merge` brought in, as [`Vcs::merge_size`] counts
    /// them, newest first, at most `limit`, no path filter. A backend
    /// without merges lists none.
    fn bubble(&self, _merge: &str, _limit: usize) -> Result<Vec<CommitInfo>> {
        Ok(Vec::new())
    }
    /// git: [`Vcs::working_status`], `None` unless something under `path`
    /// (when given) is changed or untracked; the count is still the whole
    /// tree's. Others: [`Vcs::working_status`].
    fn working_status_in(&self, _path: Option<&str>) -> Result<Option<WorkingStatus>> {
        self.working_status()
    }
    /// Local branches (git) or local bookmarks (jj), most recently
    /// committed first. A backend without branches lists none.
    fn branches(&self) -> Result<Vec<String>> {
        Ok(Vec::new())
    }
    /// The checked-out branch: git's `HEAD` branch (`None` detached); jj's
    /// one bookmark on `@`, else the one on `@-`, else `None`.
    fn current_branch(&self) -> Result<Option<String>> {
        Ok(None)
    }
    /// The revset (jj) or revision range (git) listing `branch`: its
    /// commits not in trunk, or its whole history when it is in trunk or
    /// no trunk is found. `trunk` overrides trunk detection (a revision
    /// for git, a revset for jj). A branch that doesn't exist (by exact
    /// name) is a [`NoSuchBranch`] error. `with_working_copy` (jj, the current
    /// bookmark): `@` is listed too.
    fn branch_revset(
        &self,
        branch: &str,
        _trunk: Option<&str>,
        _with_working_copy: bool,
    ) -> Result<String> {
        Err(NoSuchBranch(branch.to_string()).into())
    }
    /// `commit` is a commit ID (as returned by `log`) or, for git, any
    /// revision naming one commit. `context`: hunks or the whole file.
    fn show(&self, commit: &str, context: Context) -> Result<CommitDiff>;
    /// The message, author and per-file line counts of `commit` (same
    /// IDs as [`Vcs::show`]; merges against the first parent).
    fn stat(&self, commit: &str) -> Result<CommitStat>;
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

/// `path` (inside the repository at `root`, which is canonical) as the
/// log's path filter: repo-relative and `/`-separated. `None` for the
/// root itself.
pub fn path_in(root: &Path, path: &Path) -> Result<Option<String>> {
    let canon = path
        .canonicalize()
        .with_context(|| format!("cannot open {}", path.display()))?;
    // A symlink inside the repo names itself, not its target; one outside
    // it (an alias for the repo or a directory in it) names its target.
    let link = match (path.parent(), path.file_name()) {
        (Some(dir), Some(name)) if path.is_symlink() => {
            let dir = if dir.as_os_str().is_empty() {
                Path::new(".")
            } else {
                dir
            };
            dir.canonicalize()
                .ok()
                .map(|d| d.join(name))
                .filter(|p| p.starts_with(root))
        }
        _ => None,
    };
    let at = link.unwrap_or(canon);
    let rel = at
        .strip_prefix(root)
        .with_context(|| format!("{} is outside {}", path.display(), root.display()))?;
    let parts = rel
        .components()
        .map(|c| {
            c.as_os_str()
                .to_str()
                .with_context(|| format!("path is not UTF-8: {}", path.display()))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok((!parts.is_empty()).then(|| parts.join("/")))
}

/// `file` (repo-relative) is `path` or lies under the directory `path`.
pub fn under(file: &str, path: &str) -> bool {
    file.strip_prefix(path)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
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

static GIT_PROGRAM: OnceLock<OsString> = OnceLock::new();

/// Set the git program for the process from `[vcs] git` (`$DEBRIEF_GIT`
/// wins); call once, before [`open`]. Unset, [`git_program`] is
/// `$DEBRIEF_GIT` or `git`.
pub fn set_git_program(config: &str) {
    let _ = GIT_PROGRAM.set(resolve_git_program(std::env::var_os("DEBRIEF_GIT"), config));
}

/// The git program every git command runs: a name looked up on `PATH`,
/// or a path.
pub fn git_program() -> &'static OsStr {
    GIT_PROGRAM.get_or_init(|| resolve_git_program(std::env::var_os("DEBRIEF_GIT"), "git"))
}

/// `env` (`$DEBRIEF_GIT`) when set and non-empty, else `config`.
fn resolve_git_program(env: Option<OsString>, config: &str) -> OsString {
    env.filter(|e| !e.is_empty())
        .unwrap_or_else(|| config.into())
}

/// Run a VCS command and return its stdout; a failure carries stderr.
fn run<P, I, S>(program: P, args: I) -> Result<String>
where
    P: AsRef<OsStr>,
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let program = program.as_ref();
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
        .with_context(|| format!("failed to run {}", program.display()))?;
    if !out.status.success() {
        let args: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        bail!(
            "{} {} failed ({}): {}",
            program.display(),
            args.join(" "),
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debrief_git_wins_over_config() {
        let env = |s: &str| Some(OsString::from(s));
        assert_eq!(resolve_git_program(None, "git"), "git");
        assert_eq!(resolve_git_program(None, "/opt/git"), "/opt/git");
        assert_eq!(resolve_git_program(env("/env/git"), "/opt/git"), "/env/git");
        assert_eq!(resolve_git_program(env(""), "/opt/git"), "/opt/git");
    }
}
