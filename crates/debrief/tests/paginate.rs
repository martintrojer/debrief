//! Paging through long histories: the log loads `[log] limit` commits,
//! and a move past the last loaded row (`j`, `G`, `C-d`, the wheel, in
//! the log and in the commit list) loads the next page, off the UI thread
//! when there is an event sender. A `… more` row ends both lists while
//! more may exist; reloads keep the grown count; a filter or search miss
//! says more is not loaded and loads nothing.

mod common;

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use common::{JjEnv, commit, ctrl, git, have_jj, keys, press};
use crossterm::event::{KeyCode, KeyModifiers, MouseEvent, MouseEventKind};
use debrief::app::config::Config;
use debrief::app::{App, AppEvent, Focus, Screen, StartOptions, short_id};
use debrief::vcs;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;

const MORE: &str = "… more";
const NOT_LOADED: &str = "… more not loaded";

/// A git repo with `commit 1` .. `commit {n}` (row 0 is `commit {n}`).
fn git_repo(n: usize) -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir(&root).unwrap();
    git(&root, &["init", "-q"]);
    for i in 1..=n {
        add_commit(&root, i);
    }
    let root = root.canonicalize().unwrap();
    (tmp, root)
}

fn add_commit(root: &Path, i: usize) {
    std::fs::write(root.join(format!("f{i}.txt")), format!("{i}\n")).unwrap();
    commit(root, &format!("commit {i}"));
}

/// The app on `root` listing `limit` commits a page, the stats pane off.
fn open(root: &Path, cache: &Path, limit: usize, toml: &str) -> App {
    let mut opts = StartOptions::new(cache.to_path_buf());
    opts.config = Config::parse(toml).unwrap();
    opts.config.view.stats = false;
    opts.limit = limit;
    App::new(vcs::open(root, vcs::OpenOptions::default()).unwrap(), opts).unwrap()
}

/// 12 commits, 5 a page.
fn twelve() -> (tempfile::TempDir, PathBuf, App) {
    let (tmp, root) = git_repo(12);
    let app = open(&root, &tmp.path().join("cache"), 5, "");
    (tmp, root, app)
}

fn summaries(app: &App) -> Vec<String> {
    app.rows().iter().map(|r| r.info.summary.clone()).collect()
}

fn cursor_summary(app: &App) -> String {
    app.rows()[app.cursor()].info.summary.clone()
}

fn screen(app: &mut App, w: u16, h: u16) -> Vec<String> {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
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

fn wheel_down(app: &mut App, col: u16, row: u16) {
    let ev = MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: col,
        row,
        modifiers: KeyModifiers::NONE,
    };
    app.mouse(ev, Instant::now());
}

/// The next page reply from the loader thread.
fn page_event(rx: &mpsc::Receiver<AppEvent>) -> AppEvent {
    loop {
        let ev = rx.recv_timeout(Duration::from_secs(20)).unwrap();
        if matches!(ev, AppEvent::LogPage { .. }) {
            return ev;
        }
    }
}

// --- the log -----------------------------------------------------------

#[test]
fn a_full_page_ends_with_a_more_row_and_j_past_it_loads_the_next() {
    let (_t, _root, mut app) = twelve();
    assert_eq!(app.rows().len(), 5);
    assert!(app.log_more());
    let s = screen(&mut app, 80, 12);
    assert!(s[4].contains("commit 8"), "{s:#?}");
    assert_eq!(s[5].trim(), MORE, "{s:#?}");
    // Onto the last row: nothing loads.
    keys(&mut app, "4j");
    assert_eq!((app.rows().len(), app.cursor()), (5, 4));
    // Past it: the next page, the cursor on the next new row.
    keys(&mut app, "j");
    assert_eq!(app.rows().len(), 10);
    assert_eq!(cursor_summary(&app), "commit 7");
    assert!(app.log_more());
    // A count past the end: the rest, and the more row goes.
    keys(&mut app, "7j");
    assert_eq!(app.rows().len(), 12);
    assert!(!app.log_more());
    assert_eq!(cursor_summary(&app), "commit 1", "clamped to the last row");
    let s = screen(&mut app, 80, 14);
    assert!(!s.iter().any(|l| l.contains(MORE)), "{s:#?}");
    // Nothing more: j at the end stays.
    keys(&mut app, "j");
    assert_eq!(app.rows().len(), 12);
}

#[test]
fn the_cursor_keeps_its_commit_when_the_page_shifts_the_rows() {
    let (_t, root, mut app) = twelve();
    keys(&mut app, "4j");
    assert_eq!(cursor_summary(&app), "commit 8");
    // A new commit lands on top before the page loads.
    add_commit(&root, 13);
    keys(&mut app, "j");
    assert_eq!(summaries(&app)[0], "commit 13");
    assert_eq!(cursor_summary(&app), "commit 7", "one past commit 8");
}

#[test]
fn uncounted_g_loads_counted_g_does_not() {
    let (_t, _root, mut app) = twelve();
    keys(&mut app, "20G");
    assert_eq!((app.rows().len(), app.cursor()), (5, 4), "a row number");
    keys(&mut app, "gg");
    keys(&mut app, "G");
    assert_eq!((app.rows().len(), app.cursor()), (10, 9));
    keys(&mut app, "G");
    assert_eq!((app.rows().len(), app.cursor()), (12, 11));
    assert!(!app.log_more());
}

#[test]
fn c_d_past_the_end_loads() {
    let (_t, _root, mut app) = twelve();
    screen(&mut app, 80, 12);
    ctrl(&mut app, 'd');
    assert_eq!(app.rows().len(), 10);
    assert_eq!(app.cursor(), 5, "half of the 11-row log from row 0");
}

#[test]
fn the_wheel_loads_once_the_more_row_is_on_screen() {
    let (_t, _root, mut app) = twelve();
    // Five log rows: the commits fill the log, the more row below.
    let s = screen(&mut app, 80, 6);
    assert!(!s.iter().any(|l| l.contains(MORE)), "{s:#?}");
    wheel_down(&mut app, 5, 2);
    assert_eq!(app.rows().len(), 5, "scrolled to the more row");
    let s = screen(&mut app, 80, 6);
    assert_eq!(s[4].trim(), MORE, "{s:#?}");
    wheel_down(&mut app, 5, 2);
    assert_eq!(app.rows().len(), 10);
}

#[test]
fn the_more_row_shows_when_the_cursor_is_on_the_last_commit() {
    let (_t, _root, mut app) = twelve();
    keys(&mut app, "4j");
    let s = screen(&mut app, 80, 6);
    assert!(s[3].contains("commit 8"), "{s:#?}");
    assert_eq!(s[4].trim(), MORE, "{s:#?}");
}

#[test]
fn a_reload_keeps_the_grown_count() {
    let (_t, root, mut app) = twelve();
    keys(&mut app, "G");
    assert_eq!(app.rows().len(), 10);
    add_commit(&root, 13);
    app.event(AppEvent::Reload);
    assert_eq!(app.rows().len(), 10);
    assert_eq!(summaries(&app)[0], "commit 13");
    assert!(app.log_more());
}

// --- off the UI thread -------------------------------------------------

#[test]
fn with_a_sender_the_page_loads_on_a_worker_once() {
    let (_t, _root, mut app) = twelve();
    let (tx, rx) = mpsc::channel();
    app.set_sender(tx);
    keys(&mut app, "4j");
    keys(&mut app, "j");
    assert_eq!(app.rows().len(), 5, "the UI thread did not load it");
    assert!(app.loading_more());
    let s = screen(&mut app, 200, 12);
    assert!(s[11].contains("loading more…"), "{s:#?}");
    // Again while loading: no second load.
    keys(&mut app, "j");
    let ev = page_event(&rx);
    assert!(
        rx.recv_timeout(Duration::from_secs(1)).is_err(),
        "one load in flight"
    );
    // The cursor moves while it loads: it keeps that commit, and the
    // move that asked for the page is not replayed.
    keys(&mut app, "k");
    assert_eq!(cursor_summary(&app), "commit 9");
    app.event(ev);
    assert!(!app.loading_more());
    assert_eq!(app.rows().len(), 10);
    assert_eq!(cursor_summary(&app), "commit 9");
    let s = screen(&mut app, 200, 12);
    assert!(!s[11].contains("loading more…"), "{s:#?}");
}

#[test]
fn the_reply_moves_the_cursor_when_it_stayed_put() {
    let (_t, _root, mut app) = twelve();
    let (tx, rx) = mpsc::channel();
    app.set_sender(tx);
    keys(&mut app, "4j");
    keys(&mut app, "j");
    app.event(page_event(&rx));
    assert_eq!(cursor_summary(&app), "commit 7");
}

#[test]
fn a_reload_during_a_page_load_takes_its_count_and_drops_its_reply() {
    let (_t, root, mut app) = twelve();
    let (tx, rx) = mpsc::channel();
    app.set_sender(tx);
    keys(&mut app, "G");
    let stale = page_event(&rx);
    add_commit(&root, 13);
    app.event(AppEvent::Reload);
    assert!(!app.loading_more());
    assert_eq!(app.rows().len(), 10, "the reload used the larger count");
    assert_eq!(summaries(&app)[0], "commit 13");
    app.event(stale);
    assert_eq!(summaries(&app)[0], "commit 13", "the stale reply dropped");
    assert_eq!(app.rows().len(), 10);
}

// --- search and filter -------------------------------------------------

#[test]
fn a_filter_miss_says_more_is_not_loaded_and_loads_nothing() {
    let (_t, _root, mut app) = twelve();
    keys(&mut app, "fcommit 2");
    assert_eq!(app.rows().len(), 0);
    let s = screen(&mut app, 80, 12);
    assert!(s[11].starts_with("filter: commit 2"), "{s:#?}");
    assert!(s[11].contains(NOT_LOADED), "while typing: {s:#?}");
    press(&mut app, KeyCode::Enter);
    let s = screen(&mut app, 80, 12);
    assert!(s[11].contains(NOT_LOADED), "{s:#?}");
    assert_eq!(app.filter_counts(), (0, 5), "nothing loaded");
    // A hit says nothing.
    press(&mut app, KeyCode::Esc);
    keys(&mut app, "fcommit 9");
    let s = screen(&mut app, 80, 12);
    assert!(!s[11].contains(NOT_LOADED), "{s:#?}");
}

#[test]
fn a_search_miss_says_more_is_not_loaded_and_loads_nothing() {
    let (_t, _root, mut app) = twelve();
    keys(&mut app, "/commit 2");
    let s = screen(&mut app, 80, 12);
    assert!(s[11].starts_with("/commit 2"), "{s:#?}");
    assert!(s[11].contains(NOT_LOADED), "while typing: {s:#?}");
    press(&mut app, KeyCode::Enter);
    assert!(app.status().contains(NOT_LOADED), "{}", app.status());
    assert_eq!(app.rows().len(), 5);
    keys(&mut app, "n");
    assert!(app.status().contains(NOT_LOADED), "{}", app.status());
    assert_eq!(app.rows().len(), 5);
    // Everything loaded: a plain miss.
    keys(&mut app, "GG");
    keys(&mut app, "/zzz");
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.status(), "pattern not found: zzz");
}

// --- the count contract ------------------------------------------------

#[test]
fn git_counts_commits_not_its_working_copy_row() {
    let (tmp, root) = git_repo(4);
    std::fs::write(root.join("f1.txt"), "dirty\n").unwrap();
    let app = open(&root, &tmp.path().join("cache"), 5, "");
    assert_eq!(app.rows().len(), 5, "the working copy and four commits");
    assert!(!app.log_more(), "four commits asked five");
    let app = open(&root, &tmp.path().join("cache"), 4, "");
    assert_eq!(app.rows().len(), 5);
    assert!(app.log_more(), "four commits asked four");
}

#[test]
fn jj_counts_the_root_commit_it_drops() {
    if !have_jj() {
        return;
    }
    let e = JjEnv::new(false);
    for i in 1..=3 {
        e.write(&format!("f{i}.txt"), "x\n");
        e.jj(&["--quiet", "commit", "-m", &format!("commit {i}")]);
    }
    // ::@ is @, three commits and the root: five, asked five.
    let cache = e.root.parent().unwrap().join("cache");
    let mut app = open(&e.root, &cache, 5, "");
    assert_eq!(app.rows().len(), 4, "the root is not listed");
    assert!(app.log_more(), "jj returned as many as asked");
    keys(&mut app, "G");
    assert_eq!(app.rows().len(), 4);
    assert!(!app.log_more());
}

// --- the commit list ---------------------------------------------------

const LIST: &str = "[view]\nsidebar_commits = true\n";
const COLS: u16 = 120;
const ROWS: u16 = 30;

/// 12 commits, 5 a page, commit 12 open and the commit list focused.
fn list_app(limit: usize, n: usize) -> (tempfile::TempDir, App) {
    let (tmp, root) = git_repo(n);
    let mut app = open(&root, &tmp.path().join("cache"), limit, LIST);
    app.event(AppEvent::Resize(COLS, ROWS));
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.screen(), Screen::Commit(0));
    ctrl(&mut app, 'h');
    ctrl(&mut app, 'j');
    assert_eq!(app.focus(), Focus::Commits, "{}", app.status());
    (tmp, app)
}

fn list_cursor_summary(app: &App) -> String {
    app.rows()[app.commit_list_cursor()].info.summary.clone()
}

/// The commit list's rows on screen, under its rule.
fn list_lines(app: &mut App) -> Vec<String> {
    let c = app
        .commit_layout(Rect::new(0, 0, COLS, ROWS - 1))
        .commits
        .unwrap();
    let s = screen(app, COLS, ROWS);
    (c.y + 1..c.y + c.height)
        .map(|y| {
            s[y as usize]
                .chars()
                .skip(c.x as usize)
                .take(c.width as usize - 1)
                .collect::<String>()
        })
        .collect()
}

#[test]
fn the_commit_list_ends_with_the_more_row_and_j_past_it_loads() {
    let (_t, mut app) = list_app(5, 12);
    let l = list_lines(&mut app);
    assert!(l[4].contains(short_id(&app.rows()[4].info)), "{l:#?}");
    assert_eq!(l[5].trim(), MORE, "{l:#?}");
    keys(&mut app, "4j");
    assert_eq!(app.rows().len(), 5);
    keys(&mut app, "j");
    assert_eq!(app.rows().len(), 10);
    assert_eq!(list_cursor_summary(&app), "commit 7");
    assert_eq!(app.screen(), Screen::Commit(0), "the open commit stays");
    assert_eq!(app.cursor(), 0);
}

#[test]
fn the_commit_list_g_and_c_d_load() {
    let (_t, mut app) = list_app(5, 12);
    keys(&mut app, "G");
    assert_eq!(app.rows().len(), 10);
    assert_eq!(app.commit_list_cursor(), 9);
    let (_t, mut app) = list_app(5, 12);
    ctrl(&mut app, 'd');
    assert_eq!(app.rows().len(), 10);
    assert_eq!(app.commit_list_cursor(), 6, "half of 12 rows");
}

#[test]
fn the_commit_list_wheel_loads_at_its_end() {
    let (_t, mut app) = list_app(5, 12);
    let c = app
        .commit_layout(Rect::new(0, 0, COLS, ROWS - 1))
        .commits
        .unwrap();
    // Six rows in a 12-row list: already at its end.
    wheel_down(&mut app, c.x + 2, c.y + 3);
    assert_eq!(app.rows().len(), 10);
    assert_eq!(app.screen(), Screen::Commit(0));
}

#[test]
fn the_commit_list_shows_the_more_row_when_its_commits_fill_it() {
    // 12 commits fill the 12-row list.
    let (_t, mut app) = list_app(12, 20);
    let l = list_lines(&mut app);
    assert!(!l.iter().any(|l| l.contains(MORE)), "{l:#?}");
    keys(&mut app, "11j");
    assert_eq!(app.rows().len(), 12);
    let l = list_lines(&mut app);
    assert!(l[10].contains(short_id(&app.rows()[11].info)), "{l:#?}");
    assert_eq!(l[11].trim(), MORE, "{l:#?}");
}
