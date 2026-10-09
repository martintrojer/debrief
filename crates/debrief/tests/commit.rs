mod common;

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use common::{commit, git};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use debrief::app::{App, AppEvent, Focus, Mode, RowKind, Screen, StartOptions};
use debrief::seen::{Seen, Status};
use debrief::vcs::{self, LineKind};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

const LONG: &str = "long line with 日本 wide text and then a lot more words to push it well past the edge of any terminal we test with, ending here: END";

/// A git repo: `base`, then `big` (m.txt modified in two hunks, new.txt
/// added without a final newline, r_old.txt renamed, bin.dat added), then
/// `top`. Only `top` is seen.
fn repo() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir(&root).unwrap();
    git(&root, &["init", "-q"]);
    let lines: Vec<String> = (1..=30).map(|i| format!("line {i}")).collect();
    std::fs::write(root.join("m.txt"), lines.join("\n") + "\n").unwrap();
    let keep: String = (1..=10).map(|i| format!("rename me {i}\n")).collect();
    std::fs::write(root.join("r_old.txt"), keep).unwrap();
    commit(&root, "base");

    let mut lines = lines;
    lines[1] = "line two changed".into();
    lines[24] = LONG.into();
    std::fs::write(root.join("m.txt"), lines.join("\n") + "\n").unwrap();
    std::fs::write(root.join("new.txt"), "added\nlast").unwrap();
    git(&root, &["mv", "r_old.txt", "r_new.txt"]);
    std::fs::write(root.join("bin.dat"), [0u8, 1, 2, 0, 255, 0]).unwrap();
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-q", "-m", "big change\n\nbody text"]);

    std::fs::write(root.join("top.txt"), "top\n").unwrap();
    commit(&root, "top");
    let root = root.canonicalize().unwrap();
    let cache = tmp.path().join("cache");
    let log = vcs::open(&root, vcs::OpenOptions::default())
        .unwrap()
        .log(None, 10)
        .unwrap();
    Seen::open(&cache, &root).unwrap().mark(&log[0]).unwrap();
    (tmp, root, cache)
}

fn open(root: &Path, cache: &Path) -> App {
    App::new(
        vcs::open(root, vcs::OpenOptions::default()).unwrap(),
        StartOptions::new(cache.to_path_buf()),
    )
    .unwrap()
}

fn press(app: &mut App, code: KeyCode) {
    app.key(KeyEvent::new(code, KeyModifiers::NONE));
}

fn ctrl(app: &mut App, c: char) {
    app.key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL));
}

fn keys(app: &mut App, s: &str) {
    s.chars().for_each(|c| press(app, KeyCode::Char(c)));
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

/// The app with `big` (log row 1) open, rendered once at 100x30.
fn open_big() -> (tempfile::TempDir, App) {
    let (tmp, root, cache) = repo();
    let mut app = open(&root, &cache);
    assert_eq!(app.cursor(), 2, "the oldest new commit");
    keys(&mut app, "k");
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.screen(), Screen::Commit(1));
    screen(&mut app, 100, 30);
    (tmp, app)
}

fn row(app: &App) -> usize {
    app.commit_cursor().row
}

fn kind(app: &App) -> RowKind {
    app.commit_rows()[row(app)].kind
}

fn b64decode(s: &str) -> Vec<u8> {
    let val = |c: u8| match c {
        b'A'..=b'Z' => c - b'A',
        b'a'..=b'z' => c - b'a' + 26,
        b'0'..=b'9' => c - b'0' + 52,
        b'+' => 62,
        b'/' => 63,
        _ => panic!("bad base64 {c}"),
    };
    let mut out = Vec::new();
    for chunk in s.as_bytes().chunks(4) {
        let v: Vec<u8> = chunk
            .iter()
            .filter(|&&c| c != b'=')
            .map(|&c| val(c))
            .collect();
        let n = v.iter().fold(0u32, |n, &d| n << 6 | d as u32) << (6 * (4 - v.len()));
        out.extend(&[(n >> 16) as u8, (n >> 8) as u8, n as u8][..v.len() - 1]);
    }
    out
}

/// The text an OSC 52 sequence puts on the clipboard.
fn clipboard(out: &[u8]) -> String {
    let s = String::from_utf8(out.to_vec()).unwrap();
    let b = s
        .strip_prefix("\x1b]52;c;")
        .and_then(|s| s.strip_suffix('\x07'))
        .unwrap_or_else(|| panic!("not OSC 52: {s:?}"));
    String::from_utf8(b64decode(b)).unwrap()
}

#[test]
fn rows_in_order_with_valid_indices() {
    let (_tmp, app) = open_big();
    let rows = app.commit_rows();
    let diff = app.commit_diff().unwrap();
    assert_eq!(rows[0].kind, RowKind::Message);
    assert_eq!(rows[0].text, "big change");
    assert_eq!(rows[1].text, "");
    assert_eq!(rows[2].text, "body text");
    assert_eq!(rows[3].kind, RowKind::Blank);
    let headers: Vec<&str> = rows
        .iter()
        .filter(|r| r.kind == RowKind::FileHeader)
        .map(|r| r.text.as_str())
        .collect();
    assert_eq!(
        headers,
        [
            "── bin.dat (added) (binary) ──",
            "── m.txt ──",
            "── new.txt (added) ──",
            "── r_old.txt → r_new.txt ──",
        ]
    );
    // Every Line row points at its DiffLine and carries its exact text.
    for r in rows {
        if let RowKind::Line(k) = r.kind {
            let l = &diff.files[r.file.unwrap()].hunks[r.hunk.unwrap()].lines[r.line.unwrap()];
            assert_eq!((l.kind, l.text.as_str()), (k, r.text.as_str()));
        }
    }
    let m = rows.iter().position(|r| r.text == "── m.txt ──").unwrap();
    let kinds: Vec<RowKind> = rows[m..].iter().map(|r| r.kind).collect();
    assert_eq!(
        kinds.iter().filter(|k| **k == RowKind::HunkHeader).count(),
        3
    );
    assert!(
        rows.iter()
            .any(|r| r.kind == RowKind::Line(LineKind::Added) && r.text == LONG)
    );
    // new.txt's last line has no newline: a NoNewline row follows it.
    let last = rows.iter().position(|r| r.text == "last").unwrap();
    assert_eq!(rows[last].kind, RowKind::Line(LineKind::Added));
    assert_eq!(rows[last + 1].kind, RowKind::NoNewline);
    assert_eq!(rows[last + 1].text, "\\ No newline at end of file");
    assert_eq!(rows[last + 1].line, rows[last].line);
    let bin = rows.iter().position(|r| r.kind == RowKind::Binary).unwrap();
    assert_eq!(rows[bin - 1].text, "── bin.dat (added) (binary) ──");
}

#[test]
fn renders_gutter_colours_and_sidebar() {
    let (_tmp, mut app) = open_big();
    let s = screen(&mut app, 100, 30);
    assert!(
        s[0].starts_with(" A bin.dat"),
        "in the message: no current file {s:#?}"
    );
    assert!(s[1].contains("m.txt") && s[1].contains("+2 −2"), "{}", s[1]);
    assert!(
        s[3].contains("r_new.txt") && s[3].contains("+0 −0"),
        "{}",
        s[3]
    );
    let added = s
        .iter()
        .find(|l| l.contains("line two changed"))
        .expect("added line drawn");
    assert!(added.contains("      2 + line two changed"), "{added}");
    assert!(
        s.iter().any(|l| l.ends_with("│   2      - line 2")),
        "{s:#?}"
    );
    assert!(
        s.iter().any(|l| l.ends_with("│   1    1   line 1")),
        "{s:#?}"
    );
    assert!(
        s.iter()
            .any(|l| l.ends_with("│            @@ -1,5 +1,5 @@")),
        "{s:#?}"
    );
    let id = &app.commit_id().unwrap()[..8];
    assert!(s[29].starts_with(&format!(" {id}")), "{}", s[29]);
    let n = app.commit_rows().len();
    assert!(s[29].ends_with(&format!("g? help 1/{n}")), "{}", s[29]);
    // Colours: added green, removed red, headers bold.
    let mut t = Terminal::new(TestBackend::new(100, 30)).unwrap();
    t.draw(|f| app.render(f)).unwrap();
    let buf = t.backend().buffer().clone();
    let cell = |line: &str, text: &str| {
        let y = s.iter().position(|l| l.ends_with(line)).unwrap() as u16;
        let x = s[y as usize].chars().count() - text.chars().count();
        buf[(x as u16, y)].style()
    };
    let green = ratatui::style::Color::Rgb(0xa6, 0xe3, 0xa1);
    let red = ratatui::style::Color::Rgb(0xf3, 0x8b, 0xa8);
    assert_eq!(
        cell("+ line two changed", "line two changed").fg,
        Some(green)
    );
    assert_eq!(cell("- line 2", "line 2").fg, Some(red));
    let bold = ratatui::style::Modifier::BOLD;
    let hy = s.iter().position(|l| l.contains("── m.txt ──")).unwrap();
    let hx = s[hy].chars().position(|c| c == 'm').unwrap() as u16;
    assert!(buf[(hx, hy as u16)].style().add_modifier.contains(bold));
    assert!(
        s[hy].ends_with("──────"),
        "the header rule runs to the right edge: {}",
        s[hy]
    );
    assert_eq!(s[hy].chars().count(), 100, "{}", s[hy]);
    assert!(cell("@@ -1,5 +1,5 @@", "@@").add_modifier.contains(bold));
    // No wrap: the long line is cut at the width.
    assert!(!s.iter().any(|l| l.contains("END")));
    keys(&mut app, "]f");
    let s = screen(&mut app, 100, 30);
    assert!(s[0].starts_with("▎A bin.dat"), "{}", s[0]);
    keys(&mut app, "gg");
    // Below 80 columns the sidebar is hidden.
    let s = screen(&mut app, 79, 30);
    assert!(s[0].starts_with(' ') && !s[0].contains('▎'), "{}", s[0]);
    assert!(s[0].contains("big change"));
    // <leader>e toggles it.
    let s = screen(&mut app, 100, 30);
    assert!(s[0].contains("bin.dat"));
    keys(&mut app, " e");
    let s = screen(&mut app, 100, 30);
    assert!(!s[0].contains("bin.dat"), "{}", s[0]);
    keys(&mut app, " e");
    let s = screen(&mut app, 100, 30);
    assert!(s[0].contains("bin.dat"));
}

#[test]
fn file_and_hunk_jumps() {
    let (_tmp, mut app) = open_big();
    let mut seen = Vec::new();
    for _ in 0..4 {
        keys(&mut app, "]f");
        assert_eq!(kind(&app), RowKind::FileHeader);
        seen.push(app.commit_rows()[row(&app)].file.unwrap());
    }
    assert_eq!(seen, [0, 1, 2, 3]);
    keys(&mut app, "]f");
    assert_eq!(app.status(), "no next file");
    assert_eq!(app.current_file(), Some(3));
    keys(&mut app, "[f");
    assert_eq!(app.current_file(), Some(2));
    keys(&mut app, "gg2]f");
    assert_eq!(app.current_file(), Some(1));

    keys(&mut app, "gg");
    let mut hunks = Vec::new();
    for _ in 0..3 {
        keys(&mut app, "]c");
        assert_eq!(kind(&app), RowKind::HunkHeader);
        let r = &app.commit_rows()[row(&app)];
        hunks.push((r.file.unwrap(), r.hunk.unwrap()));
    }
    assert_eq!(hunks, [(1, 0), (1, 1), (2, 0)]);
    keys(&mut app, "[c");
    assert_eq!(app.commit_rows()[row(&app)].hunk, Some(1));
}

#[test]
fn sidebar_enter_jumps_and_marks_the_current_file() {
    let (_tmp, mut app) = open_big();
    ctrl(&mut app, 'w');
    keys(&mut app, "h");
    assert_eq!(app.focus(), Focus::Files);
    keys(&mut app, "jj");
    assert_eq!(app.sidebar_selected(), 2);
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.focus(), Focus::Diff);
    assert_eq!(kind(&app), RowKind::FileHeader);
    assert_eq!(app.current_file(), Some(2));
    let s = screen(&mut app, 100, 30);
    assert!(s[2].starts_with("▎A new.txt"), "{s:#?}");
    assert!(s[0].starts_with(" A bin.dat"), "{}", s[0]);
    // C-w h, then C-w l back; q in the list returns to the log.
    ctrl(&mut app, 'w');
    keys(&mut app, "h");
    ctrl(&mut app, 'w');
    keys(&mut app, "l");
    assert_eq!(app.focus(), Focus::Diff);
}

#[test]
fn next_commit_opens_and_marks_seen() {
    let (_tmp, mut app) = open_big();
    assert_eq!(app.rows()[1].status, Status::Seen, "opening marks seen");
    assert_eq!(app.rows()[2].status, Status::New);
    keys(&mut app, "]]");
    assert_eq!(app.screen(), Screen::Commit(2));
    assert_eq!(app.rows()[2].status, Status::Seen);
    assert_eq!(app.commit_rows()[0].text, "base");
    keys(&mut app, "]]");
    assert_eq!(app.status(), "no next commit");
    keys(&mut app, "[[[[");
    assert_eq!(app.screen(), Screen::Commit(0));
    assert_eq!(app.commit_rows()[0].text, "top");
    press(&mut app, KeyCode::Esc);
    assert_eq!(app.screen(), Screen::Log);
    assert_eq!(app.cursor(), 0, "back on the commit last shown");
}

#[test]
fn visual_yank_copies_diff_text_over_osc52() {
    let (_tmp, mut app) = open_big();
    // To the first hunk's "-line 2" row.
    keys(&mut app, "/line 2");
    press(&mut app, KeyCode::Enter);
    while app.commit_rows()[row(&app)].kind != RowKind::Line(LineKind::Removed) {
        keys(&mut app, "n");
    }
    assert_eq!(app.selection_rows(), None);
    keys(&mut app, "Vj");
    assert_eq!(app.mode(), Mode::Visual);
    let (lo, hi) = app.selection_rows().unwrap();
    assert_eq!(hi, lo + 1);
    keys(&mut app, "y");
    assert_eq!(app.mode(), Mode::Normal);
    assert_eq!(
        clipboard(&app.take_output()),
        "-line 2\n+line two changed\n"
    );
    assert!(app.take_output().is_empty(), "drained");

    // The no-newline row is yanked as it shows.
    keys(&mut app, "/last");
    press(&mut app, KeyCode::Enter);
    keys(&mut app, "vjy");
    assert_eq!(
        clipboard(&app.take_output()),
        "+last\n\\ No newline at end of file\n"
    );
}

#[test]
fn search_wraps() {
    let (_tmp, mut app) = open_big();
    keys(&mut app, "/line 2");
    assert_eq!(app.search_prompt().as_deref(), Some("/line 2"));
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.search_prompt(), None);
    let a = row(&app);
    assert_eq!(app.commit_rows()[a].text, "line 2");
    keys(&mut app, "n");
    let b = row(&app);
    assert!(b > a);
    let n = app.search_hits().len();
    assert!(n > 2, "{n}");
    for _ in 0..n - 2 {
        keys(&mut app, "n");
        assert_eq!(app.status(), "");
    }
    keys(&mut app, "n");
    assert_eq!(row(&app), a, "n wrapped around");
    assert_eq!(app.status(), "search hit BOTTOM, continuing at TOP");
    keys(&mut app, "N");
    assert_eq!(app.status(), "search hit TOP, continuing at BOTTOM");
    keys(&mut app, "n");
    keys(&mut app, "n");
    assert_eq!(row(&app), b);
}

#[test]
fn esc_precedence_and_q() {
    let (_tmp, mut app) = open_big();
    // A pending sequence: Esc drops it, the view stays.
    keys(&mut app, "]");
    press(&mut app, KeyCode::Esc);
    assert_eq!(app.screen(), Screen::Commit(1));
    keys(&mut app, "f");
    assert_eq!(row(&app), 0, "the ] was dropped");
    // A count likewise.
    keys(&mut app, "3");
    press(&mut app, KeyCode::Esc);
    assert_eq!(app.screen(), Screen::Commit(1));
    keys(&mut app, "j");
    assert_eq!(row(&app), 1, "the count was dropped");
    // The help overlay.
    keys(&mut app, "g?");
    assert_eq!(app.mode(), Mode::Help);
    press(&mut app, KeyCode::Esc);
    assert_eq!(
        (app.mode(), app.screen()),
        (Mode::Normal, Screen::Commit(1))
    );
    // Search entry; q is input there.
    keys(&mut app, "/q");
    assert_eq!(app.search_prompt().as_deref(), Some("/q"));
    press(&mut app, KeyCode::Esc);
    assert_eq!(
        (app.mode(), app.screen()),
        (Mode::Normal, Screen::Commit(1))
    );
    // Visual mode.
    keys(&mut app, "V");
    press(&mut app, KeyCode::Esc);
    assert_eq!(
        (app.mode(), app.screen()),
        (Mode::Normal, Screen::Commit(1))
    );
    // Idle: back to the log on the same commit.
    press(&mut app, KeyCode::Esc);
    assert_eq!(app.screen(), Screen::Log);
    assert_eq!(app.cursor(), 1);
    assert!(app.commit_rows().is_empty());
    // q from the commit view too.
    press(&mut app, KeyCode::Enter);
    keys(&mut app, "q");
    assert_eq!(app.screen(), Screen::Log);
    assert!(!app.should_quit());
}

#[test]
fn horizontal_motions_follow_the_cursor() {
    let (_tmp, mut app) = open_big();
    keys(&mut app, "/long line");
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.commit_rows()[row(&app)].text, LONG);
    assert_eq!(app.commit_cursor().col, 0);
    keys(&mut app, "$");
    let end = LONG.chars().count() + 2 - 1; // 日本 are two columns each
    assert_eq!(app.commit_cursor().col, end);
    let s = screen(&mut app, 100, 30);
    let line = s.iter().find(|l| l.contains("END")).expect("view followed");
    assert!(line.ends_with("END"), "{line}");
    assert!(app.commit_scroll().1 > 0);
    keys(&mut app, "0");
    assert_eq!(app.commit_scroll().1, 0);
    // w / e / b over graphemes: "long line with 日本 wide"
    keys(&mut app, "3w");
    assert_eq!(app.commit_cursor().col, 15, "on 日");
    keys(&mut app, "e");
    assert_eq!(app.commit_cursor().col, 17, "on 本");
    keys(&mut app, "l");
    assert_eq!(app.commit_cursor().col, 19, "past the wide pair");
    keys(&mut app, "h");
    assert_eq!(app.commit_cursor().col, 17);
    keys(&mut app, "b");
    assert_eq!(app.commit_cursor().col, 15);
    // { } jump between blank rows; C-e / C-y scroll.
    keys(&mut app, "gg}");
    assert_eq!(kind(&app), RowKind::Message, "the message's empty line");
    keys(&mut app, "}");
    assert_eq!(kind(&app), RowKind::Blank);
    let top = app.commit_scroll().0;
    ctrl(&mut app, 'e');
    assert_eq!(app.commit_scroll().0, top + 1);
    ctrl(&mut app, 'y');
    assert_eq!(app.commit_scroll().0, top);
    keys(&mut app, "G");
    keys(&mut app, "zt");
    assert_eq!(
        app.commit_scroll().0,
        app.commit_rows().len().saturating_sub(29)
    );
    keys(&mut app, "H");
    assert_eq!(row(&app), app.commit_scroll().0);
}

#[test]
fn enter_loads_off_thread_and_drops_stale_results() {
    let (_tmp, root, cache) = repo();
    let mut app = open(&root, &cache);
    let (tx, rx) = mpsc::channel();
    app.set_sender(tx);
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.screen(), Screen::Commit(2));
    assert!(app.commit_loading());
    let s = screen(&mut app, 100, 10);
    assert!(s[9].contains("loading…"), "{}", s[9]);
    let ev = rx.recv_timeout(Duration::from_secs(20)).unwrap();
    app.event(ev);
    assert!(!app.commit_loading());
    assert_eq!(app.commit_rows()[0].text, "base");

    // Leave before the answer arrives: it is dropped.
    press(&mut app, KeyCode::Esc);
    keys(&mut app, "k");
    press(&mut app, KeyCode::Enter);
    press(&mut app, KeyCode::Esc);
    let ev = rx.recv_timeout(Duration::from_secs(20)).unwrap();
    app.event(ev);
    assert_eq!(app.screen(), Screen::Log);
    assert!(app.commit_rows().is_empty());

    // An older request's answer after a newer open is dropped too.
    press(&mut app, KeyCode::Enter);
    keys(&mut app, "]]");
    let first = rx.recv_timeout(Duration::from_secs(20)).unwrap();
    let second = rx.recv_timeout(Duration::from_secs(20)).unwrap();
    let (stale, fresh) = match (&first, &second) {
        (AppEvent::Loaded { request: a, .. }, AppEvent::Loaded { request: b, .. }) if a < b => {
            (first, second)
        }
        _ => (second, first),
    };
    app.event(stale);
    assert!(app.commit_loading());
    app.event(fresh);
    assert_eq!(app.commit_rows()[0].text, "base");
}

#[test]
fn a_failed_load_shows_the_error() {
    let (_tmp, root, cache) = repo();
    let mut app = open(&root, &cache);
    let (tx, rx) = mpsc::channel();
    app.set_sender(tx);
    std::fs::rename(root.join(".git"), root.join("git-away")).unwrap();
    press(&mut app, KeyCode::Enter);
    let ev = rx.recv_timeout(Duration::from_secs(20)).unwrap();
    app.event(ev);
    assert!(!app.commit_loading());
    assert!(app.status().starts_with("cannot show "), "{}", app.status());
    let s = screen(&mut app, 100, 10);
    assert!(s[9].contains("cannot show"), "{}", s[9]);
}

#[test]
fn five_digit_line_numbers_widen_the_gutter_and_dollar_stays_visible() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir(&root).unwrap();
    git(&root, &["init", "-q"]);
    let filler = "filler\n".repeat(10000);
    std::fs::write(root.join("large.txt"), &filler).unwrap();
    commit(&root, "large base");
    let edited =
        filler.strip_suffix("filler\n").unwrap().to_string() + "abcdefghijklmnopqrstuvwxyzEND\n";
    std::fs::write(root.join("large.txt"), edited).unwrap();
    commit(&root, "large edit");
    let root = root.canonicalize().unwrap();
    let mut app = open(&root, &tmp.path().join("cache"));
    keys(&mut app, "gg");
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.commit_num_width(), 5);
    screen(&mut app, 40, 20);
    keys(&mut app, "/abcdefghijklmnopqrstuvwxyz");
    press(&mut app, KeyCode::Enter);
    keys(&mut app, "$");
    let s = screen(&mut app, 40, 20);
    let line = s
        .iter()
        .find(|l| l.contains("10000 +"))
        .expect("the edited line is on screen");
    assert!(
        line.ends_with("END"),
        "the cursor's grapheme is drawn: {line:?}"
    );
    assert!(app.commit_scroll().1 > 0);
}

#[test]
fn a_search_typed_while_loading_finds_the_loaded_rows() {
    let (_tmp, root, cache) = repo();
    let mut app = open(&root, &cache);
    let (tx, rx) = mpsc::channel();
    app.set_sender(tx);
    press(&mut app, KeyCode::Enter);
    assert!(app.commit_loading());
    keys(&mut app, "/line");
    press(&mut app, KeyCode::Enter);
    app.event(rx.recv_timeout(Duration::from_secs(20)).unwrap());
    assert!(!app.commit_loading());
    assert!(!app.search_hits().is_empty(), "hits recomputed on load");
    keys(&mut app, "n");
    assert!(app.commit_rows()[row(&app)].text.contains("line"));
}
