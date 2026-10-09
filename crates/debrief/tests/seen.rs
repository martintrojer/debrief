mod common;

use std::path::{Path, PathBuf};

use common::{JjEnv, commit, git, have_jj};
use debrief::seen::{Seen, Status};
use debrief::vcs::{self, CommitInfo};

fn git_repo(n: usize) -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir(&root).unwrap();
    git(&root, &["init", "-q"]);
    for i in 0..n {
        add_commit(&root, i);
    }
    let root = root.canonicalize().unwrap();
    (tmp, root)
}

fn add_commit(root: &Path, i: usize) {
    std::fs::write(root.join(format!("f{i}.txt")), format!("{i}\n")).unwrap();
    commit(root, &format!("c{i}"));
}

fn log(root: &Path) -> Vec<CommitInfo> {
    vcs::open(root, vcs::OpenOptions::default())
        .unwrap()
        .log(None, 200)
        .unwrap()
}

#[test]
fn first_open_is_fresh_and_mark_all_writes_the_file() {
    let (tmp, root) = git_repo(2);
    let cache = tmp.path().join("cache");
    let mut s = Seen::open(&cache, &root).unwrap();
    assert!(s.is_fresh());
    assert!(!s.path().exists(), "nothing written before a mark");
    assert!(s.path().starts_with(cache.join("seen")));
    let name = s.path().file_name().unwrap().to_str().unwrap();
    assert_eq!(name.len(), 16 + ".json".len());
    let commits = log(&root);
    s.mark_all(&commits).unwrap();
    assert!(!s.is_fresh());
    let json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(s.path()).unwrap()).unwrap();
    assert_eq!(json["version"], 1);
    assert_eq!(json["root"], root.to_str().unwrap());
    assert_eq!(json["commits"].as_array().unwrap().len(), 2);

    let s = Seen::open(&cache, &root).unwrap();
    assert!(!s.is_fresh());
    assert!(commits.iter().all(|c| s.status(c) == Status::Seen));
}

#[test]
fn new_commit_is_new_and_mark_persists() {
    let (tmp, root) = git_repo(1);
    let cache = tmp.path().join("cache");
    Seen::open(&cache, &root)
        .unwrap()
        .mark_all(&log(&root))
        .unwrap();
    add_commit(&root, 1);
    let commits = log(&root);
    let mut s = Seen::open(&cache, &root).unwrap();
    assert_eq!(s.status(&commits[0]), Status::New);
    assert_eq!(s.status(&commits[1]), Status::Seen);
    s.mark(&commits[0]).unwrap();
    let s = Seen::open(&cache, &root).unwrap();
    assert_eq!(s.status(&commits[0]), Status::Seen);
}

#[test]
fn separate_roots_get_separate_files() {
    let (tmp, a) = git_repo(1);
    let (_t2, b) = git_repo(1);
    let cache = tmp.path().join("cache");
    let sa = Seen::open(&cache, &a).unwrap();
    let sb = Seen::open(&cache, &b).unwrap();
    assert_ne!(sa.path(), sb.path());
}

#[test]
fn corrupt_file_is_an_error() {
    let (tmp, root) = git_repo(1);
    let cache = tmp.path().join("cache");
    let s = Seen::open(&cache, &root).unwrap();
    std::fs::create_dir_all(s.path().parent().unwrap()).unwrap();
    std::fs::write(s.path(), "not json").unwrap();
    assert!(Seen::open(&cache, &root).is_err());
}

#[test]
fn jj_rewrite_is_updated() {
    if !have_jj() {
        return;
    }
    let e = JjEnv::new(false);
    e.write("a.txt", "a\n");
    e.jj(&["--quiet", "commit", "-m", "one"]);
    let cache = e.root.parent().unwrap().join("cache");
    let v = vcs::open(&e.root, vcs::OpenOptions::default()).unwrap();
    let before = v.log(Some("@-"), 10).unwrap();
    let mut s = Seen::open(&cache, &e.root).unwrap();
    s.mark_all(&before).unwrap();
    e.jj(&["--quiet", "describe", "-r", "@-", "-m", "one, reworded"]);
    let after = v.log(Some("@-"), 10).unwrap();
    assert_eq!(after[0].change, before[0].change);
    assert_ne!(after[0].commit, before[0].commit);
    let s = Seen::open(&cache, &e.root).unwrap();
    assert_eq!(s.status(&after[0]), Status::Updated);
    assert_eq!(s.status(&before[0]), Status::Seen);
    // A brand-new change is New.
    e.write("b.txt", "b\n");
    e.jj(&["--quiet", "commit", "-m", "two"]);
    let two = v.log(Some("@-"), 10).unwrap();
    assert_eq!(s.status(&two[0]), Status::New);
}
