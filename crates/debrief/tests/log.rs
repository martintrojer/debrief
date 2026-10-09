mod common;

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use common::{JjEnv, commit, git, have_jj};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use debrief::app::watch::{RepoWatcher, watch_targets};
use debrief::app::{App, Mode, Screen, StartOptions};
use debrief::seen::{Seen, Status};
use debrief::vcs;
use ratatui::Terminal;
use ratatui::backend::TestBackend;

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
    commit(root, &format!("commit {i}"));
}

fn open(root: &Path, cache: &Path) -> App {
    App::new(
        vcs::open(root).unwrap(),
        StartOptions::new(cache.to_path_buf()),
    )
    .unwrap()
}

fn key(app: &mut App, c: char) {
    app.key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
}

fn keys(app: &mut App, s: &str) {
    s.chars().for_each(|c| key(app, c));
}

/// Render to an 80x12 TestBackend and return the screen's lines.
fn screen(app: &mut App) -> Vec<String> {
    let mut t = Terminal::new(TestBackend::new(80, 12)).unwrap();
    t.draw(|f| app.render(f)).unwrap();
    let buf = t.backend().buffer().clone();
    (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect()
}

fn markers(app: &App) -> Vec<Status> {
    app.rows().iter().map(|r| r.status).collect()
}

/// Mark only the oldest commit seen, as if debrief saw the repo then.
fn seed_oldest(root: &Path, cache: &Path) {
    let log = vcs::open(root).unwrap().log(None, 200).unwrap();
    Seen::open(cache, root)
        .unwrap()
        .mark(log.last().unwrap())
        .unwrap();
}

#[test]
fn first_open_shows_nothing_new_and_writes_the_seen_file() {
    let (tmp, root) = git_repo(3);
    let cache = tmp.path().join("cache");
    let mut app = open(&root, &cache);
    assert_eq!(markers(&app), [Status::Seen; 3]);
    assert!(app.seen().path().exists());
    assert_eq!(app.cursor(), 0);
    let s = screen(&mut app);
    assert!(!s.iter().any(|l| l.contains('●')), "{s:#?}");
    assert!(s[11].contains("0 new"), "{}", s[11]);
    drop(app);
    add_commit(&root, 3);
    let app = open(&root, &cache);
    assert_eq!(
        markers(&app),
        [Status::New, Status::Seen, Status::Seen, Status::Seen]
    );
}

#[test]
fn git_log_markers_cursor_mark_and_reload() {
    let (tmp, root) = git_repo(3);
    let cache = tmp.path().join("cache");
    seed_oldest(&root, &cache);
    let mut app = open(&root, &cache);
    assert_eq!(markers(&app), [Status::New, Status::New, Status::Seen]);
    assert_eq!(app.cursor(), 1, "the oldest new commit");
    let s = screen(&mut app);
    let log = vcs::open(&root).unwrap().log(None, 10).unwrap();
    assert!(
        s[0].starts_with(&format!("● {} ", &log[0].commit[..8])),
        "{s:#?}"
    );
    assert!(s[0].ends_with("t            commit 2"), "{}", s[0]);
    assert!(
        s[1].starts_with("● ") && s[1].ends_with("commit 1"),
        "{}",
        s[1]
    );
    assert!(
        s[2].starts_with("  ") && s[2].ends_with("commit 0"),
        "{}",
        s[2]
    );
    assert!(s[0].contains("  0s ") || s[0].contains("s "), "{}", s[0]);
    assert!(s[11].contains("2 new"), "{}", s[11]);
    assert!(s[11].contains(" git "), "{}", s[11]);
    assert!(s[11].contains("2/3"), "{}", s[11]);

    key(&mut app, 'm');
    assert_eq!(markers(&app), [Status::Seen; 3]);
    let s = screen(&mut app);
    assert!(!s.iter().any(|l| l.contains('●')), "{s:#?}");
    assert!(s[11].contains("0 new"), "{}", s[11]);

    // Reload: the new commit is on top, the cursor stays on its commit.
    let under = app.rows()[app.cursor()].info.commit.clone();
    add_commit(&root, 3);
    key(&mut app, 'r');
    assert_eq!(app.rows().len(), 4);
    assert_eq!(app.rows()[0].status, Status::New);
    assert_eq!(app.rows()[0].info.summary, "commit 3");
    assert_eq!(app.rows()[app.cursor()].info.commit, under);
    let s = screen(&mut app);
    assert!(
        s[0].starts_with("● ") && s[0].ends_with("commit 3"),
        "{}",
        s[0]
    );

    // m persisted across opens.
    drop(app);
    let app = open(&root, &cache);
    assert_eq!(
        markers(&app),
        [Status::New, Status::Seen, Status::Seen, Status::Seen]
    );
}

#[test]
fn motions_and_enter() {
    let (tmp, root) = git_repo(30);
    let cache = tmp.path().join("cache");
    seed_oldest(&root, &cache);
    let mut app = open(&root, &cache);
    assert_eq!(app.cursor(), 28);
    screen(&mut app);
    keys(&mut app, "gg");
    assert_eq!(app.cursor(), 0);
    key(&mut app, 'j');
    assert_eq!(app.cursor(), 1);
    keys(&mut app, "3j");
    assert_eq!(app.cursor(), 4);
    key(&mut app, 'k');
    assert_eq!(app.cursor(), 3);
    key(&mut app, 'G');
    assert_eq!(app.cursor(), 29);
    keys(&mut app, "2gg");
    assert_eq!(app.cursor(), 1);
    app.key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL));
    assert_eq!(app.cursor(), 6, "half of 11 log rows");
    app.key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
    assert_eq!(app.cursor(), 1);
    key(&mut app, 'G');
    let s = screen(&mut app);
    assert!(
        s[10].ends_with("commit 0"),
        "scrolled to the cursor: {s:#?}"
    );
    // Enter marks the commit seen and opens it.
    keys(&mut app, "gg");
    assert_eq!(app.rows()[0].status, Status::New);
    app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.rows()[0].status, Status::Seen);
    assert_eq!(app.screen(), Screen::Commit(0));
    key(&mut app, 'q');
    assert_eq!(app.screen(), Screen::Log);
    assert!(!app.should_quit());
    key(&mut app, 'q');
    assert!(app.should_quit());
}

#[test]
fn help_overlay_lists_the_keymap() {
    let (tmp, root) = git_repo(2);
    let mut app = open(&root, &tmp.path().join("cache"));
    keys(&mut app, "g?");
    assert_eq!(app.mode(), Mode::Help);
    let mut t = Terminal::new(TestBackend::new(80, 30)).unwrap();
    t.draw(|f| app.render(f)).unwrap();
    let text: String = format!("{:?}", t.backend().buffer());
    for want in [
        "help",
        "mark every listed commit seen",
        "reload the log",
        "Motions",
    ] {
        assert!(text.contains(want), "{want} missing:\n{text}");
    }
    key(&mut app, 'q');
    assert_eq!(app.mode(), Mode::Normal);
    assert!(!app.should_quit(), "q closes help, not the app");
}

#[test]
fn jj_log_markers_updated_and_reload() {
    if !have_jj() {
        return;
    }
    let e = JjEnv::new(false);
    e.write("a.txt", "a\n");
    e.jj(&["--quiet", "commit", "-m", "one"]);
    e.write("b.txt", "b\n");
    e.jj(&["--quiet", "commit", "-m", "two"]);
    let cache = e.root.parent().unwrap().join("cache");
    let opts = || StartOptions {
        revset: Some("::@".into()),
        ..StartOptions::new(cache.clone())
    };
    let app = App::new(vcs::open(&e.root).unwrap(), opts()).unwrap();
    // The root commit is not listed: @ (empty), two, one.
    let sums: Vec<_> = app.rows().iter().map(|r| r.info.summary.as_str()).collect();
    assert_eq!(sums, ["", "two", "one"]);
    assert!(
        app.rows()
            .iter()
            .all(|r| r.info.parents.iter().all(|p| !p.bytes().all(|b| b == b'0')))
    );
    assert_eq!(markers(&app), [Status::Seen; 3]);
    drop(app);

    e.jj(&["--quiet", "describe", "-r", "@-", "-m", "two, reworded"]);
    let mut app = App::new(vcs::open(&e.root).unwrap(), opts()).unwrap();
    // Rewriting @- rebases @: both are updated.
    assert_eq!(
        markers(&app),
        [Status::Updated, Status::Updated, Status::Seen]
    );
    assert_eq!(app.cursor(), 1);
    let s = screen(&mut app);
    let change = app.rows()[1].info.change.clone().unwrap();
    assert!(
        s[1].starts_with(&format!("◐ {} ", &change[..8])),
        "{}",
        s[1]
    );
    assert!(s[11].contains(" jj "), "{}", s[11]);
    assert!(s[11].contains("0 new, 2 updated"), "{}", s[11]);

    key(&mut app, 'm');
    assert_eq!(markers(&app), [Status::Seen; 3]);
    e.write("c.txt", "c\n");
    e.jj(&["--quiet", "commit", "-m", "three"]);
    key(&mut app, 'r');
    let sums: Vec<_> = app.rows().iter().map(|r| r.info.summary.as_str()).collect();
    assert_eq!(sums, ["", "three", "two, reworded", "one"]);
    // `jj commit` describes the seen `@` change (updated) and starts a
    // new, empty `@` (new).
    assert_eq!(
        markers(&app),
        [Status::New, Status::Updated, Status::Seen, Status::Seen]
    );
}

/// Wait for the watcher to fire within a few seconds.
fn fires(rx: &mpsc::Receiver<()>) -> bool {
    rx.recv_timeout(Duration::from_secs(5)).is_ok()
}

fn drain(rx: &mpsc::Receiver<()>) {
    while rx.recv_timeout(Duration::from_millis(600)).is_ok() {}
}

#[test]
fn git_watcher_fires_on_commit_in_a_linked_worktree() {
    let (tmp, root) = git_repo(1);
    let wt = tmp.path().join("wt");
    git(&root, &["worktree", "add", "-q", wt.to_str().unwrap()]);
    let wt = wt.canonicalize().unwrap();
    let v = vcs::open(&wt).unwrap();
    let t = watch_targets(v.as_ref()).unwrap();
    let common = root.join(".git");
    let head = common.join("worktrees/wt/HEAD");
    assert!(t.is_relevant(&head), "{t:?}");
    assert!(t.is_relevant(&common.join("refs/heads/wt")), "{t:?}");
    assert!(!t.is_relevant(&common.join("index")), "{t:?}");
    assert!(!t.is_relevant(&common.join("worktrees/wt/index")), "{t:?}");

    let (tx, rx) = mpsc::channel();
    let _w = RepoWatcher::start(t, move || {
        let _ = tx.send(());
    })
    .unwrap();
    std::thread::sleep(Duration::from_millis(200));
    drain(&rx);
    add_commit(&wt, 1);
    assert!(fires(&rx), "commit in the worktree reloads");
}

#[test]
fn git_watcher_fires_on_commit() {
    let (_tmp, root) = git_repo(1);
    let v = vcs::open(&root).unwrap();
    let (tx, rx) = mpsc::channel();
    let _w = RepoWatcher::start(watch_targets(v.as_ref()).unwrap(), move || {
        let _ = tx.send(());
    })
    .unwrap();
    std::thread::sleep(Duration::from_millis(200));
    drain(&rx);
    add_commit(&root, 1);
    assert!(fires(&rx));
}

#[test]
fn git_watcher_fires_on_commit_in_a_root_ending_in_newline() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("ends\n");
    if std::fs::create_dir(&root).is_err() {
        eprintln!("skipped: filesystem refuses a newline in a directory name");
        return;
    }
    git(&root, &["init", "-q"]);
    add_commit(&root, 0);
    let root = root.canonicalize().unwrap();
    let v = vcs::open(&root).unwrap();
    let t = watch_targets(v.as_ref()).unwrap();
    assert!(t.is_relevant(&root.join(".git/HEAD")), "{t:?}");
    let (tx, rx) = mpsc::channel();
    let _w = RepoWatcher::start(t, move || {
        let _ = tx.send(());
    })
    .unwrap();
    std::thread::sleep(Duration::from_millis(200));
    drain(&rx);
    add_commit(&root, 1);
    assert!(fires(&rx), "external commit reloads");
}

#[test]
fn jj_watcher_follows_a_secondary_workspace() {
    if !have_jj() {
        return;
    }
    let e = JjEnv::new(false);
    e.write("a.txt", "a\n");
    e.jj(&["--quiet", "commit", "-m", "one"]);
    let ws = e.root.parent().unwrap().join("ws2");
    e.jj(&["--quiet", "workspace", "add", ws.to_str().unwrap()]);
    let v = vcs::open(&ws).unwrap();
    let t = watch_targets(v.as_ref()).unwrap();
    let heads = e.root.join(".jj/repo/op_heads/heads");
    assert!(t.is_relevant(&heads.join("abc")), "{t:?}");

    let (tx, rx) = mpsc::channel();
    let _w = RepoWatcher::start(t, move || {
        let _ = tx.send(());
    })
    .unwrap();
    std::thread::sleep(Duration::from_millis(200));
    drain(&rx);
    e.jj(&["--quiet", "describe", "-r", "@-", "-m", "one, reworded"]);
    assert!(fires(&rx), "an operation in the main workspace reloads");
}
