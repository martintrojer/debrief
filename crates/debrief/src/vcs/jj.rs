//! jj backend: the `jj` CLI with read-only flags.
//!
//! `--ignore-working-copy` stops jj snapshotting the working copy (and
//! importing git refs in a colocated repo). Every call is also pinned to
//! one operation with `--at-op`: jj would otherwise merge divergent
//! operation heads, and even `--at-op=@` takes a lock file in
//! `op_heads/heads` when the heads diverge. So we read the heads
//! ourselves, refuse when they diverge, and pass the single head's ID.
//!
//! With snapshots on (the default), [`Vcs::refresh`] runs
//! `jj util snapshot` through its own command builder, with neither
//! `--ignore-working-copy` nor `--at-op`, then pins reads to the one op
//! head it leaves. That pin is kept: when a later snapshot fails (another
//! jj holding the lock, say) reads stay on it and the error is returned.
//! `refresh(false)` (the op-heads watcher) only moves the pin to the
//! current head. With `--no-snapshot` there is no pin: every call reads
//! the op head, as before.

use std::cell::RefCell;
use std::collections::HashSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use super::{
    CommitDiff, CommitInfo, CommitStat, Context as DiffContext, FULL_CONTEXT, Kind, NoSuchBranch,
    OpenOptions, Vcs, file_stats, parse_unified, run,
};

const LOG_TEMPLATE: &str = r#"commit_id ++ "\x1f" ++ change_id ++ "\x1f" ++ author.name() ++ "\x1f" ++ author.timestamp().format("%s") ++ "\x1f" ++ description.first_line() ++ "\x1f" ++ parents.map(|c| c.commit_id()).join(" ") ++ "\x1e""#;

const STAT_TEMPLATE: &str = r#"author.name() ++ "\x1f" ++ author.email() ++ "\x1f" ++ author.timestamp().format("%s") ++ "\x1f" ++ description"#;

/// A workspace-relative path as a fileset: the file, or everything under
/// the directory, quoted so no character is read as an operator.
fn fileset(path: &str) -> String {
    let quoted: String = path
        .chars()
        .flat_map(|c| match c {
            '"' | '\\' => vec!['\\', c],
            c => vec![c],
        })
        .collect();
    format!("root:\"{quoted}\"")
}

/// `s` as a jj string literal.
fn quote(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The tips a branch's log starts from: the bookmark, and `@` with it
/// when the working copy is on that bookmark's line.
fn heads_of(tip: &str, with_working_copy: bool) -> String {
    if with_working_copy {
        format!("({tip} | @)")
    } else {
        tip.to_string()
    }
}

/// The revset of local bookmark `name`.
fn bookmark(name: &str) -> String {
    format!("bookmarks(exact:{})", quote(name))
}

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

    /// `jj util snapshot`, without the read-only flags.
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

    /// The commit IDs `revset` lists, newest first, at most `limit`.
    fn ids(&self, at_op: &str, revset: &str, limit: Option<usize>) -> Result<Vec<String>> {
        let rev = format!("--revisions={revset}");
        let n = limit.map(|n| n.to_string());
        let mut args = vec!["log", "--no-graph", &rev, "-T", r#"commit_id ++ "\n""#];
        if let Some(n) = &n {
            args.extend(["-n", n]);
        }
        let out = self.jj(at_op, &args)?;
        Ok(out
            .lines()
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .collect())
    }

    /// The first-parent chain `s` (newest first), narrowed to `path` as
    /// git's `--first-parent -- <path>` narrows it: a commit touching the
    /// path, and a merge whose bubble does. jj's `files()` matches a merge
    /// on its diff against the auto-merge of its parents (empty for a
    /// clean merge), so each merge's bubble is checked on its own:
    /// `m & ((::m ~ ::first_parent(m)) & files(P))::` is `m` exactly when
    /// the bubble touches `P`. Two jj calls list the first `limit` direct
    /// hits and, in spine order, every merge newer than the last of them;
    /// the merges are then checked newest first, [`MERGE_BATCH`] a call,
    /// until the page is full down to the oldest merge checked, and one
    /// call logs the page. A path touched in fewer than `limit` places
    /// (rare, absent, deep) fills no page, so it checks every spine merge:
    /// one call per [`MERGE_BATCH`] merges.
    fn spine_log(
        &self,
        at_op: &str,
        s: &str,
        path: Option<&str>,
        limit: usize,
    ) -> Result<Vec<CommitInfo>> {
        let n = limit.to_string();
        let log = |revset: &str| -> Result<Vec<CommitInfo>> {
            let rev = format!("--revisions={revset}");
            parse_log(&self.jj(
                at_op,
                &["log", "--no-graph", "-n", &n, "-T", LOG_TEMPLATE, &rev],
            )?)
        };
        let Some(p) = path else {
            return log(s);
        };
        let files = format!("files({})", fileset(p));
        let direct = self.ids(at_op, &format!("({s}) & {files}"), Some(limit))?;
        // Anything older than a full page of direct hits can't make the
        // page.
        let newer = match direct.last() {
            Some(last) if direct.len() == limit => format!(" & {last}::"),
            _ => String::new(),
        };
        // Spine order of the direct hits and the merges, naming the hits
        // rather than evaluating `files()` again (costly for a deep path).
        let named: String = direct.iter().map(|d| format!(" | {d}")).collect();
        let spine = self.ids(at_op, &format!("({s}) & (merges(){named}){newer}"), None)?;
        let direct: HashSet<&str> = direct.iter().map(String::as_str).collect();
        let merges: Vec<usize> = (0..spine.len())
            .filter(|&i| !direct.contains(spine[i].as_str()))
            .collect();
        let mut touched = Vec::new();
        for (k, batch) in merges.chunks(MERGE_BATCH).enumerate() {
            let union = batch
                .iter()
                .map(|&i| {
                    let m = &spine[i];
                    format!("({m} & (({}) & {files})::)", bubble_of(m))
                })
                .collect::<Vec<_>>()
                .join(" | ");
            touched.extend(self.ids(at_op, &union, None)?);
            // Every hit from the oldest merge checked up is known: a full
            // page there is the page. Of the spine entries newer than it,
            // all but the merges before it are direct hits.
            let (oldest, merges_before) =
                (batch[batch.len() - 1], k * MERGE_BATCH + batch.len() - 1);
            if oldest - merges_before + touched.len() >= limit {
                break;
            }
        }
        if touched.is_empty() {
            return log(&format!("({s}) & {files}"));
        }
        log(&format!("({s}) & ({files} | {})", touched.join(" | ")))
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
        self.log_in(revset, None, limit, false)
    }

    /// `spine`: the revset `R` (default `::@`) narrowed to the first
    /// ancestors of its one head `h`, `first_ancestors(h) & (R)`. When
    /// `R` has several heads (`trunk()..(b | @)` with `@` off `b`, say)
    /// there is no single spine and `R` is listed flat.
    fn log_in(
        &self,
        revset: Option<&str>,
        path: Option<&str>,
        limit: usize,
        spine: bool,
    ) -> Result<Vec<CommitInfo>> {
        let op = self.at_op()?;
        let r = revset.unwrap_or("::@");
        if spine {
            let heads = self.ids(&op, &format!("heads({r})"), Some(2))?;
            if let [h] = &heads[..] {
                let s = format!("first_ancestors({h}) & ({r})");
                return self.spine_log(&op, &s, path, limit);
            }
        }
        // One argv element, so the revset can never be read as a flag.
        let rev = format!("--revisions={r}");
        let fileset = path.map(fileset);
        let limit = limit.to_string();
        let mut args = vec!["log", "--no-graph", "-n", &limit, "-T", LOG_TEMPLATE, &rev];
        if let Some(f) = &fileset {
            args.extend(["--", f]);
        }
        parse_log(&self.jj(&op, &args)?)
    }

    fn fold(
        &self,
        merge: &str,
        parent: &str,
        path: Option<&str>,
        limit: usize,
    ) -> Result<Vec<CommitInfo>> {
        let (merge, parent) = (symbol(merge)?, symbol(parent)?);
        let s = format!("first_ancestors({parent}) ~ ::first_parent({merge})");
        self.spine_log(&self.at_op()?, &s, path, limit)
    }

    fn merge_size(&self, merge: &str) -> Result<usize> {
        let rev = format!("--revisions={}", bubble_of(symbol(merge)?));
        let out = self.jj(&self.at_op()?, &["log", "--no-graph", &rev, "-T", r#""x""#])?;
        Ok(out.len())
    }

    fn bubble(&self, merge: &str, limit: usize) -> Result<Vec<CommitInfo>> {
        let rev = format!("--revisions={}", bubble_of(symbol(merge)?));
        let limit = limit.to_string();
        let args = ["log", "--no-graph", "-n", &limit, "-T", LOG_TEMPLATE, &rev];
        parse_log(&self.jj(&self.at_op()?, &args)?)
    }

    fn branches(&self) -> Result<Vec<String>> {
        let out = self.jj(
            &self.at_op()?,
            &[
                "bookmark",
                "list",
                "--sort=committer-date-",
                "-T",
                r#"if(!remote, stringify(name) ++ "\n")"#,
            ],
        )?;
        let mut names: Vec<String> = Vec::new();
        for l in out.lines().filter(|l| !l.is_empty()) {
            if !names.iter().any(|n| n == l) {
                names.push(l.to_string());
            }
        }
        Ok(names)
    }

    fn current_branch(&self) -> Result<Option<String>> {
        let op = self.at_op()?;
        for rev in ["--revisions=@", "--revisions=@-"] {
            let out = self.jj(
                &op,
                &[
                    "log",
                    "--no-graph",
                    rev,
                    "-T",
                    r#"local_bookmarks.map(|b| stringify(b.name()) ++ "\n").join("")"#,
                ],
            )?;
            let names: Vec<&str> = out.lines().filter(|l| !l.is_empty()).collect();
            match names[..] {
                [] => continue,
                [one] => return Ok(Some(one.to_string())),
                _ => return Ok(None),
            }
        }
        Ok(None)
    }

    fn branch_revset(
        &self,
        branch: &str,
        trunk: Option<&str>,
        with_working_copy: bool,
    ) -> Result<String> {
        let op = self.at_op()?;
        let tip = bookmark(branch);
        let has = |revset: &str| -> Result<bool> {
            let rev = format!("--revisions={revset}");
            let out = self.jj(&op, &["log", "--no-graph", "-n", "1", &rev, "-T", r#""x""#])?;
            Ok(!out.is_empty())
        };
        if !has(&tip)? {
            return Err(NoSuchBranch(branch.to_string()).into());
        }
        // `trunk()` is the root commit when no remote names a trunk: then a
        // local `main` or `master` bookmark is trunk, as for git.
        let trunk = match trunk {
            Some(t) => format!("({t})"),
            None if has("trunk() ~ root()")? => "(trunk())".to_string(),
            None => match ["main", "master"]
                .into_iter()
                .find(|b| has(&bookmark(b)).unwrap_or(false))
            {
                Some(b) if b == branch => {
                    return Ok(format!("::{}", heads_of(&tip, with_working_copy)));
                }
                Some(b) => format!("({})", bookmark(b)),
                None => return Ok(format!("::{}", heads_of(&tip, with_working_copy))),
            },
        };
        let heads = heads_of(&tip, with_working_copy);
        // Trunk's own bookmark (`main` for `main@origin`), even when ahead
        // of it, and a bookmark merged into trunk: the whole history.
        let own = format!("remote_bookmarks(exact:{}) & {trunk}", quote(branch));
        if has(&own)? || has(&format!("{tip} & ::{trunk}"))? {
            return Ok(format!("::{heads}"));
        }
        Ok(format!("{trunk}..{heads}"))
    }

    fn show(&self, commit: &str, context: DiffContext) -> Result<CommitDiff> {
        // One operation for all three calls: a consistent snapshot.
        let op = self.at_op()?;
        let id = self.resolve(&op, commit)?;
        let rev = format!("--revisions={id}");
        let message = self.jj(&op, &["log", "--no-graph", &rev, "-T", "description"])?;
        let full = format!("--context={FULL_CONTEXT}");
        let (from, to) = first_parent_range(&id);
        let mut args = vec!["diff", "--git", &from, &to];
        if context == DiffContext::Full {
            args.push(&full);
        }
        let patch = self.jj(&op, &args)?;
        Ok(CommitDiff {
            message: message.trim_end_matches('\n').to_string(),
            files: parse_unified(&patch),
        })
    }

    fn stat(&self, commit: &str) -> Result<CommitStat> {
        // `jj diff --stat` is for people; count the git diff instead.
        let op = self.at_op()?;
        let id = self.resolve(&op, commit)?;
        let rev = format!("--revisions={id}");
        let head = self.jj(&op, &["log", "--no-graph", &rev, "-T", STAT_TEMPLATE])?;
        let (author, email, time, message) = parse_stat_header(&head)?;
        let (from, to) = first_parent_range(&id);
        let patch = self.jj(&op, &["diff", "--git", &from, &to])?;
        Ok(CommitStat {
            message,
            author,
            email,
            time,
            files: file_stats(&parse_unified(&patch)),
        })
    }
}

/// `jj diff` arguments diffing commit `id` from its first parent, as git
/// shows a merge; the root commit (no parent) diffs from itself.
fn first_parent_range(id: &str) -> (String, String) {
    (
        format!("--from=coalesce(first_parent({id}), root())"),
        format!("--to={id}"),
    )
}

/// `id` (a commit or change ID) as a revset symbol, refusing anything
/// else so it can't be read as an operator or a flag.
fn symbol(id: &str) -> Result<&str> {
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_alphanumeric()) {
        bail!("not a commit ID: {id:?}");
    }
    Ok(id)
}

/// The commits merge `m` brought in, nested bubbles included, `m`
/// excluded.
fn bubble_of(m: &str) -> String {
    format!("::{m} ~ ::first_parent({m}) ~ {m}")
}

/// Spine merges per revset in [`Jj::spine_log`]'s bubble query, so the
/// argument stays a sane size.
const MERGE_BATCH: usize = 100;

fn parse_log(out: &str) -> Result<Vec<CommitInfo>> {
    out.split('\x1e')
        .filter(|r| !r.trim().is_empty())
        .map(parse_record)
        .collect()
}

/// [`STAT_TEMPLATE`]'s output: author, email, time, then the description.
fn parse_stat_header(out: &str) -> Result<(String, String, i64, String)> {
    let f: Vec<&str> = out.splitn(4, '\x1f').collect();
    let [author, email, time, message] = f[..] else {
        bail!("unexpected jj log output: {out:?}");
    };
    let time = time
        .parse()
        .with_context(|| format!("bad time in jj log: {time:?}"))?;
    Ok((
        author.to_string(),
        email.to_string(),
        time,
        message.trim_end_matches('\n').to_string(),
    ))
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
