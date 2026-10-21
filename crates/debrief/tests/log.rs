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

/// The log alone (the stats pane off), so rows fill the screen.
fn open(root: &Path, cache: &Path) -> App {
    let mut opts = StartOptions::new(cache.to_path_buf());
    opts.config.view.stats = false;
    App::new(vcs::open(root, vcs::OpenOptions::default()).unwrap(), opts).unwrap()
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
    let log = vcs::open(root, vcs::OpenOptions::default())
        .unwrap()
        .log(None, 200)
        .unwrap();
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
    let log = vcs::open(&root, vcs::OpenOptions::default())
        .unwrap()
        .log(None, 10)
        .unwrap();
    // The branch chip on top, the log under it.
    assert!(s[0].starts_with(" ⎇ "), "{s:#?}");
    assert!(
        s[1].starts_with(&format!("● {} ", &log[0].commit[..8])),
        "{s:#?}"
    );
    assert!(s[1].ends_with("t            commit 2"), "{}", s[1]);
    assert!(
        s[2].starts_with("● ") && s[2].ends_with("commit 1"),
        "{}",
        s[2]
    );
    assert!(
        s[3].starts_with("  ") && s[3].ends_with("commit 0"),
        "{}",
        s[3]
    );
    assert!(s[1].contains("  0s ") || s[1].contains("s "), "{}", s[1]);
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
        s[1].starts_with("● ") && s[1].ends_with("commit 3"),
        "{}",
        s[1]
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
    assert_eq!(app.cursor(), 6, "half of 10 log rows");
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
    let opts = || StartOptions::new(cache.clone());
    let app = App::new(
        vcs::open(&e.root, vcs::OpenOptions::default()).unwrap(),
        opts(),
    )
    .unwrap();
    // The default log is @ and its ancestors, without the root commit:
    // @ (empty), two, one.
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
    let mut app = App::new(
        vcs::open(&e.root, vcs::OpenOptions::default()).unwrap(),
        opts(),
    )
    .unwrap();
    // Rewriting @- rebases @: both are updated.
    assert_eq!(
        markers(&app),
        [Status::Updated, Status::Updated, Status::Seen]
    );
    assert_eq!(app.cursor(), 1);
    let s = screen(&mut app);
    let change = app.rows()[1].info.change.clone().unwrap();
    assert!(
        s[2].starts_with(&format!("◐ {} ", &change[..8])),
        "{}",
        s[2]
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
    let v = vcs::open(&wt, vcs::OpenOptions::default()).unwrap();
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
    let v = vcs::open(&root, vcs::OpenOptions::default()).unwrap();
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
    let v = vcs::open(&root, vcs::OpenOptions::default()).unwrap();
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
    let v = vcs::open(&ws, vcs::OpenOptions::default()).unwrap();
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

/// Open with the stats pane on (the default config).
fn open_with_stats(root: &Path, cache: &Path) -> App {
    App::new(
        vcs::open(root, vcs::OpenOptions::default()).unwrap(),
        StartOptions::new(cache.to_path_buf()),
    )
    .unwrap()
}

fn screen_at(app: &mut App, w: u16, h: u16) -> Vec<String> {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| app.render(f)).unwrap();
    let buf = t.backend().buffer().clone();
    (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect::<String>()
        })
        .collect()
}

/// Rest the cursor long enough for the stat to load (no sender: loads
/// in the tick).
fn settle(app: &mut App) {
    let t = std::time::Instant::now();
    app.tick(t);
    app.tick(t + debrief::app::stat::STAT_DELAY);
}

#[test]
fn stats_pane_side_by_side_at_120_columns_stacked_at_119() {
    let (tmp, root) = git_repo(3);
    let mut app = open_with_stats(&root, &tmp.path().join("cache"));
    let s = screen_at(&mut app, 120, 20);
    assert!(s[1].contains("loading…"), "{s:#?}");
    settle(&mut app);
    let s = screen_at(&mut app, 120, 20);
    // The branch chip on top; under it the log 72 columns, the border at
    // column 72, the pane after it.
    assert!(s[0].starts_with(" ⎇ ") && !s[0].contains('│'), "{s:#?}");
    for row in &s[1..19] {
        assert_eq!(row.chars().nth(72), Some('│'), "{s:#?}");
    }
    assert!(
        s[1].chars()
            .take(72)
            .collect::<String>()
            .contains("commit 2")
    );
    let pane: Vec<String> = s[1..]
        .iter()
        .map(|r| r.chars().skip(74).collect())
        .collect();
    // One header line (wrapped here): ID, author, date.
    assert!(pane[0].contains("  t <t@t>  "), "{pane:#?}");
    assert!(pane[0].contains(" UTC"), "{pane:#?}");
    assert!(pane[2].starts_with("commit 2"), "{pane:#?}");
    assert!(pane[4].starts_with("f2.txt | 1 +"), "{pane:#?}");
    assert!(
        pane[5].starts_with("1 file changed, 1 insertion(+), 0 deletions(-)"),
        "{pane:#?}"
    );

    let s = screen_at(&mut app, 119, 20);
    // 18 rows between the chip and the status line: the pane is the
    // bottom 40% (7), its top border on row 12.
    assert!(s[12].trim_end().chars().all(|c| c == '─'), "{s:#?}");
    assert!(s[1].contains("commit 2") && !s[1].contains('│'));
    assert!(s[13].contains(&app.rows()[0].info.commit[..12]), "{s:#?}");

    // <leader>e hides it.
    keys(&mut app, " e");
    let s = screen_at(&mut app, 119, 20);
    assert!(!s.iter().any(|r| r.contains('─')), "{s:#?}");
    keys(&mut app, " e");
    assert!(screen_at(&mut app, 119, 20)[12].starts_with('─'));
}

#[test]
fn a_wide_terminal_gives_the_stats_pane_what_the_log_doesnt_need() {
    let (tmp, root) = git_repo(3);
    let mut app = open_with_stats(&root, &tmp.path().join("cache"));
    settle(&mut app);
    // Short summaries: the log keeps its 72-column floor, the pane the rest.
    let s = screen_at(&mut app, 250, 20);
    assert_eq!(s[1].chars().nth(72), Some('│'), "{s:#?}");
    // A long summary widens the log, up to 60% (150 of 250).
    std::fs::write(root.join("long.txt"), "x\n").unwrap();
    commit(&root, &"long subject ".repeat(20));
    key(&mut app, 'r');
    let s = screen_at(&mut app, 250, 20);
    assert_eq!(s[1].chars().nth(150), Some('│'), "{s:#?}");
}

#[test]
fn stats_load_after_the_cursor_rests_and_stale_answers_drop() {
    use debrief::app::AppEvent;
    use debrief::app::stat::STAT_DELAY;
    let (tmp, root) = git_repo(3);
    let mut app = open_with_stats(&root, &tmp.path().join("cache"));
    let (tx, rx) = mpsc::channel();
    app.set_sender(tx);
    let t0 = std::time::Instant::now();
    app.tick(t0);
    app.tick(t0 + STAT_DELAY / 2);
    assert!(rx.try_recv().is_err(), "debounced");
    key(&mut app, 'j');
    app.tick(t0 + STAT_DELAY);
    assert!(rx.try_recv().is_err(), "moved: the pause starts again");
    app.tick(t0 + STAT_DELAY * 2);
    let first = rx.recv_timeout(Duration::from_secs(10)).unwrap();
    // Before the answer is fed in, the cursor moves on and a newer
    // request goes out.
    key(&mut app, 'j');
    app.tick(t0 + STAT_DELAY * 3);
    app.tick(t0 + STAT_DELAY * 4);
    let second = rx.recv_timeout(Duration::from_secs(10)).unwrap();
    let (AppEvent::Stat { request: r1, .. }, AppEvent::Stat { request: r2, .. }) =
        (&first, &second)
    else {
        panic!("not stats");
    };
    assert!(r1 < r2);
    app.event(first);
    assert_eq!(app.stat_cached(), 0, "stale answer dropped");
    app.event(second);
    assert_eq!(app.stat_cached(), 1);
    assert!(matches!(
        app.stat_pane(),
        Some(debrief::app::StatPane::Ready(s)) if s.message == "commit 0"
    ));
    // Cached: coming back to it sends nothing.
    key(&mut app, 'k');
    key(&mut app, 'j');
    app.tick(t0 + STAT_DELAY * 5);
    app.tick(t0 + STAT_DELAY * 6);
    assert!(rx.try_recv().is_err());
}

fn type_str(app: &mut App, s: &str) {
    keys(app, s);
}

fn enter(app: &mut App) {
    app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
}

fn esc(app: &mut App) {
    app.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
}

#[test]
fn filter_narrows_marks_only_visible_rows_and_survives_reload() {
    let (tmp, root) = git_repo(12);
    let cache = tmp.path().join("cache");
    seed_oldest(&root, &cache);
    let mut app = open(&root, &cache);
    assert_eq!(app.rows().len(), 12);
    key(&mut app, 'f');
    assert_eq!(app.mode(), Mode::Filter);
    type_str(&mut app, "commit 1");
    // commit 1, 10, 11.
    assert_eq!(app.rows().len(), 3);
    let s = screen(&mut app);
    assert!(s[11].starts_with("filter: commit 1"), "{s:#?}");
    app.event(debrief::app::AppEvent::Paste("0".into()));
    assert_eq!(app.rows().len(), 1);
    app.key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
    enter(&mut app);
    assert_eq!(app.mode(), Mode::Normal);
    let s = screen(&mut app);
    assert!(s[11].contains("filter: commit 1 (3/12)"), "{s:#?}");
    // The match is highlighted.
    let mut t = Terminal::new(TestBackend::new(80, 12)).unwrap();
    t.draw(|f| app.render(f)).unwrap();
    let buf = t.backend().buffer();
    let line: String = (0..80).map(|x| buf[(x, 1)].symbol().to_string()).collect();
    let at = line.find("commit 1").unwrap();
    let x = line[..at].chars().count() as u16;
    assert_eq!(buf[(x, 1)].bg, debrief::theme::MOCHA.yellow);

    // m marks only the filtered rows.
    key(&mut app, 'm');
    assert_eq!(app.status(), "marked 3 commits seen");
    esc(&mut app);
    assert_eq!(app.rows().len(), 12);
    let new = app
        .rows()
        .iter()
        .filter(|r| r.status == Status::New)
        .count();
    assert_eq!(new, 8, "12 - 3 marked - 1 seeded");

    // r keeps the filter, the cursor on the same commit.
    key(&mut app, 'f');
    type_str(&mut app, "commit 1");
    enter(&mut app);
    keys(&mut app, "ggj");
    let id = app.rows()[1].info.commit.clone();
    add_commit(&root, 12);
    key(&mut app, 'r');
    assert_eq!(app.rows().len(), 4, "commit 12 joins");
    assert_eq!(app.rows()[app.cursor()].info.commit, id);
    // Enter and ]] walk the filtered list.
    enter(&mut app);
    keys(&mut app, "]]");
    assert_eq!(
        app.commit_id(),
        Some(app.rows()[app.cursor()].info.commit.as_str())
    );
    assert!(
        app.rows()[app.cursor()]
            .info
            .summary
            .starts_with("commit 1")
    );
    key(&mut app, 'q');
    assert_eq!(app.rows().len(), 4);
}

#[test]
fn log_search_moves_and_reports_misses() {
    let (tmp, root) = git_repo(5);
    let mut app = open(&root, &tmp.path().join("cache"));
    keys(&mut app, "gg/commit 3");
    let s = screen(&mut app);
    assert!(s[11].starts_with("/commit 3"), "{s:#?}");
    enter(&mut app);
    assert_eq!(app.rows()[app.cursor()].info.summary, "commit 3");
    keys(&mut app, "/zzz");
    enter(&mut app);
    assert_eq!(app.status(), "pattern not found: zzz");
    keys(&mut app, "?commit");
    enter(&mut app);
    assert_eq!(app.rows()[app.cursor()].info.summary, "commit 4");
    key(&mut app, 'N');
    assert_eq!(app.rows()[app.cursor()].info.summary, "commit 3");
    keys(&mut app, "gg");
    key(&mut app, 'n');
    assert_eq!(app.rows()[app.cursor()].info.summary, "commit 0");
    assert_eq!(app.status(), "search hit TOP, continuing at BOTTOM");
}

#[test]
fn stats_pane_shows_files_first_and_scrolls_with_j_k() {
    let (tmp, root) = git_repo(1);
    let body: String = (0..30).map(|i| format!("line {i}\n")).collect();
    std::fs::write(root.join("g.txt"), "g\n").unwrap();
    commit(&root, &format!("long subject\n\n{body}"));
    let mut app = open_with_stats(&root, &tmp.path().join("cache"));
    settle(&mut app);
    // 119x20: the stacked pane has six rows; a one-line, one-file commit
    // shows its file row and the summary.
    key(&mut app, 'j');
    settle(&mut app);
    let s = screen_at(&mut app, 119, 20);
    let pane: Vec<&str> = s[13..19].iter().map(|r| r.trim_start()).collect();
    assert!(
        pane[0].contains(&app.rows()[1].info.commit[..12]),
        "{pane:#?}"
    );
    assert!(
        pane[0].contains("t <t@t>") && pane[0].contains(" UTC ("),
        "{pane:#?}"
    );
    assert!(
        pane.iter().any(|r| r.starts_with("f0.txt | 1 +")),
        "{pane:#?}"
    );
    assert!(
        pane.iter().any(|r| r.starts_with("1 file changed")),
        "{pane:#?}"
    );

    // The long message is cut at half the pane with `…`; the file row
    // still shows.
    key(&mut app, 'k');
    settle(&mut app);
    let s = screen_at(&mut app, 119, 20);
    let pane: Vec<&str> = s[13..19].iter().map(|r| r.trim_start()).collect();
    assert!(pane[1].starts_with("long subject"), "{pane:#?}");
    assert!(pane[3].starts_with('…'), "{pane:#?}");
    assert!(pane[5].starts_with("g.txt | 1 +"), "{pane:#?}");
    // J scrolls to the end of the message and no further.
    for _ in 0..100 {
        key(&mut app, 'J');
    }
    let s = screen_at(&mut app, 119, 20);
    let pane: Vec<&str> = s[13..19].iter().map(|r| r.trim_start()).collect();
    assert!(pane[5].starts_with("1 file changed"), "{pane:#?}");
    assert!(pane.iter().any(|r| r.starts_with("line 29")), "{pane:#?}");
    key(&mut app, 'K');
    let s = screen_at(&mut app, 119, 20);
    assert!(
        !s[18].trim_start().starts_with("1 file changed"),
        "K scrolls back up"
    );
    // The wheel scrolls the pane when over it, not over the log.
    key(&mut app, 'j');
    key(&mut app, 'k');
    use crossterm::event::{MouseEvent, MouseEventKind};
    let wheel = |kind, row| {
        debrief::app::AppEvent::Mouse(MouseEvent {
            kind,
            column: 5,
            row,
            modifiers: KeyModifiers::NONE,
        })
    };
    app.event(wheel(MouseEventKind::ScrollDown, 2));
    assert_eq!(app.stat_scroll(), 0, "over the log");
    app.event(wheel(MouseEventKind::ScrollDown, 15));
    assert_eq!(app.stat_scroll(), 3);
    app.event(wheel(MouseEventKind::ScrollUp, 15));
    assert_eq!(app.stat_scroll(), 0);
    app.event(wheel(MouseEventKind::ScrollDown, 15));
    assert_eq!(app.stat_scroll(), 3);
    // Moving to another commit resets the scroll.
    key(&mut app, 'j');
    key(&mut app, 'k');
    let s = screen_at(&mut app, 119, 20);
    assert!(s[14].trim_start().starts_with("long subject"), "{s:#?}");
}

/// The stats pane's rows at `w`x`h`, the border cut off.
fn pane_at(app: &mut App, w: u16, h: u16) -> Vec<String> {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| app.render(f)).unwrap();
    let buf = t.backend().buffer().clone();
    let Some(p) = app
        .log_layout(ratatui::layout::Rect::new(0, 0, w, h - 1))
        .stats
    else {
        return Vec::new();
    };
    (p.y..p.y + p.height)
        .map(|y| {
            (p.x..p.x + p.width)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect::<String>()
                .trim_matches(|c: char| c == ' ' || c == '│' || c == '─')
                .to_string()
        })
        .collect()
}

#[test]
fn stats_pane_message_is_whole_when_it_fits_and_every_line_is_reachable() {
    let (tmp, root) = git_repo(0);
    // Message k has k lines (subject, blank, k - 2 body lines) and one file.
    let lens: Vec<usize> = (2..=13).collect();
    for &n in &lens {
        std::fs::write(root.join(format!("m{n}.txt")), "x\n").unwrap();
        let body: String = (1..n - 1).map(|i| format!("m{n} line {i}\n")).collect();
        commit(&root, &format!("m{n} subj\n\n{body}"));
    }
    let mut app = open_with_stats(&root, &tmp.path().join("cache"));
    settle(&mut app);

    // The repro: 119x27 (the 119x26 of old plus the branch chip's row), a
    // five-line message and one file fit, so all of it shows, uncut.
    let at = |app: &mut App, n: usize| {
        key(app, 'g');
        key(app, 'g');
        for _ in 0..(13 - n) {
            key(app, 'j');
        }
        settle(app);
    };
    at(&mut app, 5);
    let pane = pane_at(&mut app, 119, 27);
    for want in [
        "m5 subj",
        "m5 line 1",
        "m5 line 2",
        "m5 line 3",
        "m5.txt | 1 +",
    ] {
        assert!(
            pane.iter().any(|r| r.starts_with(want)),
            "{want}: {pane:#?}"
        );
    }
    assert!(!pane.iter().any(|r| r.starts_with('…')), "{pane:#?}");

    // The sweep: every message line and the file row show at some J count.
    let mut failing = Vec::new();
    for w in [119u16, 160] {
        for h in 12u16..=39 {
            for &n in &lens {
                at(&mut app, n);
                let mut want: Vec<String> = vec![format!("m{n} subj"), format!("m{n}.txt | 1 +")];
                want.extend((1..n - 1).map(|i| format!("m{n} line {i}")));
                loop {
                    let pane = pane_at(&mut app, w, h);
                    want.retain(|l| !pane.iter().any(|r| r.starts_with(l.as_str())));
                    let before = app.stat_scroll();
                    key(&mut app, 'J');
                    if app.stat_scroll() == before {
                        break;
                    }
                }
                if !want.is_empty() {
                    failing.push(format!("{w}x{h} n={n}: {want:?}"));
                }
            }
        }
    }
    assert!(
        failing.is_empty(),
        "{} failing: {failing:#?}",
        failing.len()
    );
}

#[test]
fn w_sets_the_watermark_and_the_next_start_lands_above_it() {
    let (tmp, root) = git_repo(5);
    let cache = tmp.path().join("cache");
    let mut app = open(&root, &cache);
    assert_eq!(markers(&app), [Status::Seen; 5]);
    // Rows: commit 4 .. commit 0. The watermark on commit 2.
    keys(&mut app, "3G");
    key(&mut app, 'w');
    use Status::{New, Seen};
    assert_eq!(markers(&app), [New, New, Seen, Seen, Seen]);
    assert!(app.status().contains("2 newer unseen"), "{}", app.status());
    drop(app);
    let mut app = open(&root, &cache);
    assert_eq!(markers(&app), [New, New, Seen, Seen, Seen]);
    assert_eq!(app.cursor(), 1, "just above the watermark");
    // Lower it: commit 0 (the oldest). Higher rows are new again.
    key(&mut app, 'G');
    key(&mut app, 'w');
    assert_eq!(markers(&app), [New, New, New, New, Seen]);
    // Raise it to the top: everything seen.
    keys(&mut app, "gg");
    key(&mut app, 'w');
    assert_eq!(markers(&app), [Seen; 5]);
}

#[test]
fn w_under_a_filter_counts_the_whole_log() {
    let (tmp, root) = git_repo(5);
    let cache = tmp.path().join("cache");
    let mut app = open(&root, &cache);
    // Filter to commit 2 alone; the watermark there makes 3 and 4 new.
    key(&mut app, 'f');
    keys(&mut app, "commit 2");
    app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.rows().len(), 1);
    key(&mut app, 'w');
    app.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert_eq!(app.rows().len(), 5);
    use Status::{New, Seen};
    assert_eq!(markers(&app), [New, New, Seen, Seen, Seen]);
}
