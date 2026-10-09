//! git backend: the `git` CLI with read-only flags.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use super::{CommitDiff, CommitInfo, Kind, Vcs, parse_unified, run};

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
    "color.ui=never",
];

/// Flags for every command that prints a diff.
const DIFF_FLAGS: &[&str] = &[
    "--no-textconv",
    "--no-ext-diff",
    "--no-color",
    "--default-prefix",
];

pub struct Git {
    root: PathBuf,
}

impl Git {
    /// Open the git repository containing `dir`.
    pub fn open(dir: &Path) -> Result<Self> {
        let top = git_in(dir, &["rev-parse", "--show-toplevel"])?;
        let top = PathBuf::from(top.trim_end_matches('\n'));
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

    fn log(&self, revset: Option<&str>, limit: usize) -> Result<Vec<CommitInfo>> {
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

    fn show(&self, commit: &str) -> Result<CommitDiff> {
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
        args.extend_from_slice(DIFF_FLAGS);
        args.extend_from_slice(&[&id, "--"]);
        let patch = self.git(&args)?;
        Ok(CommitDiff {
            message: message.trim_end_matches('\n').to_string(),
            files: parse_unified(&patch),
        })
    }
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
