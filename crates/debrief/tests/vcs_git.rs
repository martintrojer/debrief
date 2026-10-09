mod common;

use std::path::{Path, PathBuf};

use common::{assert_same, commit, git, git_dir_snapshot, sh, snapshot};
use debrief::vcs::{self, Context, Kind, LineKind};

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
        git_dir_snapshot(&root.join(".git")),
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
    let v = vcs::open(&root, vcs::OpenOptions::default()).unwrap();
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
    let v = vcs::open(&root, vcs::OpenOptions::default()).unwrap();
    let log = v.log(None, 10).unwrap();

    let third = v.show(&log[0].commit, Context::Default).unwrap();
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

    let second = v.show(&log[1].commit, Context::Default).unwrap();
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
    assert_eq!(v.show("HEAD~1", Context::Default).unwrap(), second);
}

#[test]
fn root_commit_shows_added_files() {
    let (_t, root) = repo();
    let v = vcs::open(&root, vcs::OpenOptions::default()).unwrap();
    let first = v.log(None, 10).unwrap().pop().unwrap();
    let d = v.show(&first.commit, Context::Default).unwrap();
    assert_eq!(d.message, "first\n\nbody line");
    let added: Vec<_> = d.files.iter().map(|f| f.new_path.as_deref()).collect();
    assert_eq!(added, [Some("a.txt"), Some("sub/keep.txt")]);
    assert!(d.files.iter().all(|f| f.old_path.is_none()));
}

#[test]
fn root_commit_shows_files_despite_show_root_false() {
    let (_t, root) = repo();
    git(&root, &["config", "log.showRoot", "false"]);
    let v = vcs::open(&root, vcs::OpenOptions::default()).unwrap();
    let first = v.log(None, 10).unwrap().pop().unwrap();
    let d = v.show(&first.commit, Context::Default).unwrap();
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
    let v = vcs::open(&root, vcs::OpenOptions::default()).unwrap();
    let head = &v.log(None, 1).unwrap()[0];
    assert_eq!(head.parents.len(), 2);
    let d = v.show(&head.commit, Context::Default).unwrap();
    let names: Vec<_> = d.files.iter().map(|f| f.new_path.as_deref()).collect();
    assert_eq!(names, [Some("side.txt")]);
}

#[test]
fn open_finds_root_from_subdir_file_symlink_and_relative_path() {
    let (t, root) = repo();
    assert_eq!(
        vcs::open(&root.join("sub"), vcs::OpenOptions::default())
            .unwrap()
            .root(),
        root
    );
    assert_eq!(
        vcs::open(&root.join("sub/keep.txt"), vcs::OpenOptions::default())
            .unwrap()
            .root(),
        root
    );
    let link = t.path().join("link");
    std::os::unix::fs::symlink(&root, &link).unwrap();
    assert_eq!(
        vcs::open(&link.join("sub"), vcs::OpenOptions::default())
            .unwrap()
            .root(),
        root
    );
    // A relative path, resolved against the test process's cwd.
    let cwd = std::env::current_dir().unwrap().canonicalize().unwrap();
    let ups = cwd.components().count() - 1;
    let rel: PathBuf = std::iter::repeat_n("..", ups)
        .collect::<PathBuf>()
        .join(root.strip_prefix("/").unwrap());
    assert_eq!(
        vcs::open(&rel, vcs::OpenOptions::default()).unwrap().root(),
        root
    );
}

#[test]
fn open_keeps_trailing_newline_in_root_name() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("ends\n");
    if std::fs::create_dir(&root).is_err() {
        eprintln!("skipped: filesystem refuses a newline in a directory name");
        return;
    }
    git(&root, &["init", "-q"]);
    std::fs::write(root.join("a.txt"), "a\n").unwrap();
    commit(&root, "first");
    let root = root.canonicalize().unwrap();
    let v = vcs::open(&root, vcs::OpenOptions::default()).unwrap();
    assert_eq!(v.root(), root);
    assert_eq!(v.log(None, 10).unwrap().len(), 1);
}

#[test]
fn open_outside_a_repo_errors() {
    let t = tempfile::tempdir().unwrap();
    let err = vcs::open(t.path(), vcs::OpenOptions::default())
        .err()
        .unwrap()
        .to_string();
    assert!(err.starts_with("not a git or jj repository: "), "{err}");
}

#[test]
fn log_and_show_do_not_write() {
    let (_t, root) = repo();
    let before = git_snapshot(&root);
    let v = vcs::open(&root, vcs::OpenOptions::default()).unwrap();
    for c in v.log(None, 200).unwrap() {
        v.show(&c.commit, Context::Default).unwrap();
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
    let v = vcs::open(&root, vcs::OpenOptions::default()).unwrap();
    let head = &v.log(None, 1).unwrap()[0];
    let d = v.show(&head.commit, Context::Default).unwrap();
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
fn show_and_log_shape_ignore_output_config() {
    let (t, root) = repo();
    let sub = t.path().join("sub");
    std::fs::create_dir(&sub).unwrap();
    git(&sub, &["init", "-q"]);
    git(&sub, &["commit", "-q", "--allow-empty", "-m", "s"]);
    let url = sub.to_str().unwrap();
    git(
        &root,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            "-q",
            url,
            "sm",
        ],
    );
    std::fs::write(root.join("z.txt"), "z\n").unwrap();
    commit(&root, "é sub");
    let order = t.path().join("order");
    std::fs::write(&order, "z.txt\nsm\n").unwrap();
    git(
        &root,
        &["config", "diff.orderFile", order.to_str().unwrap()],
    );
    git(&root, &["config", "diff.submodule", "log"]);
    git(&root, &["config", "diff.ignoreSubmodules", "all"]);
    git(&root, &["config", "i18n.logOutputEncoding", "ISO-8859-1"]);

    let v = vcs::open(&root, vcs::OpenOptions::default()).unwrap();
    let head = &v.log(None, 1).unwrap()[0];
    assert_eq!(head.summary, "é sub");
    let d = v.show(&head.commit, Context::Default).unwrap();
    assert_eq!(d.message, "é sub");
    let names: Vec<_> = d.files.iter().map(|f| f.new_path.as_deref()).collect();
    assert_eq!(names, [Some(".gitmodules"), Some("sm"), Some("z.txt")]);
}

#[test]
fn option_like_revisions_are_rejected() {
    let (t, root) = repo();
    let v = vcs::open(&root, vcs::OpenOptions::default()).unwrap();
    let out = t.path().join("x");
    let flag = format!("--output={}", out.display());
    assert!(v.log(Some(&flag), 10).is_err());
    assert!(v.show(&flag, Context::Default).is_err());
    assert!(!out.exists());
}

#[test]
fn failed_command_carries_stderr() {
    let (_t, root) = repo();
    let v = vcs::open(&root, vcs::OpenOptions::default()).unwrap();
    let err = format!("{:#}", v.log(Some("no-such-ref"), 10).err().unwrap());
    assert!(err.contains("no-such-ref"), "{err}");
    assert!(v.show("no-such-ref", Context::Default).is_err());
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
    let v = vcs::open(&clone, vcs::OpenOptions::default()).unwrap();
    let log = v.log(None, 10).unwrap();
    assert_eq!(log.len(), 3);
    // Blobs are missing and --no-lazy-fetch forbids fetching them.
    assert!(v.show(&log[0].commit, Context::Default).is_err());
    assert_same(&before, &snapshot(&clone.join(".git")));
}

// The working copy (spec §3 "Working copy").

fn open(root: &Path) -> Box<dyn vcs::Vcs> {
    vcs::open(root, vcs::OpenOptions::default()).unwrap()
}

#[test]
fn clean_tree_has_no_working_copy() {
    let (_t, root) = repo();
    assert_eq!(open(&root).working_copy(Context::Default).unwrap(), None);
    // An ignored file doesn't count.
    std::fs::write(root.join(".git/info/exclude"), "*.log\n").unwrap();
    std::fs::write(root.join("x.log"), "x\n").unwrap();
    assert_eq!(open(&root).working_copy(Context::Default).unwrap(), None);
}

#[test]
fn dirty_tracked_file_is_the_working_copy_on_head() {
    let (_t, root) = repo();
    std::fs::write(root.join("a.txt"), "one\nTWO\nthree\nfour\n").unwrap();
    let v = open(&root);
    let w = v.working_copy(Context::Default).unwrap().unwrap();
    assert_eq!(w.files, 1);
    assert_eq!(w.diff.message, "");
    let head = &v.log(None, 1).unwrap()[0];
    assert_eq!(w.base.as_ref(), Some(head));
    let f = &w.diff.files[0];
    assert_eq!(f.new_path.as_deref(), Some("a.txt"));
    let added: Vec<_> = f.hunks[0]
        .lines
        .iter()
        .filter(|l| l.kind == LineKind::Added)
        .map(|l| (l.new, l.text.as_str()))
        .collect();
    assert_eq!(added, [(Some(4), "four")]);
}

#[test]
fn untracked_files_show_as_added() {
    let (_t, root) = repo();
    std::fs::write(root.join("new.txt"), "n1\nn2").unwrap();
    std::fs::write(root.join("empty.txt"), "").unwrap();
    let w = open(&root).working_copy(Context::Default).unwrap().unwrap();
    assert_eq!(w.files, 2);
    let f = w
        .diff
        .files
        .iter()
        .find(|f| f.new_path.as_deref() == Some("new.txt"))
        .unwrap();
    assert_eq!(f.old_path, None);
    let lines: Vec<_> = f.hunks[0]
        .lines
        .iter()
        .map(|l| (l.kind, l.new, l.text.as_str(), l.no_newline))
        .collect();
    assert_eq!(
        lines,
        [
            (LineKind::Added, Some(1), "n1", false),
            (LineKind::Added, Some(2), "n2", true)
        ]
    );
    let e = w
        .diff
        .files
        .iter()
        .find(|f| f.new_path.as_deref() == Some("empty.txt"))
        .unwrap();
    assert!(e.hunks.is_empty() && !e.binary && e.note.is_none());
}

#[test]
fn binary_large_symlink_and_special_untracked_files() {
    let (_t, root) = repo();
    let mut bin = b"text".to_vec();
    bin.push(0);
    std::fs::write(root.join("bin.dat"), &bin).unwrap();
    let big = "x\n".repeat(600 * 1024);
    std::fs::write(root.join("big.txt"), big).unwrap();
    std::os::unix::fs::symlink("/etc/passwd", root.join("link")).unwrap();
    let fifo = root.join("fifo");
    sh(&root, "mkfifo", &[fifo.to_str().unwrap()]);
    // git lists no fifos as untracked; check the reader on its own.
    assert_eq!(
        vcs::untracked_file(&root, "fifo").note.as_deref(),
        Some(vcs::SPECIAL_FILE)
    );
    let w = open(&root).working_copy(Context::Default).unwrap().unwrap();
    let by = |p: &str| {
        w.diff
            .files
            .iter()
            .find(|f| f.new_path.as_deref() == Some(p))
            .unwrap_or_else(|| panic!("{p} missing: {:?}", w.diff.files))
    };
    assert!(by("bin.dat").binary);
    assert!(by("bin.dat").hunks.is_empty());
    assert_eq!(by("big.txt").note.as_deref(), Some(vcs::TOO_LARGE));
    assert!(by("big.txt").hunks.is_empty());
    let link = &by("link").hunks[0].lines;
    assert_eq!(link.len(), 1);
    assert_eq!(link[0].text, "/etc/passwd", "the target, not followed");
}

#[test]
fn deleted_and_renamed_tracked_files() {
    let (_t, root) = repo();
    std::fs::remove_file(root.join("sub/keep.txt")).unwrap();
    std::fs::rename(root.join("a.txt"), root.join("z.txt")).unwrap();
    let w = open(&root).working_copy(Context::Default).unwrap().unwrap();
    let names: Vec<_> = w
        .diff
        .files
        .iter()
        .map(|f| (f.old_path.as_deref(), f.new_path.as_deref()))
        .collect();
    // Without the index, the rename is a delete plus an untracked add.
    assert!(names.contains(&(Some("sub/keep.txt"), None)), "{names:?}");
    assert!(names.contains(&(Some("a.txt"), None)), "{names:?}");
    assert!(names.contains(&(None, Some("z.txt"))), "{names:?}");
}

fn unborn(format: &str) -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir(&root).unwrap();
    git(&root, &["init", "-q", &format!("--object-format={format}")]);
    let root = root.canonicalize().unwrap();
    (tmp, root)
}

#[test]
fn unborn_head_diffs_against_the_empty_tree() {
    for format in ["sha1", "sha256"] {
        let (_t, root) = unborn(format);
        let v = open(&root);
        assert!(v.log(None, 10).unwrap().is_empty(), "{format}");
        assert!(v.log(Some("HEAD"), 10).is_err(), "a named revision errors");
        assert_eq!(v.working_copy(Context::Default).unwrap(), None, "{format}");
        std::fs::write(root.join("staged.txt"), "s\n").unwrap();
        git(&root, &["add", "staged.txt"]);
        std::fs::write(root.join("loose.txt"), "l\n").unwrap();
        let before = git_snapshot(&root);
        let w = v.working_copy(Context::Default).unwrap().unwrap();
        assert_eq!(w.base, None, "{format}");
        let mut names: Vec<_> = w
            .diff
            .files
            .iter()
            .map(|f| (f.old_path.as_deref(), f.new_path.as_deref()))
            .collect();
        names.sort();
        assert_eq!(
            names,
            [(None, Some("loose.txt")), (None, Some("staged.txt"))],
            "{format}"
        );
        assert_same(&before.0, &git_snapshot(&root).0);
    }
}

#[test]
fn working_copy_writes_nothing() {
    let (_t, root) = repo();
    std::fs::write(root.join("a.txt"), "changed\n").unwrap();
    std::fs::write(root.join("new.txt"), "n\n").unwrap();
    // Touch a tracked file without changing it: a refreshing git would
    // rewrite the index's stat data.
    let keep = root.join("sub/keep.txt");
    let t = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
    std::fs::File::options()
        .write(true)
        .open(&keep)
        .unwrap()
        .set_modified(t)
        .unwrap();
    let before = git_snapshot(&root);
    let v = open(&root);
    for _ in 0..2 {
        v.working_copy(Context::Default).unwrap().unwrap();
    }
    let after = git_snapshot(&root);
    assert_same(&before.0, &after.0);
    assert_eq!(before.1, after.1);
}

#[test]
fn split_index_contents_stay_unchanged() {
    let (_t, root) = repo();
    git(&root, &["update-index", "--split-index"]);
    git(&root, &["config", "splitIndex.sharedIndexExpire", "now"]);
    let shared: Vec<_> = std::fs::read_dir(root.join(".git"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("sharedindex.")
        })
        .collect();
    assert!(!shared.is_empty(), "no shared index");
    // Age it, so a refresh would be visible.
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(30 * 86_400);
    for p in &shared {
        std::fs::File::options()
            .write(true)
            .open(p)
            .unwrap()
            .set_modified(old)
            .unwrap();
    }
    std::fs::write(root.join("a.txt"), "changed\n").unwrap();
    let before = git_snapshot(&root);
    let raw_before = snapshot(&root.join(".git"));
    let v = open(&root);
    v.working_copy(Context::Default).unwrap().unwrap();
    v.log(None, 5).unwrap();
    assert_same(&before.0, &git_snapshot(&root).0);
    // Every file's contents, shared index included, are unchanged.
    let raw_after = snapshot(&root.join(".git"));
    let contents = |s: &common::Snapshot| -> Vec<_> {
        s.iter().map(|(p, (h, n, _))| (p.clone(), *h, *n)).collect()
    };
    assert_eq!(contents(&raw_before), contents(&raw_after));
}

fn stat_row(s: &vcs::FileStat) -> (Option<&str>, &str, Option<u32>, Option<u32>) {
    (s.old_path.as_deref(), s.path.as_str(), s.added, s.removed)
}

#[test]
fn stat_counts_modify_add_delete_rename_and_binary() {
    let (_t, root) = repo();
    std::fs::write(root.join("bin"), b"\0\x01").unwrap();
    commit(&root, "add bin");
    std::fs::write(root.join("a.txt"), "one\nTWO\nthree\nfour\n").unwrap();
    std::fs::remove_file(root.join("sub/keep.txt")).unwrap();
    std::fs::write(root.join("bin"), b"\0\x02").unwrap();
    std::fs::write(root.join("new.txt"), "n\n").unwrap();
    git(&root, &["mv", "c d.txt", "e.txt"]);
    git(&root, &["add", "-A"]);
    git(
        &root,
        &[
            "-c",
            "user.name=Ann Author",
            "-c",
            "user.email=ann@example.com",
            "commit",
            "-q",
            "-m",
            "mixed\n\nbody",
        ],
    );
    let v = vcs::open(&root, vcs::OpenOptions::default()).unwrap();
    let head = v.log(None, 1).unwrap().remove(0);
    let s = v.stat(&head.commit).unwrap();
    assert_eq!(s.message, "mixed\n\nbody");
    assert_eq!(s.author, "Ann Author");
    assert_eq!(s.email, "ann@example.com");
    assert_eq!(s.time, head.time);
    let mut rows: Vec<_> = s.files.iter().map(stat_row).collect();
    rows.sort_by_key(|r| r.1);
    assert_eq!(
        rows,
        [
            (None, "a.txt", Some(1), Some(0)),
            (None, "bin", None, None),
            (Some("c d.txt"), "e.txt", Some(0), Some(0)),
            (None, "new.txt", Some(1), Some(0)),
            (None, "sub/keep.txt", Some(0), Some(1)),
        ]
    );
    // The root commit, and a short revision.
    let first = v.stat("HEAD~4").unwrap();
    assert_eq!(first.message, "first\n\nbody line");
    let rows: Vec<_> = first.files.iter().map(stat_row).collect();
    assert_eq!(
        rows,
        [
            (None, "a.txt", Some(3), Some(0)),
            (None, "sub/keep.txt", Some(1), Some(0))
        ]
    );
    assert!(v.stat("--all").is_err());
}

#[test]
fn stat_of_an_empty_commit_has_no_files() {
    let (_t, root) = repo();
    git(&root, &["commit", "-q", "--allow-empty", "-m", "empty"]);
    let v = vcs::open(&root, vcs::OpenOptions::default()).unwrap();
    let s = v.stat("HEAD").unwrap();
    assert_eq!(s.message, "empty");
    assert!(s.files.is_empty());
}

/// 40 numbered lines, then line 20 changed.
fn long_file_repo() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir(&root).unwrap();
    git(&root, &["init", "-q"]);
    let mut lines: Vec<String> = (1..=40).map(|i| format!("line {i}")).collect();
    std::fs::write(root.join("f.txt"), lines.join("\n") + "\n").unwrap();
    commit(&root, "base");
    lines[19] = "line twenty".into();
    std::fs::write(root.join("f.txt"), lines.join("\n") + "\n").unwrap();
    commit(&root, "change");
    (tmp, root.canonicalize().unwrap())
}

fn new_lines(f: &vcs::FileDiff) -> Vec<u32> {
    f.hunks
        .iter()
        .flat_map(|h| &h.lines)
        .filter_map(|l| l.new)
        .collect()
}

#[test]
fn full_context_shows_the_whole_file() {
    let (_t, root) = long_file_repo();
    let v = open(&root);
    let hunks = v.show("HEAD", Context::Default).unwrap();
    // Default is git's three lines of context.
    assert_eq!(new_lines(&hunks.files[0]), (17..=23).collect::<Vec<_>>());
    let full = v.show("HEAD", Context::Full).unwrap();
    assert_eq!(full.message, hunks.message);
    let f = &full.files[0];
    assert_eq!(f.hunks.len(), 1, "one hunk header per file");
    assert_eq!(new_lines(f), (1..=40).collect::<Vec<_>>());
    assert_eq!(f.hunks[0].lines[0].text, "line 1");
    let changed = |d: &vcs::CommitDiff| -> usize {
        d.files[0]
            .hunks
            .iter()
            .flat_map(|h| &h.lines)
            .filter(|l| l.kind != LineKind::Context)
            .count()
    };
    assert_eq!(changed(&full), changed(&hunks));
}

#[test]
fn full_context_working_copy() {
    let (_t, root) = long_file_repo();
    let text = std::fs::read_to_string(root.join("f.txt")).unwrap();
    std::fs::write(root.join("f.txt"), text.replace("line 2\n", "line two\n")).unwrap();
    let v = open(&root);
    let w = v.working_copy(Context::Default).unwrap().unwrap();
    assert_eq!(new_lines(&w.diff.files[0]), (1..=5).collect::<Vec<_>>());
    let w = v.working_copy(Context::Full).unwrap().unwrap();
    assert_eq!(new_lines(&w.diff.files[0]), (1..=40).collect::<Vec<_>>());
}
