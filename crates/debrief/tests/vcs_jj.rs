mod common;

use common::{JjEnv as Env, assert_same, have_jj, snapshot};
use debrief::vcs::{self, Context, Kind, LineKind, OpenOptions};

/// The strict read-only tests open without snapshots.
const NO_SNAPSHOT: OpenOptions = OpenOptions { snapshot: false };

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
    let v = vcs::open(&e.root.join("a.txt"), NO_SNAPSHOT).unwrap();
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
    let v = vcs::open(&e.root, NO_SNAPSHOT).unwrap();
    let log = v.log(Some("::@ ~ root()"), 200).unwrap();
    let second = v.show(&log[1].commit, Context::Default).unwrap();
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
    let first = v.show(&log[2].commit, Context::Default).unwrap();
    assert_eq!(first.message, "first\n\nbody");
    // The empty working-copy commit has no files.
    assert!(
        v.show(&log[0].commit, Context::Default)
            .unwrap()
            .files
            .is_empty()
    );
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
    let v = vcs::open(&e.root, NO_SNAPSHOT).unwrap();
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
    let v = vcs::open(&e.root, NO_SNAPSHOT).unwrap();
    for c in v.log(None, 200).unwrap() {
        v.show(&c.commit, Context::Default).unwrap();
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
    let v = vcs::open(&e.root, NO_SNAPSHOT).unwrap();
    // We refuse rather than let jj merge (or lock) the op heads.
    let err = format!("{:#}", v.log(None, 10).err().unwrap());
    assert!(err.contains("divergent jj operation heads"), "{err}");
    assert!(
        v.show("0000000000000000000000000000000000000000", Context::Default)
            .is_err()
    );
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
    let v = vcs::open(&ws, NO_SNAPSHOT).unwrap();
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
    let v = vcs::open(&e.root, NO_SNAPSHOT).unwrap();
    let out = e.root.parent().unwrap().join("x");
    let flag = format!("--output={}", out.display());
    assert!(v.log(Some(&flag), 10).is_err());
    assert!(v.show(&flag, Context::Default).is_err());
    assert!(
        v.show("@-", Context::Default).is_err(),
        "show takes IDs, not revsets"
    );
    assert!(!out.exists());
}

#[test]
fn failed_command_carries_stderr() {
    if !have_jj() {
        return;
    }
    let e = sample();
    let v = vcs::open(&e.root, NO_SNAPSHOT).unwrap();
    let err = format!("{:#}", v.log(Some("nosuchbookmark"), 10).err().unwrap());
    assert!(err.contains("nosuchbookmark"), "{err}");
}

// Snapshots.

/// The text of `a.txt`'s added lines in `@`'s diff.
fn at_added(v: &dyn vcs::Vcs) -> Vec<String> {
    let at = v.working_copy_commit().unwrap().unwrap();
    v.show(&at, Context::Default)
        .unwrap()
        .files
        .iter()
        .flat_map(|f| f.hunks.iter().flat_map(|h| &h.lines))
        .filter(|l| l.kind == LineKind::Added)
        .map(|l| l.text.clone())
        .collect()
}

#[test]
fn snapshot_makes_an_edit_visible_in_at() {
    if !have_jj() {
        return;
    }
    let e = sample();
    e.write("a.txt", "one\nTWO\nedited\n");
    let ops = e.op_count();
    let v = vcs::open(&e.root, OpenOptions::default()).unwrap();
    // Before the snapshot, reads are as of the last jj command.
    assert!(at_added(v.as_ref()).is_empty());
    v.refresh(true).unwrap();
    assert_eq!(at_added(v.as_ref()), ["edited"]);
    assert_eq!(e.op_count(), ops + 1, "one snapshot operation");
    // Nothing new to record: no further operation.
    v.refresh(true).unwrap();
    assert_eq!(e.op_count(), ops + 1);
    // The edit is on disk untouched.
    let text = std::fs::read_to_string(e.root.join("a.txt")).unwrap();
    assert_eq!(text, "one\nTWO\nedited\n");
}

#[test]
fn no_snapshot_leaves_the_repo_alone() {
    if !have_jj() {
        return;
    }
    let e = sample();
    e.write("a.txt", "one\nTWO\nedited\n");
    let ops = e.op_count();
    let before = snapshot(&e.root.join(".jj"));
    let v = vcs::open(&e.root, NO_SNAPSHOT).unwrap();
    v.refresh(true).unwrap();
    assert!(at_added(v.as_ref()).is_empty());
    assert_eq!(e.op_count(), ops);
    assert_same(&before, &snapshot(&e.root.join(".jj")));
}

/// Make jj unable to write operations (as if another process held it).
fn lock_op_store(e: &Env, locked: bool) {
    use std::os::unix::fs::PermissionsExt;
    let mode = if locked { 0o555 } else { 0o755 };
    for d in ["op_store", "op_store/operations"] {
        let p = e.root.join(".jj/repo").join(d);
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(mode)).unwrap();
    }
}

#[test]
fn a_failed_snapshot_keeps_the_last_pin() {
    if !have_jj() {
        return;
    }
    let e = sample();
    e.write("a.txt", "one\nTWO\nfirst edit\n");
    let v = vcs::open(&e.root, OpenOptions::default()).unwrap();
    v.refresh(true).unwrap();
    assert_eq!(at_added(v.as_ref()), ["first edit"]);
    e.write("a.txt", "one\nTWO\nsecond edit\n");
    lock_op_store(&e, true);
    let err = format!("{:#}", v.refresh(true).unwrap_err());
    lock_op_store(&e, false);
    assert!(
        err.contains("jj snapshot failed; showing the last snapshot"),
        "{err}"
    );
    assert_eq!(at_added(v.as_ref()), ["first edit"], "still on the pin");
    assert!(!v.log(None, 10).unwrap().is_empty());
}

#[test]
fn a_failed_first_snapshot_reports_and_reads_the_op_head() {
    if !have_jj() {
        return;
    }
    let e = sample();
    e.write("a.txt", "one\nTWO\nedit\n");
    let v = vcs::open(&e.root, OpenOptions::default()).unwrap();
    lock_op_store(&e, true);
    let err = format!("{:#}", v.refresh(true).unwrap_err());
    lock_op_store(&e, false);
    assert!(err.starts_with("jj snapshot failed: "), "{err}");
    assert!(!err.contains("last snapshot"), "{err}");
    // Reads still work, as of the last jj command.
    assert!(at_added(v.as_ref()).is_empty());
}

#[test]
fn working_copy_commit_is_this_workspaces_at() {
    if !have_jj() {
        return;
    }
    let e = sample();
    let v = vcs::open(&e.root, NO_SNAPSHOT).unwrap();
    let at = e.jj(&[
        "--ignore-working-copy",
        "log",
        "--no-graph",
        "-r",
        "@",
        "-T",
        "commit_id",
    ]);
    assert_eq!(v.working_copy_commit().unwrap().as_deref(), Some(at.trim()));
    assert_eq!(
        v.working_copy(Context::Default).unwrap(),
        None,
        "jj has no extra row"
    );
}

#[test]
fn stat_counts_lines_per_file() {
    if !have_jj() {
        return;
    }
    let e = sample();
    let v = vcs::open(&e.root, NO_SNAPSHOT).unwrap();
    let log = v.log(Some("::@ ~ root()"), 200).unwrap();
    let s = v.stat(&log[1].commit).unwrap();
    assert_eq!(s.message, "second");
    assert_eq!(s.author, "t");
    assert_eq!(s.email, "t@t");
    assert_eq!(s.time, log[1].time);
    let rows: Vec<_> = s
        .files
        .iter()
        .map(|f| (f.old_path.as_deref(), f.path.as_str(), f.added, f.removed))
        .collect();
    assert_eq!(
        rows,
        [
            (None, "a.txt", Some(1), Some(1)),
            (None, "b.txt", Some(1), Some(0))
        ]
    );
    let first = v.stat(&log[2].commit).unwrap();
    assert_eq!(first.message, "first\n\nbody");
    // A change ID works as in show; the empty @ has no files.
    assert!(
        v.stat(log[0].change.as_deref().unwrap())
            .unwrap()
            .files
            .is_empty()
    );
}

#[test]
fn stat_rename_and_binary() {
    if !have_jj() {
        return;
    }
    let e = Env::new(false);
    e.write("a.txt", "a\nb\nc\nd\n");
    std::fs::write(e.root.join("bin"), b"\0\x01").unwrap();
    e.jj(&["--quiet", "commit", "-m", "one"]);
    std::fs::rename(e.root.join("a.txt"), e.root.join("z.txt")).unwrap();
    std::fs::write(e.root.join("bin"), b"\0\x02").unwrap();
    e.jj(&["--quiet", "commit", "-m", "two"]);
    let v = vcs::open(&e.root, NO_SNAPSHOT).unwrap();
    let log = v.log(Some("::@ ~ root()"), 200).unwrap();
    let s = v.stat(&log[1].commit).unwrap();
    let rows: Vec<_> = s
        .files
        .iter()
        .map(|f| (f.old_path.as_deref(), f.path.as_str(), f.added, f.removed))
        .collect();
    assert_eq!(
        rows,
        [
            (None, "bin", None, None),
            (Some("a.txt"), "z.txt", Some(0), Some(0))
        ]
    );
}

#[test]
fn full_context_shows_the_whole_file() {
    if !have_jj() {
        return;
    }
    let e = Env::new(false);
    let mut lines: Vec<String> = (1..=40).map(|i| format!("line {i}")).collect();
    e.write("f.txt", &(lines.join("\n") + "\n"));
    e.jj(&["--quiet", "commit", "-m", "base"]);
    lines[19] = "line twenty".into();
    e.write("f.txt", &(lines.join("\n") + "\n"));
    e.jj(&["--quiet", "commit", "-m", "change"]);
    let v = vcs::open(&e.root, NO_SNAPSHOT).unwrap();
    let id = v.log(Some("@-"), 1).unwrap()[0].commit.clone();
    let new = |d: &vcs::CommitDiff| -> Vec<u32> {
        d.files[0]
            .hunks
            .iter()
            .flat_map(|h| &h.lines)
            .filter_map(|l| l.new)
            .collect()
    };
    let hunks = v.show(&id, Context::Default).unwrap();
    assert_eq!(new(&hunks), (17..=23).collect::<Vec<_>>());
    let full = v.show(&id, Context::Full).unwrap();
    assert_eq!(full.message, hunks.message);
    assert_eq!(full.files[0].hunks.len(), 1);
    assert_eq!(new(&full), (1..=40).collect::<Vec<_>>());
}
