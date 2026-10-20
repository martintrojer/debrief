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

/// git with [`TEST_GIT`].
pub fn git(dir: &Path, args: &[&str]) -> String {
    let mut a = TEST_GIT.to_vec();
    a.extend_from_slice(args);
    sh(dir, "git", &a)
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
    pub fn new(colocate: bool) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = tmp.path().join("jj.toml");
        std::fs::write(
            &cfg,
            "[user]\nname = \"t\"\nemail = \"t@t\"\n[ui]\nconflict-marker-style = \"diff\"\n",
        )
        .unwrap();
        let root = tmp.path().join("repo");
        std::fs::create_dir(&root).unwrap();
        let env = JjEnv {
            cfg: cfg.to_str().unwrap().to_string(),
            root,
            _tmp: tmp,
        };
        let mut args = vec!["--quiet", "git", "init"];
        if colocate {
            args.push("--colocate");
        }
        env.jj(&args);
        let root = env.root.canonicalize().unwrap();
        JjEnv { root, ..env }
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
