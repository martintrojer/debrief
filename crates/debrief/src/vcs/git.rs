//! git backend: the `git` CLI with read-only flags.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use super::{
    CommitDiff, CommitInfo, CommitStat, Context as DiffContext, DiffLine, FULL_CONTEXT, FileDiff,
    FileStat, Hunk, Kind, LineKind, OpenOptions, Vcs, WorkingCopy, parse_unified, run,
};

/// Bytes of an untracked file sniffed for a NUL (binary).
const SNIFF: usize = 8 * 1024;
/// Untracked files larger than this are listed, not shown.
pub const MAX_UNTRACKED: u64 = 1024 * 1024;
/// The placeholder for an untracked file over [`MAX_UNTRACKED`].
pub const TOO_LARGE: &str = "file too large to show (over 1 MiB)";
/// The placeholder for an untracked file that is neither a regular file
/// nor a symlink.
pub const SPECIAL_FILE: &str = "(special file)";

/// Config overrides on every call: fixed diff prefixes and quoting, and
/// no extra output (signatures, colour) in machine-read streams.
const CONFIG: &[&str] = &[
    "diff.noprefix=false",
    "diff.mnemonicPrefix=false",
    "diff.relative=false",
    "core.quotePath=true",
    "core.fsmonitor=false",
    "log.showSignature=false",
    "log.showRoot=true",
    "i18n.logOutputEncoding=UTF-8",
    "color.ui=never",
];

/// Flags for every command that prints a diff.
const DIFF_FLAGS: &[&str] = &[
    "--no-textconv",
    "--no-ext-diff",
    "--no-color",
    "--default-prefix",
    "--submodule=short",
    "--ignore-submodules=none",
    "-O/dev/null",
];

pub struct Git {
    root: PathBuf,
}

impl Git {
    /// Open the git repository containing `dir`.
    pub fn open(dir: &Path) -> Result<Self> {
        let top = git_in(dir, &["rev-parse", "--show-toplevel"])?;
        // Strip only the one line terminator: a root name may itself end
        // in newlines.
        let top = PathBuf::from(top.strip_suffix('\n').unwrap_or(&top));
        let root = top
            .canonicalize()
            .with_context(|| format!("cannot open {}", top.display()))?;
        Ok(Git { root })
    }

    fn git(&self, args: &[&str]) -> Result<String> {
        git_in(&self.root, args)
    }

    /// Resolve any revision to one full commit ID, without letting the
    /// input be read as an option.
    fn resolve(&self, rev: &str) -> Result<String> {
        let spec = format!("{rev}^{{commit}}");
        let id = self.git(&[
            "rev-parse",
            "--verify",
            "--quiet",
            "--end-of-options",
            &spec,
        ])?;
        let id = id.trim();
        if id.is_empty() || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
            bail!("not a commit: {rev}");
        }
        Ok(id.to_string())
    }

    /// `HEAD`'s commit ID, `None` before the first commit.
    fn head(&self) -> Option<String> {
        self.resolve("HEAD").ok()
    }

    /// The empty tree's ID for this repo's hash (no object is written).
    fn empty_tree(&self) -> Result<String> {
        let out = self.git(&["hash-object", "-t", "tree", "/dev/null"])?;
        Ok(out.trim().to_string())
    }

    /// Untracked, non-ignored paths (repo-relative).
    fn untracked(&self) -> Result<Vec<String>> {
        let out = self.git(&["ls-files", "-z", "--others", "--exclude-standard", "--"])?;
        Ok(out
            .split('\0')
            .filter(|p| !p.is_empty())
            .map(str::to_string)
            .collect())
    }
}

/// An untracked file as an added [`FileDiff`] (spec K10): regular files
/// read up to [`MAX_UNTRACKED`] (binary when a NUL is in the first 8 KiB),
/// a symlink as one line holding its target (never followed), anything
/// else a placeholder.
pub fn untracked_file(root: &Path, path: &str) -> FileDiff {
    use std::io::Read;
    let mut f = FileDiff {
        old_path: None,
        new_path: Some(path.to_string()),
        binary: false,
        hunks: Vec::new(),
        note: None,
        mode_change: None,
        copied: false,
    };
    let abs = root.join(path);
    let meta = match std::fs::symlink_metadata(&abs) {
        Ok(m) => m,
        Err(e) => {
            f.note = Some(format!("cannot read: {e}"));
            return f;
        }
    };
    let text = if meta.file_type().is_symlink() {
        match std::fs::read_link(&abs) {
            Ok(t) => {
                let t = t.to_string_lossy().into_owned();
                f.hunks = vec![added_hunk(vec![t], true)];
                return f;
            }
            Err(e) => {
                f.note = Some(format!("cannot read: {e}"));
                return f;
            }
        }
    } else if !meta.is_file() {
        f.note = Some(SPECIAL_FILE.into());
        return f;
    } else {
        let mut bytes = Vec::new();
        let read = std::fs::File::open(&abs)
            .and_then(|file| file.take(MAX_UNTRACKED + 1).read_to_end(&mut bytes));
        if let Err(e) = read {
            f.note = Some(format!("cannot read: {e}"));
            return f;
        }
        if bytes[..bytes.len().min(SNIFF)].contains(&0) {
            f.binary = true;
            return f;
        }
        if bytes.len() as u64 > MAX_UNTRACKED {
            f.note = Some(TOO_LARGE.into());
            return f;
        }
        String::from_utf8_lossy(&bytes).into_owned()
    };
    if text.is_empty() {
        return f;
    }
    let no_newline = !text.ends_with('\n');
    let lines: Vec<String> = text.lines().map(str::to_string).collect();
    f.hunks = vec![added_hunk(lines, no_newline)];
    f
}

fn added_hunk(lines: Vec<String>, no_newline: bool) -> Hunk {
    let n = lines.len();
    let last = n.saturating_sub(1);
    Hunk {
        header: format!("@@ -0,0 +1,{n} @@"),
        old_start: 0,
        new_start: 1,
        lines: lines
            .into_iter()
            .enumerate()
            .map(|(i, text)| DiffLine {
                kind: LineKind::Added,
                old: None,
                new: Some(i as u32 + 1),
                text,
                no_newline: no_newline && i == last,
            })
            .collect(),
    }
}

/// `-U<n>` for a full-file diff; nothing (git's default) for hunks.
fn context_flag(context: DiffContext) -> Option<String> {
    (context == DiffContext::Full).then(|| format!("-U{FULL_CONTEXT}"))
}

fn git_in(dir: &Path, args: &[&str]) -> Result<String> {
    let mut argv: Vec<&OsStr> = vec![
        "--no-optional-locks".as_ref(),
        "--no-lazy-fetch".as_ref(),
        "-C".as_ref(),
        dir.as_os_str(),
    ];
    for c in CONFIG {
        argv.push("-c".as_ref());
        argv.push(c.as_ref());
    }
    argv.extend(args.iter().map(OsStr::new));
    run("git", argv)
}

impl Vcs for Git {
    fn kind(&self) -> Kind {
        Kind::Git
    }

    fn root(&self) -> &Path {
        &self.root
    }

    fn open_options(&self) -> OpenOptions {
        OpenOptions { snapshot: false }
    }

    fn log(&self, revset: Option<&str>, limit: usize) -> Result<Vec<CommitInfo>> {
        // Before the first commit the default log is empty; a named
        // revision still has to exist.
        if revset.is_none() && self.head().is_none() {
            return Ok(Vec::new());
        }
        let limit = limit.to_string();
        let rev = revset.unwrap_or("HEAD");
        let out = self.git(&[
            "log",
            "-z",
            "--no-color",
            "--format=%H%x1f%an%x1f%at%x1f%s%x1f%P",
            "-n",
            &limit,
            "--end-of-options",
            rev,
            "--",
        ])?;
        out.split('\0')
            .filter(|r| !r.trim().is_empty())
            .map(parse_record)
            .collect()
    }

    fn show(&self, commit: &str, context: DiffContext) -> Result<CommitDiff> {
        let id = self.resolve(commit)?;
        let message = self.git(&["show", "-s", "--no-color", "--format=%B", &id, "--"])?;
        let mut args = vec![
            "show",
            "--first-parent",
            "--root",
            "--format=",
            "--patch",
            "-M",
        ];
        let unified = context_flag(context);
        args.extend(unified.as_deref());
        args.extend_from_slice(DIFF_FLAGS);
        args.extend_from_slice(&[&id, "--"]);
        let patch = self.git(&args)?;
        Ok(CommitDiff {
            message: message.trim_end_matches('\n').to_string(),
            files: parse_unified(&patch),
        })
    }

    fn stat(&self, commit: &str) -> Result<CommitStat> {
        let id = self.resolve(commit)?;
        let mut args = vec![
            "show",
            "--first-parent",
            "--root",
            "--numstat",
            "-z",
            "-M",
            "--format=%an%x00%ae%x00%at%x00%B%x00",
        ];
        args.extend_from_slice(DIFF_FLAGS);
        args.extend_from_slice(&[&id, "--"]);
        parse_numstat(&self.git(&args)?)
    }

    fn working_copy(&self, context: DiffContext) -> Result<Option<WorkingCopy>> {
        let head = self.head();
        let against = match &head {
            Some(id) => id.clone(),
            None => self.empty_tree()?,
        };
        // `diff-index` (plumbing), not `diff`: porcelain `git diff`
        // refreshes the index's stat data even with --no-optional-locks.
        let mut args = vec!["diff-index", "--patch", "-M"];
        let unified = context_flag(context);
        args.extend(unified.as_deref());
        args.extend_from_slice(DIFF_FLAGS);
        args.extend_from_slice(&[&against, "--"]);
        let patch = self.git(&args)?;
        let mut files = parse_unified(&patch);
        for p in self.untracked()? {
            files.push(untracked_file(&self.root, &p));
        }
        if files.is_empty() {
            return Ok(None);
        }
        let base = match head {
            Some(id) => self.log(Some(&id), 1)?.into_iter().next(),
            None => None,
        };
        Ok(Some(WorkingCopy {
            base,
            files: files.len(),
            diff: CommitDiff {
                message: String::new(),
                files,
            },
        }))
    }
}

/// `git show --numstat -z` with the format `%an NUL %ae NUL %at NUL %B
/// NUL`: the header fields, then one record per file, `added TAB removed
/// TAB path NUL`, or for a rename `added TAB removed TAB NUL old NUL new
/// NUL`; binary files count `-`.
fn parse_numstat(out: &str) -> Result<CommitStat> {
    let mut f = out.splitn(5, '\0');
    let (Some(author), Some(email), Some(time), Some(message)) =
        (f.next(), f.next(), f.next(), f.next())
    else {
        bail!("unexpected git show output: {out:?}");
    };
    let time = time
        .trim()
        .parse()
        .with_context(|| format!("bad time in git show: {time:?}"))?;
    // Records start with a digit or `-`; before them git puts the header
    // terminator and a newline.
    let rest = f.next().unwrap_or("").trim_start_matches(['\0', '\n']);
    let mut fields = rest.split('\0');
    let mut files = Vec::new();
    while let Some(rec) = fields.next() {
        if rec.is_empty() {
            continue;
        }
        let mut parts = rec.splitn(3, '\t');
        let (Some(add), Some(del), Some(path)) = (parts.next(), parts.next(), parts.next()) else {
            bail!("unexpected numstat record: {rec:?}");
        };
        let count = |s: &str| -> Result<Option<u32>> {
            if s == "-" {
                return Ok(None);
            }
            Ok(Some(
                s.parse()
                    .with_context(|| format!("bad numstat count: {s:?}"))?,
            ))
        };
        let (old_path, path) = if path.is_empty() {
            let (Some(old), Some(new)) = (fields.next(), fields.next()) else {
                bail!("truncated numstat rename: {rec:?}");
            };
            (Some(old.to_string()), new.to_string())
        } else {
            (None, path.to_string())
        };
        files.push(FileStat {
            path,
            old_path,
            added: count(add)?,
            removed: count(del)?,
        });
    }
    Ok(CommitStat {
        message: message.trim_end_matches('\n').to_string(),
        author: author.to_string(),
        email: email.to_string(),
        time,
        files,
    })
}

fn parse_record(rec: &str) -> Result<CommitInfo> {
    let rec = rec.trim_start_matches('\n');
    let f: Vec<&str> = rec.split('\x1f').collect();
    let [commit, author, time, summary, parents] = f[..] else {
        bail!("unexpected git log record: {rec:?}");
    };
    Ok(CommitInfo {
        commit: commit.to_string(),
        change: None,
        author: author.to_string(),
        time: time
            .parse()
            .with_context(|| format!("bad time in git log: {time:?}"))?,
        summary: summary.to_string(),
        parents: parents.split_whitespace().map(str::to_string).collect(),
    })
}
