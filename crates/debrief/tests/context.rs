//! `T`, hunks / the full file: the reload
//! keeps the cursor line, `]c` walks change blocks in the full file, the
//! mode persists and comes from `[view] context`, comments cross modes.

mod common;

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use common::{commit, git};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use debrief::app::config::Config;
use debrief::app::{App, Mode, RowKind, Screen, StartOptions};
use debrief::vcs::{self, Context, LineKind};

struct Fixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    cache: PathBuf,
}

/// `base`, then `change`: a.txt (60 lines) edits line 3, drops 20-22 and
/// edits 40 (three hunks); b.txt edits line 2. Then `top` adds c.txt.
fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir(&root).unwrap();
    git(&root, &["init", "-q"]);
    let a: Vec<String> = (1..=60).map(|i| format!("a{i}")).collect();
    std::fs::write(root.join("a.txt"), a.join("\n") + "\n").unwrap();
    std::fs::write(root.join("b.txt"), "b1\nb2\n").unwrap();
    commit(&root, "base");
    let mut a = a;
    a[39] = "a40 new".into();
    a.drain(19..22);
    a[2] = "a3 new".into();
    std::fs::write(root.join("a.txt"), a.join("\n") + "\n").unwrap();
    std::fs::write(root.join("b.txt"), "b1\nb2 new\n").unwrap();
    commit(&root, "change");
    std::fs::write(root.join("c.txt"), "c1\n").unwrap();
    commit(&root, "top");
    Fixture {
        root: root.canonicalize().unwrap(),
        cache: tmp.path().join("cache"),
        _tmp: tmp,
    }
}

impl Fixture {
    fn app_with(&self, toml: &str) -> App {
        let mut opts = StartOptions::new(self.cache.clone());
        opts.config = Config::parse(toml).unwrap();
        App::new(
            vcs::open(&self.root, vcs::OpenOptions::default()).unwrap(),
            opts,
        )
        .unwrap()
    }

    /// The app with `change` (log row 1) open.
    fn open_with(&self, toml: &str) -> App {
        let mut app = self.app_with(toml);
        keys(&mut app, "ggj");
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.screen(), Screen::Commit(1));
        app
    }

    fn open(&self) -> App {
        self.open_with("")
    }
}

fn press(app: &mut App, code: KeyCode) {
    app.key(KeyEvent::new(code, KeyModifiers::NONE));
}

fn keys(app: &mut App, s: &str) {
    s.chars().for_each(|c| press(app, KeyCode::Char(c)));
}

fn row_of(app: &App, text: &str, kind: RowKind) -> usize {
    app.commit_rows()
        .iter()
        .position(|r| r.text == text && r.kind == kind)
        .unwrap_or_else(|| panic!("no {kind:?} row {text:?}"))
}

fn has_row(app: &App, text: &str) -> bool {
    app.commit_rows().iter().any(|r| r.text == text)
}

fn cursor_text(app: &App) -> String {
    app.commit_rows()[app.commit_cursor().row].text.clone()
}

fn goto(app: &mut App, row: usize) {
    keys(app, &format!("{}G", row + 1));
    assert_eq!(app.commit_cursor().row, row);
}

#[test]
fn t_switches_to_the_full_file_and_back_keeping_the_line() {
    let f = fixture();
    let mut app = f.open();
    assert_eq!(app.context(), Context::Default);
    assert!(!has_row(&app, "a30"), "far from any change");
    let r = row_of(&app, "a40 new", RowKind::Line(LineKind::Added));
    goto(&mut app, r);
    let hunks = app.commit_rows().len();

    keys(&mut app, "T");
    assert_eq!(app.context(), Context::Full);
    assert_eq!(app.status(), "full file");
    assert!(has_row(&app, "a30"));
    assert!(has_row(&app, "a1"));
    assert!(app.commit_rows().len() > hunks);
    assert_eq!(cursor_text(&app), "a40 new");
    let hunk_headers = |app: &App, file: usize| {
        app.commit_rows()
            .iter()
            .filter(|r| r.kind == RowKind::HunkHeader && r.file == Some(file))
            .count()
    };
    assert_eq!(hunk_headers(&app, 0), 1, "one hunk header per file");

    keys(&mut app, "T");
    assert_eq!(app.context(), Context::Default);
    assert_eq!(app.status(), "hunks");
    assert_eq!(app.commit_rows().len(), hunks);
    assert_eq!(cursor_text(&app), "a40 new");
}

#[test]
fn a_line_missing_in_hunks_lands_on_the_nearest_row_of_its_file() {
    let f = fixture();
    let mut app = f.open_with("[view]\ncontext = \"full\"\n");
    assert_eq!(app.context(), Context::Full, "[view] context");
    let r = row_of(&app, "a30", RowKind::Line(LineKind::Context));
    goto(&mut app, r);
    keys(&mut app, "T");
    assert!(!has_row(&app, "a30"));
    let row = &app.commit_rows()[app.commit_cursor().row];
    assert_eq!(row.file, Some(0), "same file");
    // a30 is new line 27: the nearest shown is the second hunk's last
    // line, a25 (new 22), not the third's first, a37 (new 34).
    assert_eq!(cursor_text(&app), "a25");
}

#[test]
fn the_message_and_a_file_header_keep_their_place() {
    let f = fixture();
    let mut app = f.open();
    keys(&mut app, "gg");
    keys(&mut app, "T");
    assert_eq!(app.commit_cursor().row, 0);
    assert_eq!(cursor_text(&app), "change");
    let h = app
        .commit_rows()
        .iter()
        .position(|r| r.kind == RowKind::FileHeader && r.text.contains("b.txt"))
        .unwrap();
    goto(&mut app, h);
    keys(&mut app, "T");
    let row = &app.commit_rows()[app.commit_cursor().row];
    assert_eq!(row.kind, RowKind::FileHeader);
    assert!(row.text.contains("b.txt"));
}

#[test]
fn bracket_c_jumps_between_change_blocks_in_the_full_file() {
    let f = fixture();
    let mut app = f.open_with("[view]\ncontext = \"full\"\n");
    keys(&mut app, "gg");
    let mut seen = Vec::new();
    loop {
        let before = app.commit_cursor().row;
        keys(&mut app, "]c");
        if app.commit_cursor().row == before {
            break;
        }
        seen.push(cursor_text(&app));
    }
    // a.txt: line 3 (removed then added), 20-22 removed, line 40; b.txt
    // line 2.
    assert_eq!(seen, ["a3", "a20", "a40", "b2"]);
    assert_eq!(app.status(), "no next change");
    keys(&mut app, "[c");
    assert_eq!(cursor_text(&app), "a40");
    keys(&mut app, "2[c");
    assert_eq!(cursor_text(&app), "a3");
    keys(&mut app, "[c");
    assert_eq!(app.status(), "no previous change");

    // In hunks `]c` still goes by hunk header.
    keys(&mut app, "T");
    keys(&mut app, "gg]c");
    assert_eq!(
        app.commit_rows()[app.commit_cursor().row].kind,
        RowKind::HunkHeader
    );
}

#[test]
fn the_mode_persists_for_other_commits_and_the_sidebar_counts_do_not_change() {
    let f = fixture();
    let mut app = f.open();
    let counts = |app: &App| -> Vec<(usize, usize)> {
        app.commit_diff()
            .unwrap()
            .files
            .iter()
            .map(debrief::app::commit::file_counts)
            .collect()
    };
    let before = counts(&app);
    keys(&mut app, "T");
    assert_eq!(counts(&app), before);
    keys(&mut app, "[[");
    assert_eq!(app.commit_rows()[0].text, "top");
    keys(&mut app, "]]");
    assert!(has_row(&app, "a30"), "still full");
    press(&mut app, KeyCode::Esc);
    press(&mut app, KeyCode::Enter);
    assert!(has_row(&app, "a30"), "still full after the log");
}

#[test]
fn t_reloads_off_thread_and_drops_the_stale_load() {
    let f = fixture();
    let mut app = f.app_with("");
    let (tx, rx) = mpsc::channel();
    app.set_sender(tx);
    keys(&mut app, "ggj");
    press(&mut app, KeyCode::Enter);
    let recv = || rx.recv_timeout(Duration::from_secs(20)).unwrap();
    app.event(recv());
    let r = row_of(&app, "a40 new", RowKind::Line(LineKind::Added));
    goto(&mut app, r);
    keys(&mut app, "T");
    assert!(app.commit_loading());
    // Toggling back before the first answer: only the last one counts.
    keys(&mut app, "T");
    let events: Vec<_> = (0..2).map(|_| recv()).collect();
    for ev in events {
        app.event(ev);
    }
    // Highlighting of plain text sends nothing, so the queue is empty.
    assert!(!app.commit_loading());
    assert!(!has_row(&app, "a30"));
    assert_eq!(cursor_text(&app), "a40 new");
}

#[test]
fn a_comment_in_one_mode_marks_its_line_in_the_other() {
    let f = fixture();
    let mut app = f.open_with("[view]\ncontext = \"full\"\n");
    let r = row_of(&app, "a40 new", RowKind::Line(LineKind::Added));
    goto(&mut app, r);
    keys(&mut app, "cc");
    assert_eq!(app.mode(), Mode::Comment);
    keys(&mut app, "full");
    press(&mut app, KeyCode::Enter);
    keys(&mut app, "T");
    let r = row_of(&app, "a40 new", RowKind::Line(LineKind::Added));
    assert_eq!(app.review_marked_rows(), [r]);

    let r2 = row_of(&app, "a3 new", RowKind::Line(LineKind::Added));
    goto(&mut app, r2);
    keys(&mut app, "cc");
    keys(&mut app, "hunks");
    press(&mut app, KeyCode::Enter);
    keys(&mut app, "T");
    let rows = [
        row_of(&app, "a3 new", RowKind::Line(LineKind::Added)),
        row_of(&app, "a40 new", RowKind::Line(LineKind::Added)),
    ];
    assert_eq!(app.review_marked_rows(), rows);
}

#[test]
fn the_git_working_copy_follows_t() {
    let f = fixture();
    let text = std::fs::read_to_string(f.root.join("a.txt")).unwrap();
    std::fs::write(f.root.join("a.txt"), text.replace("a50\n", "a50 wip\n")).unwrap();
    let mut app = f.app_with("");
    keys(&mut app, "gg");
    press(&mut app, KeyCode::Enter);
    assert!(app.viewing_working_copy());
    assert!(!has_row(&app, "a10"));
    let r = row_of(&app, "a50 wip", RowKind::Line(LineKind::Added));
    goto(&mut app, r);
    keys(&mut app, "T");
    assert!(app.viewing_working_copy());
    assert!(has_row(&app, "a10"));
    assert_eq!(cursor_text(&app), "a50 wip");
    // A refresh keeps the mode.
    keys(&mut app, "r");
    assert!(has_row(&app, "a10"));
}

#[test]
fn full_file_highlighting_starts_at_line_one() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir(&root).unwrap();
    git(&root, &["init", "-q"]);
    // A block comment opened far above the change: hunks colour the
    // changed line as code, the full file as a comment.
    let mut lines = vec!["/*".to_string()];
    lines.extend((1..=20).map(|i| format!("note {i}")));
    lines.push("*/".into());
    lines.push("fn f() {}".into());
    std::fs::write(root.join("x.rs"), lines.join("\n") + "\n").unwrap();
    commit(&root, "base");
    lines[15] = "note changed".into();
    std::fs::write(root.join("x.rs"), lines.join("\n") + "\n").unwrap();
    commit(&root, "change");
    let root: &Path = &root.canonicalize().unwrap();
    let mut app = App::new(
        vcs::open(root, vcs::OpenOptions::default()).unwrap(),
        StartOptions::new(tmp.path().join("cache")),
    )
    .unwrap();
    keys(&mut app, "gg");
    press(&mut app, KeyCode::Enter);
    let colour = |app: &App| {
        let r = row_of(app, "note changed", RowKind::Line(LineKind::Added));
        app.commit_spans(&app.commit_rows()[r]).cloned()
    };
    let hunks = colour(&app);
    keys(&mut app, "T");
    let full = colour(&app);
    assert!(full.is_some());
    assert_ne!(hunks, full, "the full file knows it is in a comment");
}

#[test]
fn the_working_copy_log_editor_target_ignores_t() {
    let f = fixture();
    let text = std::fs::read_to_string(f.root.join("a.txt")).unwrap();
    std::fs::write(f.root.join("a.txt"), text.replace("a50\n", "a50 wip\n")).unwrap();
    let a = f.root.join("a.txt");
    let mut app = f.app_with("");
    keys(&mut app, "gg");
    // The first hunk's first line: three lines of context above a50,
    // now new line 47 (a20-22 were dropped).
    assert_eq!(app.edit_target(), Ok((a.clone(), 44)));
    press(&mut app, KeyCode::Enter);
    keys(&mut app, "T");
    assert_eq!(app.context(), Context::Full);
    press(&mut app, KeyCode::Esc);
    assert_eq!(app.edit_target(), Ok((a.clone(), 44)), "after T");
    // Full from the start too.
    let mut app = f.app_with("[view]\ncontext = \"full\"\n");
    keys(&mut app, "gg");
    assert_eq!(app.edit_target(), Ok((a, 44)), "[view] context = full");
}

#[test]
fn the_git_working_copy_loads_off_thread_on_open_t_and_r() {
    let f = fixture();
    let text = std::fs::read_to_string(f.root.join("a.txt")).unwrap();
    std::fs::write(f.root.join("a.txt"), text.replace("a50\n", "a50 wip\n")).unwrap();
    let mut app = f.app_with("");
    let (tx, rx) = mpsc::channel();
    app.set_sender(tx);
    let recv = || rx.recv_timeout(Duration::from_secs(20)).unwrap();
    keys(&mut app, "gg");
    press(&mut app, KeyCode::Enter);
    assert!(app.viewing_working_copy());
    assert!(app.commit_loading(), "open returns before the diff");
    app.event(recv());
    assert!(!app.commit_loading());
    let r = row_of(&app, "a50 wip", RowKind::Line(LineKind::Added));
    goto(&mut app, r);

    keys(&mut app, "T");
    assert!(app.commit_loading(), "T returns before the diff");
    assert!(!has_row(&app, "a10"), "nothing applied yet");
    // Toggling back before the answer: only the last load counts.
    keys(&mut app, "T");
    keys(&mut app, "T");
    let mut events: Vec<_> = (0..3).map(|_| recv()).collect();
    events.reverse();
    for ev in events {
        app.event(ev);
    }
    assert!(!app.commit_loading());
    assert!(has_row(&app, "a10"), "the full file arrived");
    assert_eq!(cursor_text(&app), "a50 wip");

    // `r` too, keeping the cursor row.
    let row = app.commit_cursor().row;
    keys(&mut app, "r");
    assert!(app.commit_loading(), "r returns before the diff");
    // The diff and the log's row, in either order.
    let settle = |app: &mut App| {
        while let Ok(ev) = rx.recv_timeout(Duration::from_secs(3)) {
            app.event(ev);
        }
    };
    settle(&mut app);
    assert!(!app.commit_loading());
    assert_eq!(app.commit_cursor().row, row);

    // A tree cleaned meanwhile closes the view once the answer is in.
    git(&f.root, &["checkout", "-q", "--", "a.txt"]);
    keys(&mut app, "r");
    assert!(app.viewing_working_copy(), "r returns before the answer");
    settle(&mut app);
    assert_eq!(app.screen(), Screen::Log);
    assert_eq!(app.status(), "The working copy is clean");
}
