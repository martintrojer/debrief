//! Helpers shared by the integration tests.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::hash::{DefaultHasher, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use debrief::app::App;
use debrief::vcs;

/// Press `code` with no modifiers.
pub fn press(app: &mut App, code: KeyCode) {
    app.key(KeyEvent::new(code, KeyModifiers::NONE));
}

/// Press `C-<c>`.
pub fn ctrl(app: &mut App, c: char) {
    app.key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL));
}

/// Type each char of `s`.
pub fn keys(app: &mut App, s: &str) {
    s.chars().for_each(|c| press(app, KeyCode::Char(c)));
}

/// Run a command in `dir` and return stdout; panic with stderr on failure.
pub fn sh(dir: &Path, program: &str, args: &[&str]) -> String {
    sh_env(dir, program, args, &[])
}

pub fn sh_env(dir: &Path, program: &str, args: &[&str], env: &[(&str, &str)]) -> String {
    let mut cmd = Command::new(program);
    cmd.current_dir(dir).args(args).env("LC_ALL", "C");
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out = cmd.output().expect("spawn");
    assert!(
        out.status.success(),
        "{program} {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

/// Every file under `dir`: relative path -> (content hash, size, mtime).
pub type Snapshot = BTreeMap<PathBuf, (u64, u64, SystemTime)>;

pub fn snapshot(dir: &Path) -> Snapshot {
    let mut out = BTreeMap::new();
    walk(dir, dir, &mut out);
    out
}

fn walk(base: &Path, dir: &Path, out: &mut Snapshot) {
    for e in std::fs::read_dir(dir).unwrap() {
        let e = e.unwrap();
        let p = e.path();
        let meta = std::fs::symlink_metadata(&p).unwrap();
        if meta.is_dir() {
            walk(base, &p, out);
            out.insert(
                p.strip_prefix(base).unwrap().to_path_buf(),
                (0, 0, meta.modified().unwrap()),
            );
        } else {
            let mut h = DefaultHasher::new();
            h.write(&std::fs::read(&p).unwrap_or_default());
            out.insert(
                p.strip_prefix(base).unwrap().to_path_buf(),
                (h.finish(), meta.len(), meta.modified().unwrap()),
            );
        }
    }
}

/// [`snapshot`] of a `.git` dir, without the mtimes of split-index
/// `sharedindex.*` files: git itself refreshes those on any read.
/// Their contents still count.
pub fn git_dir_snapshot(dir: &Path) -> Snapshot {
    let mut s = snapshot(dir);
    for (p, v) in s.iter_mut() {
        let shared = p
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with("sharedindex."));
        if shared {
            v.2 = SystemTime::UNIX_EPOCH;
        }
    }
    s
}

pub fn assert_same(before: &Snapshot, after: &Snapshot) {
    if before != after {
        let changed: Vec<_> = before
            .keys()
            .chain(after.keys())
            .filter(|k| before.get(*k) != after.get(*k))
            .collect();
        panic!("repo metadata changed: {changed:?}");
    }
}

/// Config every test git gets: a fixed identity, and no automatic
/// maintenance. From git 2.55 a commit starts `git maintenance run --auto
/// --detach`, whose child keeps `objects/maintenance.lock` after the
/// commit returns, so a fixture would still be changing while a test
/// snapshots it.
pub const TEST_GIT: [&str; 12] = [
    "-c",
    "user.name=t",
    "-c",
    "user.email=t@t",
    "-c",
    "maintenance.auto=false",
    "-c",
    "maintenance.autoDetach=false",
    "-c",
    "gc.auto=0",
    "-c",
    "gc.autoDetach=false",
];

/// The git the tests run: `$DEBRIEF_GIT` when set and non-empty (as
/// `vcs::git_program` resolves it), else `git`.
pub fn git_program() -> String {
    std::env::var("DEBRIEF_GIT")
        .ok()
        .filter(|g| !g.is_empty())
        .unwrap_or_else(|| "git".into())
}

/// git with [`TEST_GIT`].
pub fn git(dir: &Path, args: &[&str]) -> String {
    let mut a = TEST_GIT.to_vec();
    a.extend_from_slice(args);
    sh(dir, &git_program(), &a)
}

/// One file change in a [`git_repo`] commit.
pub enum Change {
    Write(String, Vec<u8>),
    Delete(String),
}

/// Write `path` (relative, `/`-separated) with `data`.
pub fn w(path: &str, data: impl AsRef<[u8]>) -> Change {
    Change::Write(path.into(), data.as_ref().to_vec())
}

/// Delete `path`.
pub fn rm(path: &str) -> Change {
    Change::Delete(path.into())
}

/// A git repo at `root` (created if missing) on `main`, holding
/// `commits` (message, changes) oldest first, checked out with a clean
/// tree. Three git processes: `init`, one `fast-import`, `read-tree`.
///
/// Committer and author are `t <t@t>`; commit dates rise by 1s and end
/// 1s before now, so commits a test makes later still sort after.
/// Messages get the trailing newline `git commit -m` adds.
pub fn git_repo(root: &Path, commits: &[(&str, Vec<Change>)]) {
    std::fs::create_dir_all(root).unwrap();
    git(root, &["init", "-q", "-b", "main"]);
    let stream = import_stream(&chain(commits), "refs/heads/main", &[]);
    fast_import(root, &stream, &[]);
    // Unlike `reset --hard`, writes no ORIG_HEAD or reflog entry.
    git(root, &["read-tree", "-u", "--reset", "HEAD"]);
}

/// One commit of a fast-import stream: message, parents (commit numbers
/// from 1; none: the previous commit on the ref), changes.
type Node<'a> = (&'a str, &'a [usize], &'a [Change]);

/// `commits` as [`Node`]s on one line.
fn chain<'a>(commits: &'a [(&'a str, Vec<Change>)]) -> Vec<Node<'a>> {
    commits
        .iter()
        .map(|(m, c)| (*m, &[][..], c.as_slice()))
        .collect()
}

/// A fast-import stream of `commits` (as in [`git_repo`]) on `tip`,
/// marked `:1` up, then each of `refs` (ref, commit number from 1). A
/// commit naming parents starts from the first and merges the rest.
fn import_stream(commits: &[Node], tip: &str, refs: &[(String, usize)]) -> Vec<u8> {
    use std::io::Write;
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let base = now - commits.len() as u64;
    let mut s = Vec::new();
    for (i, (msg, parents, changes)) in commits.iter().enumerate() {
        let mut msg = msg.to_string();
        if !msg.ends_with('\n') {
            msg.push('\n');
        }
        write!(
            s,
            "commit {tip}\nmark :{}\ncommitter t <t@t> {} +0000\ndata {}\n{msg}",
            i + 1,
            base + i as u64,
            msg.len()
        )
        .unwrap();
        if let Some((first, rest)) = parents.split_first() {
            writeln!(s, "from :{first}").unwrap();
            for m in rest {
                writeln!(s, "merge :{m}").unwrap();
            }
        }
        for c in changes.iter() {
            match c {
                Change::Write(p, data) => {
                    writeln!(s, "M 100644 inline {}\ndata {}", quote(p), data.len()).unwrap();
                    s.extend_from_slice(data);
                    s.push(b'\n');
                }
                Change::Delete(p) => writeln!(s, "D {}", quote(p)).unwrap(),
            }
        }
        s.push(b'\n');
    }
    for (r, n) in refs {
        write!(s, "reset {r}\nfrom :{n}\n\n").unwrap();
    }
    s
}

/// [`bubbles_repo`]'s first-parent spine, newest first. The octopus is
/// the tip, so a fresh log's first row is a merge.
pub const BUBBLES_SPINE: &[&str] = &[
    "Octopus x y z",
    "Merge n",
    "m1",
    "Merge b",
    "Merge a",
    "base",
];

/// [`bubbles_repo`]'s merges (by summary) and their fold groups, one per
/// non-first parent in parent order, each newest first. `Merge s` is the
/// merge nested in `Merge n`'s bubble.
pub const BUBBLES_GROUPS: &[(&str, &[&[&str]])] = &[
    ("Octopus x y z", &[&["x1"], &["y1"], &["z1"]]),
    ("Merge n", &[&["Merge s", "n2", "n1"]]),
    ("Merge s", &[&["s1"]]),
    ("Merge b", &[&["b2", "b1"]]),
    ("Merge a", &[&["a2", "a1"]]),
];

/// [`bubbles_repo`]'s merges (by summary) and the paths their bubble's
/// commits touch, sorted.
pub const BUBBLES_FILES: &[(&str, &[&str])] = &[
    ("Octopus x y z", &["x.txt", "y.txt", "z.txt"]),
    ("Merge n", &["n/1.txt", "n/2.txt", "n/s1.txt"]),
    ("Merge s", &["n/s1.txt"]),
    ("Merge b", &["b/1.txt", "b/2.txt", "notes.txt"]),
    ("Merge a", &["a/1.txt", "a/2.txt", "notes.txt"]),
];

/// The path two separate bubbles (`Merge a`'s, `Merge b`'s) and one
/// direct spine commit (`m1`) touch.
pub const BUBBLES_SHARED: &str = "notes.txt";

/// A git repo at `root` (created if missing) on `main`, checked out
/// clean: `base`; bubbles `a1 a2` and `b1 b2` merged in turn (`Merge a`,
/// `Merge b`); `m1` on the spine; a nested bubble `n1 n2` that merged
/// its own sub-branch `s1` (`Merge s`) before landing (`Merge n`); then
/// an octopus of `x1`, `y1`, `z1` at the tip. Summaries are unique; IDs
/// vary (dates follow the clock). See [`BUBBLES_SPINE`],
/// [`BUBBLES_GROUPS`], [`BUBBLES_FILES`].
pub fn bubbles_repo(root: &Path) {
    let c = |p: &str, d: &str| vec![w(p, d)];
    let n = BUBBLES_SHARED;
    // A merge commit writes its bubble's files itself: fast-import starts
    // its tree from the first parent's.
    let commits: Vec<(&str, Vec<usize>, Vec<Change>)> = vec![
        ("base", vec![], c("base.txt", "base\n")), // 1
        ("a1", vec![1], c("a/1.txt", "a1\n")),     // 2
        ("a2", vec![2], vec![w("a/2.txt", "a2\n"), w(n, "a\n")]), // 3
        (
            "Merge a",
            vec![1, 3],
            vec![w("a/1.txt", "a1\n"), w("a/2.txt", "a2\n"), w(n, "a\n")],
        ), // 4
        ("b1", vec![4], vec![w("b/1.txt", "b1\n"), w(n, "a\nb\n")]), // 5
        ("b2", vec![5], c("b/2.txt", "b2\n")),     // 6
        (
            "Merge b",
            vec![4, 6],
            vec![w("b/1.txt", "b1\n"), w("b/2.txt", "b2\n"), w(n, "a\nb\n")],
        ), // 7
        ("m1", vec![7], c(n, "a\nb\nm\n")),        // 8
        ("n1", vec![8], c("n/1.txt", "n1\n")),     // 9
        ("s1", vec![9], c("n/s1.txt", "s1\n")),    // 10
        ("n2", vec![9], c("n/2.txt", "n2\n")),     // 11
        ("Merge s", vec![11, 10], c("n/s1.txt", "s1\n")), // 12
        (
            "Merge n",
            vec![8, 12],
            vec![
                w("n/1.txt", "n1\n"),
                w("n/2.txt", "n2\n"),
                w("n/s1.txt", "s1\n"),
            ],
        ), // 13
        ("x1", vec![13], c("x.txt", "x\n")),       // 14
        ("y1", vec![13], c("y.txt", "y\n")),       // 15
        ("z1", vec![13], c("z.txt", "z\n")),       // 16
        (
            "Octopus x y z",
            vec![13, 14, 15, 16],
            vec![w("x.txt", "x\n"), w("y.txt", "y\n"), w("z.txt", "z\n")],
        ), // 17
    ];
    graph_repo(root, &commits);
}

/// A git repo at `root` (created if missing) on `main`, holding
/// `commits` (message, parents as commit numbers from 1, changes) oldest
/// first, checked out clean. No parents: the previous commit. A merge's
/// tree starts from its first parent's, so it writes its bubble's files
/// itself. Three git processes, as [`git_repo`].
pub fn graph_repo(root: &Path, commits: &[(impl AsRef<str>, Vec<usize>, Vec<Change>)]) {
    let nodes: Vec<Node> = commits
        .iter()
        .map(|(m, p, c)| (m.as_ref(), p.as_slice(), c.as_slice()))
        .collect();
    std::fs::create_dir_all(root).unwrap();
    git(root, &["init", "-q", "-b", "main"]);
    fast_import(root, &import_stream(&nodes, "refs/heads/main", &[]), &[]);
    git(root, &["read-tree", "-u", "--reset", "HEAD"]);
}

/// Feed `stream` to `git fast-import` in `dir`.
fn fast_import(dir: &Path, stream: &[u8], args: &[&str]) {
    use std::io::Write;
    let mut cmd = Command::new(git_program());
    cmd.current_dir(dir)
        .args(TEST_GIT)
        .args(["fast-import", "--quiet"])
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = cmd.spawn().expect("spawn git fast-import");
    child.stdin.take().unwrap().write_all(stream).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "git fast-import failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// `p` as a fast-import C-style quoted path.
fn quote(p: &str) -> String {
    let mut q = String::from("\"");
    for ch in p.chars() {
        match ch {
            '"' => q.push_str("\\\""),
            '\\' => q.push_str("\\\\"),
            '\n' => q.push_str("\\n"),
            c => q.push(c),
        }
    }
    q.push('"');
    q
}

/// `git add -A` and commit.
pub fn commit(dir: &Path, msg: &str) {
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", msg]);
}

/// jj is on PATH; prints a skip note when it is not.
pub fn have_jj() -> bool {
    let ok = Command::new("jj")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !ok {
        eprintln!("skipping: jj is not installed");
    }
    ok
}

/// A throwaway jj repo with its own user config.
pub struct JjEnv {
    pub _tmp: tempfile::TempDir,
    pub cfg: String,
    pub root: PathBuf,
}

impl JjEnv {
    /// An empty repo; colocated (`.git` at the root) or not.
    pub fn new(colocate: bool) -> Self {
        let env = Self::unborn();
        let colocation = if colocate {
            "--colocate"
        } else {
            "--no-colocate"
        };
        env.jj(&["--quiet", "git", "init", colocation]);
        env.finish()
    }

    /// A non-colocated repo holding `commits` (as in [`git_repo`]),
    /// `bookmarks` (name, commit number from 1) and an empty `@` on
    /// commit number `at`. Built with git (a bare repo beside the root,
    /// so no `.git` at it) and one `jj git init --git-repo`: three
    /// processes, not one jj command per commit.
    pub fn with_history(
        commits: &[(&str, Vec<Change>)],
        bookmarks: &[(&str, usize)],
        at: usize,
    ) -> Self {
        let env = Self::unborn();
        let dir = env._tmp.path();
        let git_dir = dir.join("git");
        git(dir, &["init", "-q", "--bare", "git"]);
        // Commits go on a scratch ref, deleted at the end, so jj imports
        // only `bookmarks`; `@` comes from a detached HEAD.
        let tip = "refs/fixture/tip";
        let refs: Vec<_> = bookmarks
            .iter()
            .map(|(b, n)| (format!("refs/heads/{b}"), *n))
            .collect();
        let mut stream = import_stream(&chain(commits), tip, &refs);
        stream.extend_from_slice(format!("reset {tip}\n\n").as_bytes());
        let marks = dir.join("marks");
        let marks_arg = format!("--export-marks={}", marks.display());
        fast_import(&git_dir, &stream, &[&marks_arg]);
        let marks = std::fs::read_to_string(marks).unwrap();
        let head = marks
            .lines()
            .find_map(|l| l.strip_prefix(&format!(":{at} ")))
            .unwrap();
        std::fs::write(git_dir.join("HEAD"), format!("{head}\n")).unwrap();
        env.jj(&[
            "--quiet",
            "git",
            "init",
            "--git-repo",
            git_dir.to_str().unwrap(),
        ]);
        env.finish()
    }

    /// A non-colocated repo over the git repo `build` makes in the
    /// directory it is given (beside the root), with jj's new empty `@`
    /// on the git `HEAD`: one `jj git init --git-repo`.
    pub fn over_git(build: impl FnOnce(&Path)) -> Self {
        let env = Self::unborn();
        let git_dir = env._tmp.path().join("git");
        build(&git_dir);
        env.jj(&[
            "--quiet",
            "git",
            "init",
            "--git-repo",
            git_dir.to_str().unwrap(),
        ]);
        env.finish()
    }

    /// The config and an empty root, before `jj git init`.
    fn unborn() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = tmp.path().join("jj.toml");
        std::fs::write(
            &cfg,
            "[user]\nname = \"t\"\nemail = \"t@t\"\n[ui]\nconflict-marker-style = \"diff\"\n",
        )
        .unwrap();
        let root = tmp.path().join("repo");
        std::fs::create_dir(&root).unwrap();
        JjEnv {
            cfg: cfg.to_str().unwrap().to_string(),
            root,
            _tmp: tmp,
        }
    }

    fn finish(self) -> Self {
        let root = self.root.canonicalize().unwrap();
        JjEnv { root, ..self }
    }

    /// Run jj in the repo with the test config (ordinary, writing jj).
    pub fn jj(&self, args: &[&str]) -> String {
        sh_env(&self.root, "jj", args, &[("JJ_CONFIG", &self.cfg)])
    }

    pub fn write(&self, path: &str, text: &str) {
        std::fs::write(self.root.join(path), text).unwrap();
    }

    pub fn op_count(&self) -> usize {
        self.jj(&[
            "--ignore-working-copy",
            "--at-op=@",
            "op",
            "log",
            "--no-graph",
            "-T",
            "id ++ \"\\n\"",
        ])
        .lines()
        .count()
    }

    pub fn op_heads(&self) -> Vec<String> {
        let mut v: Vec<_> = std::fs::read_dir(self.root.join(".jj/repo/op_heads/heads"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }
}

/// git: `main`: first, second; `feat/x` off second: f1, f2; checked out
/// on `main`.
pub fn git_branch_repo() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir(&root).unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    std::fs::write(root.join("a.txt"), "1\n").unwrap();
    commit(&root, "first");
    std::fs::write(root.join("a.txt"), "2\n").unwrap();
    commit(&root, "second");
    git(&root, &["checkout", "-q", "-b", "feat/x"]);
    std::fs::write(root.join("f.txt"), "1\n").unwrap();
    commit(&root, "f1");
    std::fs::write(root.join("f.txt"), "2\n").unwrap();
    commit(&root, "f2");
    git(&root, &["checkout", "-q", "main"]);
    let root = root.canonicalize().unwrap();
    (tmp, root)
}

/// The summaries `revset` lists, newest first.
pub fn summaries_of(v: &dyn vcs::Vcs, revset: &str) -> Vec<String> {
    v.log(Some(revset), 200)
        .unwrap()
        .into_iter()
        .map(|c| c.summary)
        .collect()
}

/// Summaries, in order.
pub fn summaries(log: &[vcs::CommitInfo]) -> Vec<&str> {
    log.iter().map(|c| c.summary.as_str()).collect()
}

/// [`bubbles_repo`] as `v` reads it: every commit, by summary.
pub fn bubbles_by_summary(v: &dyn vcs::Vcs, revset: &str) -> BTreeMap<String, vcs::CommitInfo> {
    v.log(Some(revset), 200)
        .unwrap()
        .into_iter()
        .map(|c| (c.summary.clone(), c))
        .collect()
}

/// `v`'s spine of `revset` is [`BUBBLES_SPINE`] (after `extra`, rows a
/// backend adds on top), and listing it with a limit cuts it.
pub fn check_bubbles_spine(v: &dyn vcs::Vcs, revset: Option<&str>, extra: &[&str]) {
    let want: Vec<&str> = extra.iter().chain(BUBBLES_SPINE).copied().collect();
    let log = v.log_in(revset, None, 200, true).unwrap();
    assert_eq!(summaries(&log), want);
    let two = v.log_in(revset, None, 2, true).unwrap();
    assert_eq!(summaries(&two), want[..2]);
    // Off, the side commits are listed too.
    let flat = v.log_in(revset, None, 200, false).unwrap();
    assert_eq!(flat.len(), want.len() + 11, "{:?}", summaries(&flat));
}

/// Every merge's fold groups are [`BUBBLES_GROUPS`]: one per non-first
/// parent, a nested merge a row of its own, not flattened.
pub fn check_bubbles_groups(v: &dyn vcs::Vcs, all: &BTreeMap<String, vcs::CommitInfo>) {
    for (merge, groups) in BUBBLES_GROUPS {
        let m = &all[*merge];
        assert_eq!(m.parents.len(), groups.len() + 1, "{merge}");
        for (parent, want) in m.parents[1..].iter().zip(groups.iter()) {
            let got = v.fold(&m.commit, parent, None, 200).unwrap();
            assert_eq!(summaries(&got), *want, "{merge}");
        }
    }
    let n = &all["Merge n"];
    let one = v.fold(&n.commit, &n.parents[1], None, 1).unwrap();
    assert_eq!(summaries(&one), ["Merge s"]);
}

/// Every merge's size and bubble: the commits it brought in, nested
/// included, the merge excluded; the bubble touches [`BUBBLES_FILES`].
pub fn check_bubbles_sizes(v: &dyn vcs::Vcs, all: &BTreeMap<String, vcs::CommitInfo>) {
    let sizes = [
        ("Octopus x y z", 3),
        ("Merge n", 4),
        ("Merge s", 1),
        ("Merge b", 2),
        ("Merge a", 2),
    ];
    for (merge, size) in sizes {
        let id = &all[merge].commit;
        assert_eq!(v.merge_size(id).unwrap(), size, "{merge}");
        let bubble = v.bubble(id, 200).unwrap();
        assert_eq!(bubble.len(), size, "{merge}: {:?}", summaries(&bubble));
        assert!(bubble.iter().all(|c| &c.commit != id), "{merge}");
        let mut got = summaries(&bubble);
        got.sort();
        let mut want = flatten(merge);
        want.sort();
        assert_eq!(got, want, "{merge}");
    }
    let n = &all["Merge n"].commit;
    assert_eq!(v.bubble(n, 2).unwrap().len(), 2);
    // A merge's file list is what its bubble's own diffs touch.
    for (merge, files) in BUBBLES_FILES {
        let id = &all[*merge].commit;
        let mut got: Vec<String> = v
            .bubble(id, 200)
            .unwrap()
            .iter()
            .filter(|c| c.parents.len() == 1)
            .flat_map(|c| v.stat(&c.commit).unwrap().files)
            .map(|f| f.path)
            .collect();
        got.sort();
        got.dedup();
        assert_eq!(got, *files, "{merge}");
    }
}

/// The summaries `merge`'s bubble holds, from [`BUBBLES_GROUPS`].
fn flatten(merge: &str) -> Vec<&'static str> {
    let groups = BUBBLES_GROUPS.iter().find(|(m, _)| *m == merge).unwrap().1;
    let mut out = Vec::new();
    for s in groups.iter().flat_map(|g| g.iter()) {
        out.push(*s);
        if BUBBLES_GROUPS.iter().any(|(m, _)| m == s) {
            out.extend(flatten(s));
        }
    }
    out
}

/// Spine + path lists a spine merge whose bubble touched the path, as
/// git's `--first-parent -- <path>` does, and a fold narrows the same.
pub fn check_bubbles_path(
    v: &dyn vcs::Vcs,
    revset: Option<&str>,
    all: &BTreeMap<String, vcs::CommitInfo>,
) {
    let shared = v.log_in(revset, Some(BUBBLES_SHARED), 200, true).unwrap();
    assert_eq!(summaries(&shared), ["m1", "Merge b", "Merge a"]);
    let page = v.log_in(revset, Some(BUBBLES_SHARED), 2, true).unwrap();
    assert_eq!(summaries(&page), ["m1", "Merge b"]);
    // Through a nested bubble, and a directory.
    for (path, want) in [
        ("n/s1.txt", &["Merge n"][..]),
        ("n", &["Merge n"]),
        ("y.txt", &["Octopus x y z"]),
    ] {
        let log = v.log_in(revset, Some(path), 200, true).unwrap();
        assert_eq!(summaries(&log), want, "{path}");
    }
    let a = &all["Merge a"];
    let fold = v
        .fold(&a.commit, &a.parents[1], Some(BUBBLES_SHARED), 200)
        .unwrap();
    assert_eq!(summaries(&fold), ["a2"]);
    let n = &all["Merge n"];
    let fold = v
        .fold(&n.commit, &n.parents[1], Some("n/s1.txt"), 200)
        .unwrap();
    assert_eq!(summaries(&fold), ["Merge s"]);
}
