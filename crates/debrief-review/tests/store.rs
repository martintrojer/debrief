use std::fs;
use std::path::Path;
use std::process::Command;

use debrief_review::{Comment, Rev, Review, Side, repo_root};
use tempfile::TempDir;

fn dirs() -> (TempDir, TempDir) {
    (TempDir::new().unwrap(), TempDir::new().unwrap())
}

fn note(path: &str, line: u32, body: &str) -> Comment {
    Comment::on_file(path, line..=line, "excerpt", body)
}

#[test]
fn open_creates_nothing() {
    let (cache, root) = dirs();
    let r = Review::open(cache.path(), root.path()).unwrap();
    assert!(r.comments().is_empty());
    assert_eq!(r.root(), root.path().canonicalize().unwrap());
    assert_eq!(fs::read_dir(cache.path()).unwrap().count(), 0);
}

#[test]
fn json_without_lock_still_opens() {
    let (cache, root) = dirs();
    let mut r = Review::open(cache.path(), root.path()).unwrap();
    r.add(note("x", 1, "b")).unwrap();
    for e in fs::read_dir(cache.path().join("reviews")).unwrap() {
        let p = e.unwrap().path();
        if p.extension().unwrap() == "lock" {
            fs::remove_file(p).unwrap();
        }
    }
    let fresh = Review::open(cache.path(), root.path()).unwrap();
    assert_eq!(fresh.comments(), r.comments());
}

#[test]
fn add_edit_remove_clear_round_trip() {
    let (cache, root) = dirs();
    let mut r = Review::open(cache.path(), root.path()).unwrap();
    let rev = Rev {
        commit: "3f2a9c1e0000".into(),
        change: Some("kmxqpwrz".into()),
        summary: Some("feat: x".into()),
    };
    let a = r.add(note("docs/plan.md", 12, "one")).unwrap();
    let b = r
        .add(Comment::on_diff(
            "src/x.rs",
            rev.clone(),
            Side::New,
            40..=44,
            "+x",
            "two",
        ))
        .unwrap();
    assert_eq!((a, b), (1, 2));

    let fresh = Review::open(cache.path(), root.path()).unwrap();
    assert_eq!(fresh.comments(), r.comments());
    let c = &fresh.comments()[1];
    assert_eq!(c.rev.as_ref(), Some(&rev));
    assert_eq!(c.side, Some(Side::New));
    assert_eq!(c.lines, (40, 44));
    assert!(c.created > 0);

    assert!(r.edit(a, "one, edited").unwrap());
    assert!(!r.edit(99, "nope").unwrap());
    let fresh = Review::open(cache.path(), root.path()).unwrap();
    assert_eq!(fresh.comments()[0].body, "one, edited");

    assert!(r.remove(a).unwrap());
    assert!(!r.remove(a).unwrap());
    let fresh = Review::open(cache.path(), root.path()).unwrap();
    assert_eq!(fresh.comments().len(), 1);
    assert_eq!(fresh.comments()[0].id, b);

    // Ids keep counting from the max.
    assert_eq!(r.add(note("a", 1, "three")).unwrap(), 3);

    r.clear().unwrap();
    assert!(r.comments().is_empty());
    assert!(
        Review::open(cache.path(), root.path())
            .unwrap()
            .comments()
            .is_empty()
    );
    let jsons = fs::read_dir(cache.path().join("reviews"))
        .unwrap()
        .filter(|e| e.as_ref().unwrap().path().extension().unwrap() == "json")
        .count();
    assert_eq!(jsons, 0);
}

#[test]
fn side_serialises_lowercase() {
    let (cache, root) = dirs();
    let mut r = Review::open(cache.path(), root.path()).unwrap();
    let rev = Rev {
        commit: "c".into(),
        change: None,
        summary: None,
    };
    r.add(Comment::on_diff("f", rev, Side::Old, 1..=1, "-x", "b"))
        .unwrap();
    let file = fs::read_dir(cache.path().join("reviews"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.extension().unwrap() == "json")
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&fs::read(file).unwrap()).unwrap();
    assert_eq!(json["version"], 1);
    assert_eq!(
        json["root"],
        root.path().canonicalize().unwrap().to_str().unwrap()
    );
    assert_eq!(json["comments"][0]["side"], "old");
}

#[test]
fn comments_sorted_by_path_line_id() {
    let (cache, root) = dirs();
    let mut r = Review::open(cache.path(), root.path()).unwrap();
    r.add(note("b.rs", 1, "b1")).unwrap();
    r.add(note("a.rs", 9, "a9")).unwrap();
    r.add(note("a.rs", 2, "a2 first")).unwrap();
    r.add(note("a.rs", 2, "a2 second")).unwrap();
    let bodies: Vec<_> = r.comments().iter().map(|c| c.body.as_str()).collect();
    assert_eq!(bodies, ["a2 first", "a2 second", "a9", "b1"]);
    let fresh = Review::open(cache.path(), root.path()).unwrap();
    assert_eq!(fresh.comments(), r.comments());
}

#[test]
fn reload_sees_other_handles() {
    let (cache, root) = dirs();
    let mut a = Review::open(cache.path(), root.path()).unwrap();
    let mut b = Review::open(cache.path(), root.path()).unwrap();
    assert!(!b.reload().unwrap());
    a.add(note("x", 1, "from a")).unwrap();
    assert!(b.reload().unwrap());
    assert_eq!(b.comments().len(), 1);
    assert!(!b.reload().unwrap());
    // A mutation through b re-reads first, so it keeps a's comment.
    b.add(note("y", 1, "from b")).unwrap();
    assert!(a.reload().unwrap());
    assert_eq!(a.comments().len(), 2);
}

#[test]
fn root_mismatch_is_an_error() {
    let (cache, root) = dirs();
    let mut r = Review::open(cache.path(), root.path()).unwrap();
    r.add(note("x", 1, "b")).unwrap();
    let file = fs::read_dir(cache.path().join("reviews"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.extension().unwrap() == "json")
        .unwrap();
    let text = fs::read_to_string(&file).unwrap();
    let canon = root.path().canonicalize().unwrap();
    let canon = serde_json::to_string(canon.to_str().unwrap()).unwrap();
    fs::write(&file, text.replace(&canon, "\"/somewhere/else\"")).unwrap();
    let err = Review::open(cache.path(), root.path()).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    assert!(r.add(note("x", 2, "b")).is_err());
}

#[test]
fn threads_adding_concurrently_get_distinct_ids() {
    let (cache, root) = dirs();
    let handles: Vec<_> = (0..2)
        .map(|_| {
            let (cache, root) = (cache.path().to_owned(), root.path().to_owned());
            std::thread::spawn(move || {
                let mut r = Review::open(&cache, &root).unwrap();
                for i in 0..50 {
                    r.add(note("f", i, "b")).unwrap();
                }
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    assert_distinct_ids(cache.path(), root.path(), 100);
}

const CHILD: &str = "DEBRIEF_TEST_CHILD";

/// Re-run as a child process: add 50 comments, then exit.
#[test]
fn processes_adding_concurrently_get_distinct_ids() {
    if let Some(dirs) = std::env::var_os(CHILD) {
        let dirs = dirs.into_string().unwrap();
        let (cache, root) = dirs.split_once('\n').unwrap();
        let mut r = Review::open(Path::new(cache), Path::new(root)).unwrap();
        for i in 0..50 {
            r.add(note("f", i, "b")).unwrap();
        }
        return;
    }
    let (cache, root) = dirs();
    let env = format!("{}\n{}", cache.path().display(), root.path().display());
    let exe = std::env::current_exe().unwrap();
    let children: Vec<_> = (0..2)
        .map(|_| {
            Command::new(&exe)
                .args([
                    "--exact",
                    "processes_adding_concurrently_get_distinct_ids",
                    "--nocapture",
                ])
                .env(CHILD, &env)
                .spawn()
                .unwrap()
        })
        .collect();
    for mut c in children {
        assert!(c.wait().unwrap().success());
    }
    assert_distinct_ids(cache.path(), root.path(), 100);
}

fn assert_distinct_ids(cache: &Path, root: &Path, n: usize) {
    let r = Review::open(cache, root).unwrap();
    let mut ids: Vec<_> = r.comments().iter().map(|c| c.id).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), n);
    assert_eq!(r.comments().len(), n);
}

#[test]
fn repo_root_finds_nearest_vcs_dir() {
    let tmp = TempDir::new().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let repo = base.join("repo");
    let ws = repo.join("ws");
    fs::create_dir_all(repo.join(".git")).unwrap();
    fs::create_dir_all(ws.join(".jj")).unwrap();
    fs::create_dir_all(ws.join("src/deep")).unwrap();
    fs::create_dir_all(repo.join("docs")).unwrap();
    fs::write(repo.join("docs/plan.md"), "x").unwrap();
    fs::create_dir_all(base.join("plain/sub")).unwrap();
    fs::write(base.join("plain/sub/f.txt"), "x").unwrap();

    assert_eq!(repo_root(&repo.join("docs/plan.md")), repo);
    assert_eq!(repo_root(&repo.join("docs")), repo);
    // A jj workspace with its own .jj is its own root.
    assert_eq!(repo_root(&ws.join("src/deep")), ws);
    // Outside any VCS: the directory (of the file).
    assert_eq!(
        repo_root(&base.join("plain/sub/f.txt")),
        base.join("plain/sub")
    );
    assert_eq!(repo_root(&base.join("plain")), base.join("plain"));
    // Canonicalised.
    assert_eq!(repo_root(&repo.join("docs/../docs")), repo);
}
