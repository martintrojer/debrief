mod common;

use std::path::{Path, PathBuf};

use common::{assert_same, sh, snapshot};
use debrief::vcs::{self, Kind, LineKind};

fn git(dir: &Path, args: &[&str]) -> String {
    let mut a = vec!["-c", "user.name=t", "-c", "user.email=t@t"];
    a.extend_from_slice(args);
    sh(dir, "git", &a)
}

fn commit(dir: &Path, msg: &str) {
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", msg]);
}

/// A repo with three commits: add, modify + add, rename with edit.
fn repo() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir(&root).unwrap();
    git(&root, &["init", "-q"]);
    std::fs::write(root.join("a.txt"), "one\ntwo\nthree\n").unwrap();
    std::fs::create_dir(root.join("sub")).unwrap();
    std::fs::write(root.join("sub/keep.txt"), "keep\n").unwrap();
    commit(&root, "first\n\nbody line");
    std::fs::write(root.join("a.txt"), "one\nTWO\nthree\n").unwrap();
    std::fs::write(root.join("b.txt"), "b1\nb2\nb3\nb4\nb5\n").unwrap();
    commit(&root, "second");
    std::fs::remove_file(root.join("b.txt")).unwrap();
    std::fs::write(root.join("c d.txt"), "b1\nb2\nb3\nb4\nB5\n").unwrap();
    commit(&root, "third: rename");
    let root = root.canonicalize().unwrap();
    (tmp, root)
}

fn git_snapshot(root: &Path) -> (common::Snapshot, String) {
    (
        snapshot(&root.join(".git")),
        sh(
            root,
            "git",
            &["--no-optional-locks", "status", "--porcelain"],
        ),
    )
}

#[test]
fn log_lists_newest_first_with_fields() {
    let (_t, root) = repo();
    let v = vcs::open(&root).unwrap();
    assert_eq!(v.kind(), Kind::Git);
    assert_eq!(v.root(), root);
    let log = v.log(None, 200).unwrap();
    let summaries: Vec<_> = log.iter().map(|c| c.summary.as_str()).collect();
    assert_eq!(summaries, ["third: rename", "second", "first"]);
    for c in &log {
        assert_eq!(c.commit.len(), 40);
        assert_eq!(c.change, None);
        assert_eq!(c.author, "t");
        assert!(c.time > 1_600_000_000);
    }
    assert_eq!(log[0].parents, vec![log[1].commit.clone()]);
    assert_eq!(log[1].parents, vec![log[2].commit.clone()]);
    assert!(log[2].parents.is_empty());
    assert_eq!(v.log(None, 2).unwrap().len(), 2);
    let ranged = v.log(Some("HEAD~2..HEAD"), 200).unwrap();
    assert_eq!(ranged.len(), 2);
}

#[test]
fn show_parses_files() {
    let (_t, root) = repo();
    let v = vcs::open(&root).unwrap();
    let log = v.log(None, 10).unwrap();

    let third = v.show(&log[0].commit).unwrap();
    assert_eq!(third.message, "third: rename");
    assert_eq!(third.files.len(), 1);
    let f = &third.files[0];
    assert_eq!(f.old_path.as_deref(), Some("b.txt"));
    assert_eq!(f.new_path.as_deref(), Some("c d.txt"));
    let l = &f.hunks[0].lines;
    assert!(
        l.iter()
            .any(|l| l.kind == LineKind::Added && l.text == "B5")
    );

    let second = v.show(&log[1].commit).unwrap();
    let names: Vec<_> = second
        .files
        .iter()
        .map(|f| (f.old_path.as_deref(), f.new_path.as_deref()))
        .collect();
    assert_eq!(
        names,
        [(Some("a.txt"), Some("a.txt")), (None, Some("b.txt"))]
    );
    // Short and symbolic revisions resolve too.
    assert_eq!(v.show("HEAD~1").unwrap(), second);
}

#[test]
fn root_commit_shows_added_files() {
    let (_t, root) = repo();
    let v = vcs::open(&root).unwrap();
    let first = v.log(None, 10).unwrap().pop().unwrap();
    let d = v.show(&first.commit).unwrap();
    assert_eq!(d.message, "first\n\nbody line");
    let added: Vec<_> = d.files.iter().map(|f| f.new_path.as_deref()).collect();
    assert_eq!(added, [Some("a.txt"), Some("sub/keep.txt")]);
    assert!(d.files.iter().all(|f| f.old_path.is_none()));
}

#[test]
fn root_commit_shows_files_despite_show_root_false() {
    let (_t, root) = repo();
    git(&root, &["config", "log.showRoot", "false"]);
    let v = vcs::open(&root).unwrap();
    let first = v.log(None, 10).unwrap().pop().unwrap();
    let d = v.show(&first.commit).unwrap();
    let added: Vec<_> = d.files.iter().map(|f| f.new_path.as_deref()).collect();
    assert_eq!(added, [Some("a.txt"), Some("sub/keep.txt")]);
}

#[test]
fn merge_shows_first_parent_diff() {
    let (_t, root) = repo();
    git(&root, &["checkout", "-q", "-b", "side", "HEAD~1"]);
    std::fs::write(root.join("side.txt"), "side\n").unwrap();
    commit(&root, "side");
    git(&root, &["checkout", "-q", "-"]);
    git(&root, &["merge", "-q", "--no-edit", "side"]);
    let v = vcs::open(&root).unwrap();
    let head = &v.log(None, 1).unwrap()[0];
    assert_eq!(head.parents.len(), 2);
    let d = v.show(&head.commit).unwrap();
    let names: Vec<_> = d.files.iter().map(|f| f.new_path.as_deref()).collect();
    assert_eq!(names, [Some("side.txt")]);
}

#[test]
fn open_finds_root_from_subdir_file_symlink_and_relative_path() {
    let (t, root) = repo();
    assert_eq!(vcs::open(&root.join("sub")).unwrap().root(), root);
    assert_eq!(vcs::open(&root.join("sub/keep.txt")).unwrap().root(), root);
    let link = t.path().join("link");
    std::os::unix::fs::symlink(&root, &link).unwrap();
    assert_eq!(vcs::open(&link.join("sub")).unwrap().root(), root);
    // A relative path, resolved against the test process's cwd.
    let cwd = std::env::current_dir().unwrap().canonicalize().unwrap();
    let ups = cwd.components().count() - 1;
    let rel: PathBuf = std::iter::repeat_n("..", ups)
        .collect::<PathBuf>()
        .join(root.strip_prefix("/").unwrap());
    assert_eq!(vcs::open(&rel).unwrap().root(), root);
}

#[test]
fn open_outside_a_repo_errors() {
    let t = tempfile::tempdir().unwrap();
    let err = vcs::open(t.path()).err().unwrap().to_string();
    assert!(err.starts_with("not a git or jj repository: "), "{err}");
}

#[test]
fn log_and_show_do_not_write() {
    let (_t, root) = repo();
    let before = git_snapshot(&root);
    let v = vcs::open(&root).unwrap();
    for c in v.log(None, 200).unwrap() {
        v.show(&c.commit).unwrap();
    }
    v.log(Some("HEAD~1"), 5).unwrap();
    let after = git_snapshot(&root);
    assert_same(&before.0, &after.0);
    assert_eq!(before.1, after.1);
}

#[test]
fn hostile_config_still_parses_and_writes_nothing() {
    let (_t, root) = repo();
    git(&root, &["config", "diff.noprefix", "true"]);
    git(&root, &["config", "diff.mnemonicPrefix", "true"]);
    git(&root, &["config", "core.quotePath", "false"]);
    git(
        &root,
        &[
            "config",
            "diff.up.textconv",
            r#"sh -c "tr a-z A-Z < \"$0\"""#,
        ],
    );
    git(&root, &["config", "diff.up.cachetextconv", "true"]);
    git(&root, &["config", "diff.external", "false"]);
    std::fs::write(root.join(".gitattributes"), "*.txt diff=up\n").unwrap();
    std::fs::write(root.join("é.txt"), "lower\n").unwrap();
    commit(&root, "attrs");

    let before = git_snapshot(&root);
    let v = vcs::open(&root).unwrap();
    let head = &v.log(None, 1).unwrap()[0];
    let d = v.show(&head.commit).unwrap();
    let names: Vec<_> = d.files.iter().map(|f| f.new_path.as_deref()).collect();
    assert_eq!(names, [Some(".gitattributes"), Some("é.txt")]);
    // No textconv: the original text, not upper-cased.
    assert_eq!(d.files[1].hunks[0].lines[0].text, "lower");
    let after = git_snapshot(&root);
    assert_same(&before.0, &after.0);
    let refs = sh(&root, "git", &["for-each-ref", "--format=%(refname)"]);
    assert!(!refs.contains("refs/notes/textconv"), "{refs}");
}

#[test]
fn option_like_revisions_are_rejected() {
    let (t, root) = repo();
    let v = vcs::open(&root).unwrap();
    let out = t.path().join("x");
    let flag = format!("--output={}", out.display());
    assert!(v.log(Some(&flag), 10).is_err());
    assert!(v.show(&flag).is_err());
    assert!(!out.exists());
}

#[test]
fn failed_command_carries_stderr() {
    let (_t, root) = repo();
    let v = vcs::open(&root).unwrap();
    let err = format!("{:#}", v.log(Some("no-such-ref"), 10).err().unwrap());
    assert!(err.contains("no-such-ref"), "{err}");
    assert!(v.show("no-such-ref").is_err());
}

#[test]
fn partial_clone_missing_objects_error() {
    let (t, root) = repo();
    let clone = t.path().join("clone");
    git(&root, &["config", "uploadpack.allowFilter", "true"]);
    let url = format!("file://{}", root.display());
    let ok = std::process::Command::new("git")
        .args(["clone", "-q", "--no-checkout"])
        .args(["--filter=blob:none", &url])
        .arg(&clone)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    let missing = ok
        && sh(
            &clone,
            "git",
            &["rev-list", "--objects", "--missing=print", "--all"],
        )
        .lines()
        .any(|l| l.starts_with('?'));
    if !missing {
        eprintln!("skipping: partial clone from a local path is unavailable");
        return;
    }
    let before = snapshot(&clone.join(".git"));
    let v = vcs::open(&clone).unwrap();
    let log = v.log(None, 10).unwrap();
    assert_eq!(log.len(), 3);
    // Blobs are missing and --no-lazy-fetch forbids fetching them.
    assert!(v.show(&log[0].commit).is_err());
    assert_same(&before, &snapshot(&clone.join(".git")));
}
