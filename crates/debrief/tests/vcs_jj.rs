mod common;

use std::path::PathBuf;
use std::process::Command;

use common::{assert_same, sh_env, snapshot};
use debrief::vcs::{self, Kind, LineKind};

fn have_jj() -> bool {
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

struct Env {
    _tmp: tempfile::TempDir,
    cfg: String,
    root: PathBuf,
}

impl Env {
    fn new(colocate: bool) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = tmp.path().join("jj.toml");
        std::fs::write(
            &cfg,
            "[user]\nname = \"t\"\nemail = \"t@t\"\n[ui]\nconflict-marker-style = \"diff\"\n",
        )
        .unwrap();
        let root = tmp.path().join("repo");
        std::fs::create_dir(&root).unwrap();
        let env = Env {
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
        Env { root, ..env }
    }

    /// Run jj in the repo with the test config (ordinary, writing jj).
    fn jj(&self, args: &[&str]) -> String {
        sh_env(&self.root, "jj", args, &[("JJ_CONFIG", &self.cfg)])
    }

    fn write(&self, path: &str, text: &str) {
        std::fs::write(self.root.join(path), text).unwrap();
    }

    fn op_count(&self) -> usize {
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

    fn op_heads(&self) -> Vec<String> {
        let mut v: Vec<_> = std::fs::read_dir(self.root.join(".jj/repo/op_heads/heads"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }
}

fn sample() -> Env {
    let e = Env::new(false);
    e.write("a.txt", "one\ntwo\n");
    e.jj(&["--quiet", "commit", "-m", "first\n\nbody"]);
    e.write("a.txt", "one\nTWO\n");
    e.write("b.txt", "b\n");
    e.jj(&["--quiet", "commit", "-m", "second"]);
    e
}

#[test]
fn log_has_change_ids_and_parents() {
    if !have_jj() {
        return;
    }
    let e = sample();
    let v = vcs::open(&e.root.join("a.txt")).unwrap();
    assert_eq!(v.kind(), Kind::Jj);
    assert_eq!(v.root(), e.root);
    let log = v.log(Some("::@ ~ root()"), 200).unwrap();
    let summaries: Vec<_> = log.iter().map(|c| c.summary.as_str()).collect();
    assert_eq!(summaries, ["", "second", "first"]);
    for c in &log {
        assert_eq!(c.commit.len(), 40);
        let change = c.change.as_deref().unwrap();
        assert_eq!(change.len(), 32);
        assert!(change.bytes().all(|b| (b'k'..=b'z').contains(&b)));
        assert!(c.time > 1_600_000_000);
        assert_eq!(c.parents.len(), 1);
    }
    assert_eq!(log[0].parents[0], log[1].commit);
    assert_eq!(log[1].parents[0], log[2].commit);
    assert_eq!(v.log(Some("::@ ~ root()"), 1).unwrap().len(), 1);
    // Default revset (the user's revsets.log) works too.
    assert!(!v.log(None, 200).unwrap().is_empty());
}

#[test]
fn show_parses_message_and_diff() {
    if !have_jj() {
        return;
    }
    let e = sample();
    let v = vcs::open(&e.root).unwrap();
    let log = v.log(Some("::@ ~ root()"), 200).unwrap();
    let second = v.show(&log[1].commit).unwrap();
    assert_eq!(second.message, "second");
    let names: Vec<_> = second
        .files
        .iter()
        .map(|f| (f.old_path.as_deref(), f.new_path.as_deref()))
        .collect();
    assert_eq!(
        names,
        [(Some("a.txt"), Some("a.txt")), (None, Some("b.txt"))]
    );
    let l = &second.files[0].hunks[0].lines;
    assert!(
        l.iter()
            .any(|l| l.kind == LineKind::Added && l.text == "TWO")
    );
    let first = v.show(&log[2].commit).unwrap();
    assert_eq!(first.message, "first\n\nbody");
    // The empty working-copy commit has no files.
    assert!(v.show(&log[0].commit).unwrap().files.is_empty());
}

#[test]
fn colocated_repo_opens_as_jj() {
    if !have_jj() {
        return;
    }
    let e = Env::new(true);
    assert!(e.root.join(".git").exists());
    e.write("a.txt", "a\n");
    e.jj(&["--quiet", "commit", "-m", "one"]);
    let v = vcs::open(&e.root).unwrap();
    assert_eq!(v.kind(), Kind::Jj);
    assert_eq!(v.root(), e.root);
    let log = v.log(Some("::@ ~ root()"), 10).unwrap();
    assert_eq!(log[1].summary, "one");
}

#[test]
fn log_and_show_do_not_write() {
    if !have_jj() {
        return;
    }
    let e = sample();
    // Leave a working-copy edit jj has not snapshotted yet.
    e.write("dirty.txt", "dirty\n");
    let ops = e.op_count();
    let before = snapshot(&e.root.join(".jj"));
    let v = vcs::open(&e.root).unwrap();
    for c in v.log(None, 200).unwrap() {
        v.show(&c.commit).unwrap();
    }
    v.log(Some("::@"), 200).unwrap();
    assert_same(&before, &snapshot(&e.root.join(".jj")));
    assert_eq!(e.op_count(), ops);
}

#[test]
fn divergent_op_heads_are_left_alone() {
    if !have_jj() {
        return;
    }
    let e = sample();
    let op = e
        .jj(&[
            "--ignore-working-copy",
            "op",
            "log",
            "--no-graph",
            "-n1",
            "-T",
            "id",
        ])
        .trim()
        .to_string();
    let at = format!("--at-op={op}");
    for m in ["x1", "x2"] {
        e.jj(&[
            "--ignore-working-copy",
            &at,
            "--quiet",
            "describe",
            "-r",
            "@-",
            "-m",
            m,
        ]);
    }
    let heads = e.op_heads();
    assert_eq!(heads.len(), 2, "expected divergent op heads");
    let before = snapshot(&e.root.join(".jj"));
    let v = vcs::open(&e.root).unwrap();
    // We refuse rather than let jj merge (or lock) the op heads.
    let err = format!("{:#}", v.log(None, 10).err().unwrap());
    assert!(err.contains("divergent jj operation heads"), "{err}");
    assert!(v.show("0000000000000000000000000000000000000000").is_err());
    assert_eq!(e.op_heads(), heads);
    assert_same(&before, &snapshot(&e.root.join(".jj")));
}

#[test]
fn secondary_workspace_reads_the_shared_repo() {
    if !have_jj() {
        return;
    }
    let e = sample();
    let ws = e.root.parent().unwrap().join("ws2");
    e.jj(&["--quiet", "workspace", "add", ws.to_str().unwrap()]);
    let v = vcs::open(&ws).unwrap();
    assert_eq!(v.kind(), Kind::Jj);
    assert_eq!(v.root(), ws.canonicalize().unwrap());
    let log = v.log(Some("all() ~ root()"), 200).unwrap();
    assert!(log.iter().any(|c| c.summary == "second"));
}

#[test]
fn option_like_revsets_are_rejected() {
    if !have_jj() {
        return;
    }
    let e = sample();
    let v = vcs::open(&e.root).unwrap();
    let out = e.root.parent().unwrap().join("x");
    let flag = format!("--output={}", out.display());
    assert!(v.log(Some(&flag), 10).is_err());
    assert!(v.show(&flag).is_err());
    assert!(v.show("@-").is_err(), "show takes IDs, not revsets");
    assert!(!out.exists());
}

#[test]
fn failed_command_carries_stderr() {
    if !have_jj() {
        return;
    }
    let e = sample();
    let v = vcs::open(&e.root).unwrap();
    let err = format!("{:#}", v.log(Some("nosuchbookmark"), 10).err().unwrap());
    assert!(err.contains("nosuchbookmark"), "{err}");
}
