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
    assert!(b > a);

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

    // Ids keep growing.
    assert!(r.add(note("a", 1, "three")).unwrap() > b);

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
fn clear_refuses_a_batch_for_another_root() {
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
    let other = text.replace(&canon, "\"/somewhere/else\"");
    fs::write(&file, &other).unwrap();
    let err = r.clear().unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    assert_eq!(fs::read_to_string(&file).unwrap(), other);
}

fn json_files(cache: &Path) -> usize {
    fs::read_dir(cache.join("reviews"))
        .unwrap()
        .filter(|e| e.as_ref().unwrap().path().extension().unwrap() == "json")
        .count()
}

#[test]
fn remove_all_keeps_comments_added_after_export() {
    let (cache, root) = dirs();
    let mut sender = Review::open(cache.path(), root.path()).unwrap();
    sender.add(note("a", 1, "one")).unwrap();
    sender.add(note("b", 1, "two")).unwrap();
    sender.reload().unwrap();
    let sent: Vec<u64> = sender.comments().iter().map(|c| c.id).collect();

    // Another handle adds a comment between the sender's export and its removal.
    let mut other = Review::open(cache.path(), root.path()).unwrap();
    let late = other.add(note("c", 1, "late")).unwrap();

    assert_eq!(sender.remove_all(&sent).unwrap(), 2);
    let ids: Vec<_> = sender.comments().iter().map(|c| c.id).collect();
    assert_eq!(ids, [late]);
    let fresh = Review::open(cache.path(), root.path()).unwrap();
    assert_eq!(fresh.comments().len(), 1);
    assert_eq!(fresh.comments()[0].body, "late");
}

#[test]
fn ids_never_repeat_after_the_batch_file_is_deleted() {
    let (cache, root) = dirs();
    let mut r = Review::open(cache.path(), root.path()).unwrap();
    let old = r.add(note("a", 1, "one")).unwrap();
    assert_eq!(r.remove_all(&[old]).unwrap(), 1);
    assert_eq!(json_files(cache.path()), 0);
    let mut other = Review::open(cache.path(), root.path()).unwrap();
    let new = other.add(note("a", 1, "two")).unwrap();
    assert!(new > old, "{new} should be > {old}");
}

#[test]
fn remove_all_ignores_missing_ids_and_deletes_empty_file() {
    let (cache, root) = dirs();
    let mut r = Review::open(cache.path(), root.path()).unwrap();
    // Nothing on disk: removing is a no-op that creates no batch file.
    assert_eq!(r.remove_all(&[1, 2]).unwrap(), 0);
    let a = r.add(note("a", 1, "one")).unwrap();
    let b = r.add(note("b", 1, "two")).unwrap();
    assert!(r.remove(a).unwrap());
    assert_eq!(r.remove_all(&[a, 99]).unwrap(), 0);
    assert_eq!(json_files(cache.path()), 1);
    assert_eq!(r.remove_all(&[a, b]).unwrap(), 1);
    assert!(r.comments().is_empty());
    assert_eq!(json_files(cache.path()), 0);
    assert!(
        Review::open(cache.path(), root.path())
            .unwrap()
            .comments()
            .is_empty()
    );
}

#[test]
fn remove_all_refuses_a_batch_for_another_root() {
    let (cache, root) = dirs();
    let mut r = Review::open(cache.path(), root.path()).unwrap();
    let id = r.add(note("x", 1, "b")).unwrap();
    let file = fs::read_dir(cache.path().join("reviews"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.extension().unwrap() == "json")
        .unwrap();
    let text = fs::read_to_string(&file).unwrap();
    let canon = root.path().canonicalize().unwrap();
    let canon = serde_json::to_string(canon.to_str().unwrap()).unwrap();
    let other = text.replace(&canon, "\"/somewhere/else\"");
    fs::write(&file, &other).unwrap();
    let err = r.remove_all(&[id]).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    assert_eq!(fs::read_to_string(&file).unwrap(), other);
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

fn json_file(cache: &Path) -> std::path::PathBuf {
    fs::read_dir(cache.join("reviews"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.extension().unwrap() == "json")
        .unwrap()
}

#[test]
fn kind_round_trips_and_untyped_is_not_stored() {
    let (cache, root) = dirs();
    let mut r = Review::open(cache.path(), root.path()).unwrap();
    let typed = r
        .add(note("a", 1, "x").with_kind(Some("issue".into())))
        .unwrap();
    let untyped = r.add(note("b", 1, "y")).unwrap();
    let json: serde_json::Value =
        serde_json::from_slice(&fs::read(json_file(cache.path())).unwrap()).unwrap();
    assert_eq!(json["version"], 1);
    assert_eq!(json["comments"][0]["kind"], "issue");
    assert!(json["comments"][1].get("kind").is_none());
    let fresh = Review::open(cache.path(), root.path()).unwrap();
    assert_eq!(fresh.comments(), r.comments());
    let kind = |id| {
        fresh
            .comments()
            .iter()
            .find(|c| c.id == id)
            .unwrap()
            .kind
            .clone()
    };
    assert_eq!(kind(typed).as_deref(), Some("issue"));
    assert_eq!(kind(untyped), None);
}

#[test]
fn v1_batch_without_kind_loads() {
    let (cache, root) = dirs();
    let mut r = Review::open(cache.path(), root.path()).unwrap();
    r.add(note("a", 1, "x")).unwrap();
    let canon = root.path().canonicalize().unwrap();
    let old = serde_json::json!({
        "version": 1,
        "root": canon,
        "comments": [{
            "id": 7,
            "path": "docs/plan.md",
            "rev": null,
            "side": null,
            "lines": [3, 4],
            "excerpt": "e",
            "body": "b",
            "created": 1700000000
        }]
    });
    fs::write(json_file(cache.path()), old.to_string()).unwrap();
    let r = Review::open(cache.path(), root.path()).unwrap();
    assert_eq!(r.comments().len(), 1);
    let c = &r.comments()[0];
    assert_eq!((c.id, c.lines, c.kind.clone()), (7, (3, 4), None));
}

#[test]
fn set_kind_sets_and_clears() {
    let (cache, root) = dirs();
    let mut r = Review::open(cache.path(), root.path()).unwrap();
    let id = r.add(note("a", 1, "x")).unwrap();
    assert!(r.set_kind(id, Some("nit")).unwrap());
    assert_eq!(r.comments()[0].kind.as_deref(), Some("nit"));
    let fresh = Review::open(cache.path(), root.path()).unwrap();
    assert_eq!(fresh.comments()[0].kind.as_deref(), Some("nit"));
    assert!(r.set_kind(id, None).unwrap());
    assert_eq!(r.comments()[0].kind, None);
    assert!(!r.set_kind(id + 1, Some("issue")).unwrap());
}

#[test]
fn builtin_kinds_are_the_four_with_definitions() {
    let ids: Vec<_> = debrief_review::builtin_kinds()
        .into_iter()
        .map(|k| {
            assert!(k.definition.is_some());
            k.id
        })
        .collect();
    assert_eq!(ids, ["issue", "suggestion", "question", "nit"]);
    assert_eq!(debrief_review::Kind::label("nit"), "NIT");
}

fn lock_file(cache: &Path) -> std::path::PathBuf {
    fs::read_dir(cache.join("reviews"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.extension().unwrap() == "lock")
        .unwrap()
}

#[test]
fn restore_keeps_ids_into_an_empty_batch() {
    let (cache, root) = dirs();
    let mut r = Review::open(cache.path(), root.path()).unwrap();
    let a = r.add(note("a", 1, "one")).unwrap();
    let b = r.add(note("b", 2, "two")).unwrap();
    let saved = r.comments().to_vec();
    r.clear().unwrap();
    let map = r.restore(&saved).unwrap();
    assert_eq!(map, [(a, a), (b, b)]);
    assert_eq!(r.comments(), saved);
    let fresh = Review::open(cache.path(), root.path()).unwrap();
    assert_eq!(fresh.comments(), saved);
    // add never reissues a restored id.
    let c = r.add(note("c", 1, "three")).unwrap();
    assert!(c > b);
}

#[test]
fn restore_skips_comments_already_present() {
    let (cache, root) = dirs();
    let mut r = Review::open(cache.path(), root.path()).unwrap();
    let a = r.add(note("a", 1, "one")).unwrap();
    let b = r.add(note("b", 2, "two")).unwrap();
    let saved = r.comments().to_vec();
    r.edit(a, "edited").unwrap();
    r.remove(b).unwrap();
    let map = r.restore(&saved).unwrap();
    assert_eq!(map, [(b, b)], "a is present: the batch's version wins");
    assert_eq!(r.comments().len(), 2);
    assert_eq!(r.comments()[0].body, "edited");
    assert!(r.restore(&saved).unwrap().is_empty(), "nothing twice");
    assert_eq!(r.comments().len(), 2);
}

#[test]
fn restore_gives_a_clashing_id_a_fresh_one() {
    let (cache, root) = dirs();
    let mut r = Review::open(cache.path(), root.path()).unwrap();
    let a = r.add(note("a", 1, "saved")).unwrap();
    let saved = r.comments().to_vec();
    r.clear().unwrap();
    // A wiped lock lets add reissue the id for a different comment.
    fs::remove_file(lock_file(cache.path())).unwrap();
    let mut clash = note("z", 9, "other");
    clash.id = a;
    clash.created = 1;
    r.restore(&[clash]).unwrap();
    assert_eq!(r.comments()[0].id, a);
    let map = r.restore(&saved).unwrap();
    assert_eq!(map.len(), 1);
    let (old, new) = map[0];
    assert_eq!(old, a);
    assert!(new > a, "{new} > {a}");
    let bodies: Vec<_> = r
        .comments()
        .iter()
        .map(|c| (c.id, c.body.as_str()))
        .collect();
    assert_eq!(bodies, [(new, "saved"), (a, "other")]);
    let last: u64 = fs::read_to_string(lock_file(cache.path()))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert_eq!(last, new);
    assert!(r.add(note("n", 1, "next")).unwrap() > new);
}

#[test]
fn restore_never_lowers_the_last_issued_id() {
    let (cache, root) = dirs();
    let mut r = Review::open(cache.path(), root.path()).unwrap();
    let a = r.add(note("a", 1, "one")).unwrap();
    let saved = r.comments().to_vec();
    r.clear().unwrap();
    let b = r.add(note("b", 1, "two")).unwrap();
    r.clear().unwrap();
    assert_eq!(r.restore(&saved).unwrap(), [(a, a)]);
    let c = r.add(note("c", 1, "three")).unwrap();
    assert!(c > b, "{c} > {b}");
}
