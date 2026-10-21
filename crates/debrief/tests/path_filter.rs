//! The log's path filter: `debrief PATH` below the root, the `F` prompt,
//! `<leader>F`, and the backends' `log_in`.

mod common;

use std::path::{Path, PathBuf};

use clap::Parser;
use common::{JjEnv, commit, ctrl, git_repo, have_jj, keys, press, w};
use crossterm::event::KeyCode;
use debrief::app::{App, AppEvent, Mode, Screen, StartOptions};
use debrief::cli::Cli;
use debrief::vcs::{self, OpenOptions};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

/// m0: a/x.txt, m1: b.txt, m2: a/sub/y.txt, m3: a/x.txt and b.txt,
/// m4: `a/q"*.txt`.
fn repo() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    // On `main` whatever init.defaultBranch says: the screens name it.
    git_repo(
        &root,
        &[
            ("m0", vec![w("a/x.txt", "0\n")]),
            ("m1", vec![w("b.txt", "1\n")]),
            ("m2", vec![w("a/sub/y.txt", "2\n")]),
            ("m3", vec![w("a/x.txt", "3\n"), w("b.txt", "3\n")]),
            ("m4", vec![w("a/q\"*.txt", "4\n")]),
        ],
    );
    let root = root.canonicalize().unwrap();
    (tmp, root)
}

fn summaries(v: &[vcs::CommitInfo]) -> Vec<&str> {
    v.iter().map(|c| c.summary.as_str()).collect()
}

fn rows(app: &App) -> Vec<String> {
    app.rows().iter().map(|r| r.info.summary.clone()).collect()
}

fn open(root: &Path, cache: &Path, path: Option<&str>) -> App {
    let mut opts = StartOptions::new(cache.to_path_buf());
    opts.config.view.stats = false;
    opts.path = path.map(str::to_string);
    App::new(vcs::open(root, OpenOptions::default()).unwrap(), opts).unwrap()
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

#[test]
fn git_log_in_a_file_a_directory_and_odd_names() {
    let (_t, root) = repo();
    let v = vcs::open(&root, OpenOptions::default()).unwrap();
    let log = |p| v.log_in(None, Some(p), 10).unwrap();
    assert_eq!(summaries(&log("a/x.txt")), ["m3", "m0"]);
    assert_eq!(summaries(&log("a")), ["m4", "m3", "m2", "m0"]);
    assert_eq!(summaries(&log("a/sub")), ["m2"]);
    // Glob characters are literal.
    assert_eq!(summaries(&log("a/q\"*.txt")), ["m4"]);
    assert_eq!(summaries(&log("a/*")), Vec::<&str>::new());
    assert!(log("nope").is_empty());
    // The limit counts matching commits.
    assert_eq!(
        summaries(&v.log_in(None, Some("a"), 2).unwrap()),
        ["m4", "m3"]
    );
    // Deleted files keep their history.
    std::fs::remove_file(root.join("b.txt")).unwrap();
    commit(&root, "rm b");
    assert_eq!(summaries(&log("b.txt")), ["rm b", "m3", "m1"]);
}

#[test]
fn git_working_row_shows_only_when_the_path_is_dirty() {
    let (_t, root) = repo();
    let v = vcs::open(&root, OpenOptions::default()).unwrap();
    std::fs::write(root.join("b.txt"), "dirty\n").unwrap();
    assert!(v.working_status_in(Some("b.txt")).unwrap().is_some());
    assert!(v.working_status_in(Some("a")).unwrap().is_none());
    std::fs::write(root.join("a/sub/new.txt"), "u\n").unwrap();
    let w = v.working_status_in(Some("a")).unwrap().unwrap();
    assert_eq!(w.files, 2, "the whole tree's count");
    assert!(
        v.working_status_in(Some("a/s")).unwrap().is_none(),
        "not a prefix match"
    );
}

#[test]
fn jj_log_in_a_file_and_a_directory() {
    if !have_jj() {
        return;
    }
    let e = JjEnv::new(false);
    std::fs::create_dir(e.root.join("a")).unwrap();
    e.write("a/x.txt", "0\n");
    e.jj(&["--quiet", "commit", "-m", "m0"]);
    e.write("b.txt", "1\n");
    e.jj(&["--quiet", "commit", "-m", "m1"]);
    e.write("a/q\"y.txt", "2\n");
    e.jj(&["--quiet", "commit", "-m", "m2"]);
    let v = vcs::open(&e.root, OpenOptions { snapshot: false }).unwrap();
    let log = |p| v.log_in(Some("::@ ~ root()"), Some(p), 10).unwrap();
    assert_eq!(summaries(&log("a")), ["m2", "m0"]);
    assert_eq!(summaries(&log("b.txt")), ["m1"]);
    assert_eq!(summaries(&log("a/q\"y.txt")), ["m2"]);
    assert!(log("nope").is_empty());
    // The limit counts matching commits.
    let one = v.log_in(Some("::@ ~ root()"), Some("a"), 1).unwrap();
    assert_eq!(summaries(&one), ["m2"]);
    // Deleted files keep their history.
    std::fs::remove_file(e.root.join("b.txt")).unwrap();
    e.jj(&["--quiet", "commit", "-m", "rm b"]);
    let v = vcs::open(&e.root, OpenOptions { snapshot: false }).unwrap();
    let log = v.log_in(Some("::@ ~ root()"), Some("b.txt"), 10).unwrap();
    assert_eq!(summaries(&log), ["rm b", "m1"]);
}

#[test]
fn cli_path_below_the_root_sets_the_filter() {
    let (_t, root) = repo();
    let filter = |arg: &Path| {
        let cli = Cli::try_parse_from([Path::new("debrief"), arg]).unwrap();
        let v = vcs::open(&cli.path(), OpenOptions::default()).unwrap();
        cli.path_filter(v.root()).unwrap()
    };
    assert_eq!(filter(&root), None);
    assert_eq!(filter(&root.join(".")), None);
    assert_eq!(filter(&root.join("a")), Some("a".into()));
    assert_eq!(filter(&root.join("a/sub/")), Some("a/sub".into()));
    assert_eq!(filter(&root.join("a/x.txt")), Some("a/x.txt".into()));
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        // A link inside the repo names itself.
        symlink(root.join("b.txt"), root.join("link")).unwrap();
        assert_eq!(filter(&root.join("link")), Some("link".into()));
        // A link outside it names its target.
        let out = root.parent().unwrap();
        symlink(&root, out.join("to-root")).unwrap();
        symlink(root.join("a/sub"), out.join("to-sub")).unwrap();
        assert_eq!(filter(&out.join("to-root")), None);
        assert_eq!(filter(&out.join("to-sub")), Some("a/sub".into()));
    }
    let cli = Cli::try_parse_from(["debrief"]).unwrap();
    assert_eq!(cli.path_filter(&root).unwrap(), None);
}

#[test]
fn the_log_loads_only_matching_commits_and_shows_the_path() {
    let (tmp, root) = repo();
    let mut app = open(&root, &tmp.path().join("cache"), Some("a/x.txt"));
    assert_eq!(rows(&app), ["m3", "m0"]);
    let s = screen(&mut app, 80, 8);
    assert_eq!(s[0], " ⎇ main  ⌕ a/x.txt", "{s:#?}");
    assert!(s[1].ends_with("m3"), "{s:#?}");
    assert!(s[7].contains("branch: main  path: a/x.txt"), "{}", s[7]);
    // The text filter narrows what the path loaded; both show.
    keys(&mut app, "fm3");
    press(&mut app, KeyCode::Enter);
    assert_eq!(rows(&app), ["m3"]);
    let s = screen(&mut app, 100, 8);
    assert!(
        s[7].contains("path: a/x.txt  0 new  filter: m3 (1/2)"),
        "{}",
        s[7]
    );
}

#[test]
fn f_edits_applies_cancels_and_removes_the_path() {
    let (tmp, root) = repo();
    let mut app = open(&root, &tmp.path().join("cache"), None);
    assert_eq!(rows(&app).len(), 5);
    keys(&mut app, "Fa/sub/");
    assert_eq!(app.mode(), Mode::PathFilter);
    assert_eq!(app.search_prompt().as_deref(), Some("path: a/sub/"));
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.mode(), Mode::Normal);
    assert_eq!(app.path_filter(), Some("a/sub"), "normalized");
    assert_eq!(rows(&app), ["m2"]);
    // Esc keeps the old path.
    keys(&mut app, "F");
    assert_eq!(app.search_prompt().as_deref(), Some("path: a/sub"));
    press(&mut app, KeyCode::Backspace);
    keys(&mut app, "zzz");
    press(&mut app, KeyCode::Esc);
    assert_eq!(app.path_filter(), Some("a/sub"));
    assert_eq!(rows(&app), ["m2"]);
    // Edit to a parent directory.
    keys(&mut app, "F");
    for _ in 0..4 {
        press(&mut app, KeyCode::Backspace);
    }
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.path_filter(), Some("a"));
    assert_eq!(rows(&app), ["m4", "m3", "m2", "m0"]);
    // C-u and an empty Enter remove it.
    keys(&mut app, "F");
    ctrl(&mut app, 'u');
    assert_eq!(app.search_prompt().as_deref(), Some("path: "));
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.path_filter(), None);
    assert_eq!(rows(&app).len(), 5);
    let s = screen(&mut app, 80, 8);
    assert!(!s.iter().any(|l| l.contains('⌕')), "{s:#?}");
}

#[test]
fn leader_f_clears_both_filters() {
    let (tmp, root) = repo();
    let mut app = open(&root, &tmp.path().join("cache"), Some("a"));
    keys(&mut app, "fm3");
    press(&mut app, KeyCode::Enter);
    assert_eq!(rows(&app), ["m3"]);
    keys(&mut app, " F");
    assert_eq!(app.path_filter(), None);
    assert_eq!(rows(&app).len(), 5);
    keys(&mut app, " F");
    assert_eq!(app.status(), "no filter");
}

#[test]
fn paging_counts_matching_commits() {
    let (tmp, root) = repo();
    let mut opts = StartOptions::new(tmp.path().join("cache"));
    opts.limit = 2;
    opts.path = Some("a".into());
    let mut app = App::new(vcs::open(&root, OpenOptions::default()).unwrap(), opts).unwrap();
    assert_eq!(rows(&app), ["m4", "m3"]);
    assert!(app.log_more());
    keys(&mut app, "G");
    assert_eq!(rows(&app), ["m4", "m3", "m2", "m0"]);
    // A new path starts from the first page again.
    keys(&mut app, "F");
    ctrl(&mut app, 'u');
    keys(&mut app, "b.txt");
    press(&mut app, KeyCode::Enter);
    assert_eq!(rows(&app), ["m3", "m1"]);
}

#[test]
fn a_filtered_first_run_marks_the_whole_log_seen() {
    let (tmp, root) = repo();
    let cache = tmp.path().join("cache");
    drop(open(&root, &cache, Some("b.txt")));
    let app = open(&root, &cache, None);
    assert_eq!(app.counts(), (0, 0), "nothing new without the filter");
}

#[test]
fn the_working_row_follows_the_path() {
    let (tmp, root) = repo();
    std::fs::write(root.join("b.txt"), "dirty\n").unwrap();
    let mut app = open(&root, &tmp.path().join("cache"), Some("a"));
    assert!(app.working_copy().is_none());
    assert!(!app.rows()[0].working);
    keys(&mut app, "F");
    ctrl(&mut app, 'u');
    keys(&mut app, "b.txt");
    press(&mut app, KeyCode::Enter);
    assert!(app.rows()[0].working, "{:?}", rows(&app));
}

#[test]
fn a_commit_opens_on_the_first_file_under_the_path() {
    let (tmp, root) = repo();
    // A sidebar wide enough for the rule's branch and path chips.
    let mut opts = StartOptions::new(tmp.path().join("cache"));
    opts.config.view.stats = false;
    opts.config.view.sidebar_width = debrief::app::config::SidebarWidth::Fixed(30);
    opts.path = Some("b.txt".into());
    let mut app = App::new(vcs::open(&root, OpenOptions::default()).unwrap(), opts).unwrap();
    assert_eq!(rows(&app), ["m3", "m1"]);
    press(&mut app, KeyCode::Enter);
    assert!(matches!(app.screen(), Screen::Commit(0)));
    let v = app.commit_rows();
    let row = v
        .iter()
        .position(|r| r.text.contains("b.txt"))
        .expect("b.txt's header");
    assert_eq!(app.commit_cursor().row, row);
    // The commit list's rule shows the path too.
    keys(&mut app, " E");
    let s = screen(&mut app, 120, 30);
    assert!(
        s.iter().any(|l| l.contains("─ ⎇ main  ⌕ b.txt ─")),
        "{s:#?}"
    );
    assert!(s[29].contains("branch: main  path: b.txt"), "{}", s[29]);
}

/// Enter a path in the `F` prompt, replacing what it holds.
fn set_path(app: &mut App, path: &str) {
    keys(app, "F");
    ctrl(app, 'u');
    keys(app, path);
    press(app, KeyCode::Enter);
}

/// From the open commit's diff to the commit list (shown).
fn to_commit_list(app: &mut App) {
    keys(app, " E");
    ctrl(app, 'h');
    ctrl(app, 'j');
}

#[test]
fn a_path_change_puts_the_cursor_on_its_commit_or_the_first_row() {
    let (tmp, root) = repo();
    let mut app = open(&root, &tmp.path().join("cache"), None);
    keys(&mut app, "jj");
    assert_eq!(rows(&app)[app.cursor()], "m2");
    set_path(&mut app, "a");
    assert_eq!(rows(&app)[app.cursor()], "m2", "still listed");
    set_path(&mut app, "b.txt");
    assert_eq!(rows(&app), ["m3", "m1"]);
    assert_eq!(app.cursor(), 0, "m2 is gone: the first row");
}

#[test]
fn bad_paths_are_refused_and_keep_the_old_filter() {
    let (tmp, root) = repo();
    let mut app = open(&root, &tmp.path().join("cache"), Some("a/sub"));
    for typed in ["a/../b.txt", "/etc"] {
        set_path(&mut app, typed);
        assert_eq!(app.path_filter(), Some("a/sub"), "{typed}");
        assert_eq!(rows(&app), ["m2"], "{typed}");
        assert!(!app.status().is_empty(), "{typed}: says why");
    }
    // An absolute path inside the repo is made relative.
    keys(&mut app, "F");
    ctrl(&mut app, 'u');
    app.paste(root.join("b.txt").to_str().unwrap());
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.path_filter(), Some("b.txt"));
    assert_eq!(rows(&app), ["m3", "m1"]);
}

#[test]
fn a_failed_query_keeps_the_old_path_and_rows() {
    let (tmp, root) = repo();
    let mut opts = StartOptions::new(tmp.path().join("cache"));
    // A named revision: without a repo the log fails (the default log of
    // a repo with no HEAD is empty, not an error).
    opts.revset = Some("HEAD".into());
    opts.path = Some("b.txt".into());
    let mut app = App::new(vcs::open(&root, OpenOptions::default()).unwrap(), opts).unwrap();
    // Every read fails from here.
    std::fs::rename(root.join(".git"), root.join(".git-gone")).unwrap();
    set_path(&mut app, "a");
    assert_eq!(app.path_filter(), Some("b.txt"));
    assert_eq!(rows(&app), ["m3", "m1"]);
    assert!(
        app.status().starts_with("path filter not applied"),
        "{}",
        app.status()
    );
}

#[test]
fn paste_types_into_the_path_prompt() {
    let (tmp, root) = repo();
    let mut app = open(&root, &tmp.path().join("cache"), None);
    keys(&mut app, "F");
    app.paste("a/\nsub");
    assert_eq!(app.search_prompt().as_deref(), Some("path: a/ sub"));
    ctrl(&mut app, 'u');
    app.paste("a/sub");
    press(&mut app, KeyCode::Enter);
    assert_eq!(rows(&app), ["m2"]);
}

#[test]
fn a_filtered_first_run_shows_nothing_new_past_the_unfiltered_page() {
    let (tmp, root) = repo();
    let mut opts = StartOptions::new(tmp.path().join("cache"));
    opts.limit = 2;
    opts.path = Some("b.txt".into());
    let app = App::new(vcs::open(&root, OpenOptions::default()).unwrap(), opts).unwrap();
    assert_eq!(rows(&app), ["m3", "m1"]);
    assert_eq!(app.counts(), (0, 0));
}

#[test]
fn reloading_the_working_copy_view_keeps_a_row_the_path_hides_hidden() {
    let (tmp, root) = repo();
    std::fs::write(root.join("a/x.txt"), "dirty\n").unwrap();
    let mut app = open(&root, &tmp.path().join("cache"), None);
    assert!(app.rows()[0].working);
    press(&mut app, KeyCode::Enter);
    assert!(app.viewing_working_copy());
    to_commit_list(&mut app);
    set_path(&mut app, "b.txt");
    assert!(!app.rows().iter().any(|r| r.working), "{:?}", rows(&app));
    // `T` and `r` re-read the working copy's diff: the row stays hidden.
    press(&mut app, KeyCode::Esc);
    keys(&mut app, "T");
    assert!(!app.rows().iter().any(|r| r.working), "T: {:?}", rows(&app));
    keys(&mut app, "r");
    assert!(!app.rows().iter().any(|r| r.working), "r: {:?}", rows(&app));
}

#[test]
fn a_working_copy_diff_in_flight_keeps_a_row_the_path_hides_hidden() {
    let (tmp, root) = repo();
    std::fs::write(root.join("a/x.txt"), "dirty\n").unwrap();
    let mut app = open(&root, &tmp.path().join("cache"), None);
    let (tx, rx) = std::sync::mpsc::channel();
    app.set_sender(tx);
    press(&mut app, KeyCode::Enter);
    to_commit_list(&mut app);
    set_path(&mut app, "b.txt");
    // Every answer, the working copy's diff first, as the race had it.
    let mut evs: Vec<_> = (0..2)
        .map(|_| rx.recv_timeout(std::time::Duration::from_secs(20)).unwrap())
        .collect();
    assert!(
        evs.iter()
            .any(|e| matches!(e, AppEvent::WorkingLoaded { .. })),
        "the diff was in flight"
    );
    evs.sort_by_key(|e| !matches!(e, AppEvent::WorkingLoaded { .. }));
    for ev in evs {
        app.event(ev);
    }
    assert!(!app.rows().iter().any(|r| r.working), "{:?}", rows(&app));
}

#[test]
fn next_commit_after_the_path_hides_the_open_one() {
    let (tmp, root) = repo();
    let mut app = open(&root, &tmp.path().join("cache"), None);
    press(&mut app, KeyCode::Enter);
    let m4 = app.rows()[0].info.commit.clone();
    assert_eq!(app.commit_id(), Some(m4.as_str()));
    to_commit_list(&mut app);
    set_path(&mut app, "b.txt");
    assert_eq!(rows(&app), ["m3", "m1"]);
    assert_eq!(app.commit_id(), Some(m4.as_str()), "the view stays open");
    press(&mut app, KeyCode::Esc);
    keys(&mut app, "]]");
    let m3 = app.rows()[0].info.commit.clone();
    assert_eq!(
        app.commit_id(),
        Some(m3.as_str()),
        "the nearest older match"
    );
}

#[test]
fn long_and_wide_paths_fit_the_status_line() {
    let (tmp, root) = repo();
    let deep = "a/very/long/directory/name/that/goes/on/and/on/x.txt";
    std::fs::create_dir_all(root.join(deep).parent().unwrap()).unwrap();
    std::fs::write(root.join(deep), "x\n").unwrap();
    let wide = "界".repeat(40);
    std::fs::write(root.join(&wide), "w\n").unwrap();
    commit(&root, "deep");
    let mut app = open(&root, &tmp.path().join("cache"), Some(deep));
    keys(&mut app, "fdeep");
    press(&mut app, KeyCode::Enter);
    let s = screen(&mut app, 60, 6);
    let status = &s[5];
    assert!(status.contains("path: …"), "left-elided: {status}");
    assert!(status.contains("on/x.txt"), "{status}");
    assert!(status.contains("0 new"), "{status}");
    assert!(status.ends_with("1/1"), "{status}");
    // A wide path on both screens, at several widths: no overflow.
    keys(&mut app, " F");
    keys(&mut app, "F");
    app.paste(&wide);
    press(&mut app, KeyCode::Enter);
    assert_eq!(rows(&app), ["deep"]);
    for w in [30, 40, 60, 80] {
        let s = screen(&mut app, w, 6);
        assert!(s[5].ends_with("1/1"), "log {w}: {}", s[5]);
    }
    press(&mut app, KeyCode::Enter);
    for w in [30, 40, 60, 80] {
        let s = screen(&mut app, w, 6);
        assert!(s[5].contains("path:"), "commit {w}: {}", s[5]);
    }
}
