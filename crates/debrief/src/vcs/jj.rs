//! jj backend: the `jj` CLI with read-only flags.
//!
//! `--ignore-working-copy` stops jj snapshotting the working copy (and
//! importing git refs in a colocated repo). Every call is also pinned to
//! one operation with `--at-op`: jj would otherwise merge divergent
//! operation heads, and even `--at-op=@` takes a lock file in
//! `op_heads/heads` when the heads diverge. So we read the heads
//! ourselves, refuse when they diverge, and pass the single head's ID.
//!
//! With snapshots on (spec Q21, the default), [`Vcs::refresh`] runs
//! `jj util snapshot` through its own command builder, with neither
//! `--ignore-working-copy` nor `--at-op`, then pins reads to the one op
//! head it leaves. That pin is kept: when a later snapshot fails (another
//! jj holding the lock, say) reads stay on it and the error is returned.
//! `refresh(false)` (the op-heads watcher) only moves the pin to the
//! current head. With `--no-snapshot` there is no pin: every call reads
//! the op head, as before.

use std::cell::RefCell;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use super::{CommitDiff, CommitInfo, Kind, OpenOptions, Vcs, parse_unified, run};

const LOG_TEMPLATE: &str = r#"commit_id ++ "\x1f" ++ change_id ++ "\x1f" ++ author.name() ++ "\x1f" ++ author.timestamp().format("%s") ++ "\x1f" ++ description.first_line() ++ "\x1f" ++ parents.map(|c| c.commit_id()).join(" ") ++ "\x1e""#;

pub struct Jj {
    root: PathBuf,
    opts: OpenOptions,
    /// The operation reads are pinned to after the last good snapshot.
    pin: RefCell<Option<String>>,
}

impl Jj {
    /// `root` is the directory holding `.jj`.
    pub fn new(root: PathBuf, opts: OpenOptions) -> Self {
        Jj {
            root,
            opts,
            pin: RefCell::new(None),
        }
    }

    /// Reads pinned to operation `op` (a reopened handle).
    pub fn pinned(root: PathBuf, opts: OpenOptions, op: String) -> Self {
        Jj {
            root,
            opts,
            pin: RefCell::new(Some(op)),
        }
    }

    /// The operation reads use: the snapshot's pin when there is one,
    /// else the current single op head.
    fn at_op(&self) -> Result<String> {
        if let Some(op) = self.pin.borrow().clone() {
            return Ok(op);
        }
        self.op_head()
    }

    /// The `--at-op` value: the one operation head, or `@` when the op
    /// heads store is not the plain directory jj uses by default.
    fn op_head(&self) -> Result<String> {
        let mut repo = self.root.join(".jj/repo");
        if repo.is_file() {
            // A secondary workspace: `.jj/repo` holds the repo path.
            let target = std::fs::read_to_string(&repo)
                .with_context(|| format!("cannot read {}", repo.display()))?;
            repo = self.root.join(".jj").join(target.trim_end_matches('\n'));
        }
        let store = repo.join("op_heads");
        let kind = std::fs::read_to_string(store.join("type")).unwrap_or_default();
        if kind.trim() != "simple_op_heads_store" {
            return Ok("@".to_string());
        }
        let heads_dir = store.join("heads");
        let mut heads = Vec::new();
        for e in std::fs::read_dir(&heads_dir)
            .with_context(|| format!("cannot read {}", heads_dir.display()))?
        {
            let name = e?.file_name().to_string_lossy().into_owned();
            if !name.is_empty() && name.bytes().all(|b| b.is_ascii_hexdigit()) {
                heads.push(name);
            }
        }
        match &heads[..] {
            [one] => Ok(one.clone()),
            [] => bail!("no jj operation heads in {}", heads_dir.display()),
            _ => bail!(
                "divergent jj operation heads ({}); run any jj command to reconcile them",
                heads.len()
            ),
        }
    }

    /// `jj util snapshot`, without the read-only flags (Q21).
    fn snapshot(&self) -> Result<()> {
        let argv: Vec<&OsStr> = vec![
            "--no-pager".as_ref(),
            "--color".as_ref(),
            "never".as_ref(),
            "--quiet".as_ref(),
            "-R".as_ref(),
            self.root.as_os_str(),
            "util".as_ref(),
            "snapshot".as_ref(),
        ];
        run("jj", argv).map(|_| ())
    }

    fn jj(&self, at_op: &str, args: &[&str]) -> Result<String> {
        let at_op = format!("--at-op={at_op}");
        let mut argv: Vec<&OsStr> = vec![
            "--ignore-working-copy".as_ref(),
            at_op.as_ref(),
            "--no-pager".as_ref(),
            "--color".as_ref(),
            "never".as_ref(),
            "-R".as_ref(),
            self.root.as_os_str(),
        ];
        argv.extend(args.iter().map(OsStr::new));
        run("jj", argv)
    }

    /// Resolve a commit ID (or change ID) to exactly one full commit ID.
    fn resolve(&self, at_op: &str, id: &str) -> Result<String> {
        if id.is_empty() || !id.bytes().all(|b| b.is_ascii_alphanumeric()) {
            bail!("not a commit ID: {id:?}");
        }
        let rev = format!("--revisions={id}");
        let out = self.jj(
            at_op,
            &["log", "--no-graph", &rev, "-T", r#"commit_id ++ "\n""#],
        )?;
        let ids: Vec<&str> = out.lines().filter(|l| !l.is_empty()).collect();
        match ids[..] {
            [one] => Ok(one.to_string()),
            _ => bail!("{id} does not name exactly one commit"),
        }
    }
}

impl Vcs for Jj {
    fn kind(&self) -> Kind {
        Kind::Jj
    }

    fn root(&self) -> &Path {
        &self.root
    }

    fn open_options(&self) -> OpenOptions {
        self.opts
    }

    fn refresh(&self, snapshot: bool) -> Result<()> {
        if !self.opts.snapshot {
            return Ok(());
        }
        let had_pin = self.pin.borrow().is_some();
        let pinned = if snapshot {
            self.snapshot().and_then(|()| self.op_head())
        } else {
            self.op_head()
        };
        match pinned {
            Ok(op) => {
                *self.pin.borrow_mut() = Some(op);
                Ok(())
            }
            Err(e) if !snapshot => Err(e),
            Err(e) if had_pin => Err(e.context("jj snapshot failed; showing the last snapshot")),
            Err(e) => Err(e.context("jj snapshot failed")),
        }
    }

    fn pinned_op(&self) -> Option<String> {
        self.pin.borrow().clone()
    }

    fn working_copy_commit(&self) -> Result<Option<String>> {
        let out = self.jj(
            &self.at_op()?,
            &[
                "log",
                "--no-graph",
                "--revisions=@",
                "-T",
                r#"commit_id ++ "\n""#,
            ],
        )?;
        Ok(out.lines().find(|l| !l.is_empty()).map(str::to_string))
    }

    fn log(&self, revset: Option<&str>, limit: usize) -> Result<Vec<CommitInfo>> {
        let limit = limit.to_string();
        let mut args = vec!["log", "--no-graph", "-n", &limit, "-T", LOG_TEMPLATE];
        // One argv element, so the revset can never be read as a flag.
        let rev;
        if let Some(r) = revset {
            rev = format!("--revisions={r}");
            args.push(&rev);
        }
        let out = self.jj(&self.at_op()?, &args)?;
        out.split('\x1e')
            .filter(|r| !r.trim().is_empty())
            .map(parse_record)
            .collect()
    }

    fn show(&self, commit: &str) -> Result<CommitDiff> {
        // One operation for all three calls: a consistent snapshot.
        let op = self.at_op()?;
        let id = self.resolve(&op, commit)?;
        let rev = format!("--revisions={id}");
        let message = self.jj(&op, &["log", "--no-graph", &rev, "-T", "description"])?;
        let patch = self.jj(&op, &["diff", "--git", &rev])?;
        Ok(CommitDiff {
            message: message.trim_end_matches('\n').to_string(),
            files: parse_unified(&patch),
        })
    }
}

fn parse_record(rec: &str) -> Result<CommitInfo> {
    let rec = rec.trim_start_matches('\n');
    let f: Vec<&str> = rec.split('\x1f').collect();
    let [commit, change, author, time, summary, parents] = f[..] else {
        bail!("unexpected jj log record: {rec:?}");
    };
    Ok(CommitInfo {
        commit: commit.to_string(),
        change: Some(change.to_string()),
        author: author.to_string(),
        time: time
            .parse()
            .with_context(|| format!("bad time in jj log: {time:?}"))?,
        summary: summary.to_string(),
        parents: parents.split_whitespace().map(str::to_string).collect(),
    })
}
