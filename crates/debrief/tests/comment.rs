//! Comments on diff lines: capture (`cc`, visual `c`), the prompt, the
//! gutter, `]r`, `K`, polling the shared batch, and a jj rewrite. A temp
//! repo and a temp cache dir per test; nothing sleeps.

mod common;

use std::path::{Path, PathBuf};
use std::time::Instant;

use common::{JjEnv, commit, git, have_jj};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use debrief::app::comment::{EMPTY_COMMENT, EXCERPT_LINES, NOT_A_LINE, SPANS_FILES};
use debrief::app::review::{MARKER, REVIEW_POLL};
use debrief::app::{App, Mode, RowKind, Screen, StartOptions};
use debrief::vcs::{self, LineKind};
use debrief_review::{Comment, Rev, Review, Side};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

struct Fixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    cache: PathBuf,
}

/// `base`, then `change`: a.txt edits line 3, drops 20-22 and edits 40
/// (three hunks); b.txt edits line 2; big.txt is added with 50 lines.
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
    let big: String = (1..=50).map(|i| format!("g{i}\n")).collect();
    std::fs::write(root.join("big.txt"), big).unwrap();
    commit(&root, "change");
    Fixture {
        root: root.canonicalize().unwrap(),
        cache: tmp.path().join("cache"),
        _tmp: tmp,
    }
}

fn app_on(root: &Path, cache: &Path) -> App {
    App::new(
        vcs::open(root).unwrap(),
        StartOptions::new(cache.to_path_buf()),
    )
    .unwrap()
}

impl Fixture {
    /// The app with `change` (the top commit) open.
    fn open(&self) -> App {
        let mut app = app_on(&self.root, &self.cache);
        keys(&mut app, "gg");
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.screen(), Screen::Commit(0));
        app
    }

    fn review(&self) -> Review {
        Review::open(&self.cache, &self.root).unwrap()
    }

    fn comments(&self) -> Vec<Comment> {
        self.review().comments().to_vec()
    }
}

fn press(app: &mut App, code: KeyCode) {
    app.key(KeyEvent::new(code, KeyModifiers::NONE));
}

fn keys(app: &mut App, s: &str) {
    s.chars().for_each(|c| press(app, KeyCode::Char(c)));
}

fn type_and_save(app: &mut App, body: &str) {
    assert_eq!(app.mode(), Mode::Comment, "prompt open: {}", app.status());
    keys(app, body);
    press(app, KeyCode::Enter);
    assert_eq!(app.mode(), Mode::Normal);
}

/// Index of the diff row with `text` and `kind`.
fn row_of(app: &App, text: &str, kind: RowKind) -> usize {
    app.commit_rows()
        .iter()
        .position(|r| r.text == text && r.kind == kind)
        .unwrap_or_else(|| panic!("no {kind:?} row {text:?}"))
}

fn added(app: &App, text: &str) -> usize {
    row_of(app, text, RowKind::Line(LineKind::Added))
}

fn removed(app: &App, text: &str) -> usize {
    row_of(app, text, RowKind::Line(LineKind::Removed))
}

fn context(app: &App, text: &str) -> usize {
    row_of(app, text, RowKind::Line(LineKind::Context))
}

fn header(app: &App, path: &str) -> usize {
    app.commit_rows()
        .iter()
        .position(|r| r.kind == RowKind::FileHeader && r.text.contains(path))
        .unwrap()
}

/// Move the cursor to diff row `row`.
fn goto(app: &mut App, row: usize) {
    keys(app, &format!("{}G", row + 1));
    assert_eq!(app.commit_cursor().row, row);
}

/// Select rows `from..=to` and press `c`.
fn select_and_comment(app: &mut App, from: usize, to: usize) {
    goto(app, from);
    keys(app, "V");
    for _ in from..to {
        keys(app, "j");
    }
    assert_eq!(app.selection_rows(), Some((from, to)));
    keys(app, "c");
}

fn screen(app: &mut App) -> Vec<String> {
    let mut t = Terminal::new(TestBackend::new(100, 30)).unwrap();
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

#[test]
fn cc_on_an_added_line() {
    let f = fixture();
    let mut app = f.open();
    let r = added(&app, "a3 new");
    goto(&mut app, r);
    keys(&mut app, "cc");
    assert_eq!(app.comment_prompt().as_deref(), Some("comment: "));
    type_and_save(&mut app, "why?");
    assert_eq!(app.status(), "Comment added (1 in batch)");
    let cs = f.comments();
    assert_eq!(cs.len(), 1);
    let c = &cs[0];
    assert_eq!(c.path, "a.txt");
    assert_eq!(c.side, Some(Side::New));
    assert_eq!(c.lines, (3, 3));
    assert_eq!(c.excerpt, "+a3 new");
    assert_eq!(c.body, "why?");
    let info = &app.rows()[0].info;
    assert_eq!(
        c.rev,
        Some(Rev {
            commit: info.commit.clone(),
            change: None,
            summary: Some("change".into()),
        })
    );
}

#[test]
fn removed_only_selection_uses_the_old_side() {
    let f = fixture();
    let mut app = f.open();
    let (a, b) = (removed(&app, "a20"), removed(&app, "a22"));
    select_and_comment(&mut app, a, b);
    type_and_save(&mut app, "gone");
    let c = &f.comments()[0];
    assert_eq!(c.side, Some(Side::Old));
    assert_eq!(c.lines, (20, 22));
    assert_eq!(c.excerpt, "-a20\n-a21\n-a22");
}

#[test]
fn mixed_selection_uses_the_new_side_with_both_in_the_excerpt() {
    let f = fixture();
    let mut app = f.open();
    let (a, b) = (context(&app, "a2"), added(&app, "a3 new"));
    select_and_comment(&mut app, a, b);
    type_and_save(&mut app, "mixed");
    let c = &f.comments()[0];
    assert_eq!(c.side, Some(Side::New));
    assert_eq!(c.lines, (2, 3));
    assert_eq!(c.excerpt, " a2\n-a3\n+a3 new");
}

#[test]
fn same_file_selection_across_two_hunks() {
    let f = fixture();
    let mut app = f.open();
    let (a, b) = (added(&app, "a3 new"), removed(&app, "a20"));
    let between = &app.commit_rows()[a..=b];
    assert!(between.iter().any(|r| r.kind == RowKind::HunkHeader));
    select_and_comment(&mut app, a, b);
    type_and_save(&mut app, "two hunks");
    let c = &f.comments()[0];
    assert_eq!(c.side, Some(Side::New));
    // Line numbers come from the diff lines, not row indices.
    assert_eq!(c.lines, (3, 19));
    assert!(!c.excerpt.contains("@@"), "{}", c.excerpt);
    assert!(c.excerpt.starts_with("+a3 new\n a4\n"), "{}", c.excerpt);
    assert!(c.excerpt.ends_with(" a19\n-a20"), "{}", c.excerpt);
}

#[test]
fn a_file_header_is_refused() {
    let f = fixture();
    let mut app = f.open();
    let r = header(&app, "a.txt");
    goto(&mut app, r);
    keys(&mut app, "cc");
    assert_eq!(app.mode(), Mode::Normal);
    assert_eq!(app.status(), NOT_A_LINE);
    // A selection of only headers and blank rows too.
    let h = header(&app, "b.txt");
    select_and_comment(&mut app, h - 1, h + 1);
    assert_eq!(app.status(), NOT_A_LINE);
    assert!(f.comments().is_empty());
}

#[test]
fn a_selection_across_files_is_refused() {
    let f = fixture();
    let mut app = f.open();
    let (a, b) = (context(&app, "a43"), added(&app, "b2 new"));
    select_and_comment(&mut app, a, b);
    assert_eq!(app.mode(), Mode::Normal);
    assert_eq!(app.status(), SPANS_FILES);
    assert!(f.comments().is_empty());
}

#[test]
fn a_long_selection_keeps_its_range_and_the_first_40_lines() {
    let f = fixture();
    let mut app = f.open();
    let (a, b) = (added(&app, "g1"), added(&app, "g50"));
    assert_eq!(b - a, 49);
    select_and_comment(&mut app, a, b);
    type_and_save(&mut app, "long");
    let c = &f.comments()[0];
    assert_eq!(c.lines, (1, 50));
    let lines: Vec<_> = c.excerpt.lines().collect();
    assert_eq!(lines.len(), EXCERPT_LINES);
    assert_eq!(lines[39], "+g40");
}

#[test]
fn prompt_esc_cancels_and_an_empty_body_is_not_saved() {
    let f = fixture();
    let mut app = f.open();
    let r = added(&app, "a3 new");
    goto(&mut app, r);
    keys(&mut app, "cc");
    keys(&mut app, "q text");
    assert_eq!(app.comment_prompt().as_deref(), Some("comment: q text"));
    press(&mut app, KeyCode::Esc);
    assert_eq!(app.mode(), Mode::Normal);
    assert_eq!(
        app.screen(),
        Screen::Commit(0),
        "Esc only closed the prompt"
    );
    keys(&mut app, "cc");
    keys(&mut app, "  ");
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.status(), EMPTY_COMMENT);
    assert!(f.comments().is_empty());
}

#[test]
fn ctrl_e_moves_the_draft_into_the_editor() {
    let f = fixture();
    let script = f.root.parent().unwrap().join("ed.sh");
    std::fs::write(
        &script,
        "#!/bin/sh\nprintf 'edited: %s' \"$(cat \"$1\")\" > \"$1\"\n",
    )
    .unwrap();
    let mut app = f.open().with_env({
        let s = format!("sh {}", script.display());
        move |k| (k == "EDITOR").then(|| s.clone())
    });
    let r = added(&app, "a3 new");
    goto(&mut app, r);
    keys(&mut app, "cctyped");
    app.key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL));
    assert_eq!(app.mode(), Mode::Normal);
    assert!(app.pending_effect().is_some());
    app.run_pending_effect();
    let cs = f.comments();
    assert_eq!(cs.len(), 1, "{}", app.status());
    assert_eq!(cs[0].body, "edited: typed");
    assert_eq!(cs[0].lines, (3, 3));
}

#[test]
fn gutter_marker_jump_preview_and_popup() {
    let f = fixture();
    let mut app = f.open();
    let row = added(&app, "a40 new");
    goto(&mut app, row);
    keys(&mut app, "cc");
    type_and_save(&mut app, "first line\nsecond");
    assert_eq!(app.review_marked_rows(), [row]);
    let s = screen(&mut app);
    let drawn: Vec<_> = s.iter().filter(|l| l.contains(MARKER)).collect();
    assert_eq!(drawn.len(), 1, "{s:#?}");
    assert!(drawn[0].contains("a40 new"), "{}", drawn[0]);

    keys(&mut app, "gg");
    keys(&mut app, "]r");
    assert_eq!(app.commit_cursor().row, row);
    keys(&mut app, "]r");
    assert_eq!(app.status(), "No more review comments");
    keys(&mut app, "j");
    assert!(app.status().is_empty());
    keys(&mut app, "[r");
    assert_eq!(app.commit_cursor().row, row);
    assert_eq!(app.review_preview().as_deref(), Some("● first line"));
    let s = screen(&mut app);
    assert!(s[29].contains("● first line"), "{}", s[29]);

    keys(&mut app, "K");
    assert_eq!(app.mode(), Mode::Popup);
    assert_eq!(app.popup(), Some("● line 37\nfirst line\nsecond"));
    let s = screen(&mut app);
    assert!(s.iter().any(|l| l.contains("second")), "{s:#?}");
    press(&mut app, KeyCode::Esc);
    assert_eq!(app.mode(), Mode::Normal);
    assert_eq!(app.screen(), Screen::Commit(0));
}

#[test]
fn another_handle_adding_a_comment_shows_after_a_tick() {
    let f = fixture();
    let mut app = f.open();
    assert!(app.review_marked_rows().is_empty());
    let info = app.rows()[0].info.clone();
    let rev = Rev {
        commit: info.commit,
        change: None,
        summary: None,
    };
    f.review()
        .add(Comment::on_diff(
            "b.txt",
            rev,
            Side::New,
            2..=2,
            "+b2 new",
            "from ramble",
        ))
        .unwrap();
    let now = Instant::now();
    app.tick(now);
    app.tick(now + REVIEW_POLL);
    assert_eq!(app.review_marked_rows(), [added(&app, "b2 new")]);
}

#[test]
fn a_comment_on_another_commit_marks_nothing_here() {
    let f = fixture();
    let mut app = f.open();
    let r = added(&app, "a3 new");
    goto(&mut app, r);
    keys(&mut app, "cc");
    type_and_save(&mut app, "x");
    keys(&mut app, "]]");
    assert_eq!(app.screen(), Screen::Commit(1));
    assert!(app.review_marked_rows().is_empty());
    keys(&mut app, "]r");
    assert_eq!(app.status(), "No review comments on this commit");
}

#[test]
fn a_jj_rewrite_keeps_the_anchor_and_draws_no_marker() {
    if !have_jj() {
        return;
    }
    let e = JjEnv::new(false);
    e.write("a.txt", "one\ntwo\n");
    e.jj(&["--quiet", "commit", "-m", "first"]);
    e.write("a.txt", "one\nTWO\n");
    e.jj(&["--quiet", "commit", "-m", "second"]);
    let cache = e.root.parent().unwrap().join("cache");
    let opts = || StartOptions {
        revset: Some("::@- ~ root()".into()),
        ..StartOptions::new(cache.clone())
    };
    let mut app = App::new(vcs::open(&e.root).unwrap(), opts()).unwrap();
    keys(&mut app, "gg");
    press(&mut app, KeyCode::Enter);
    let info = app.rows()[0].info.clone();
    assert_eq!(info.summary, "second");
    let r = added(&app, "TWO");
    goto(&mut app, r);
    keys(&mut app, "cc");
    type_and_save(&mut app, "shout");
    assert_eq!(app.review_marked_rows().len(), 1);
    drop(app);

    e.jj(&["--quiet", "describe", "-r", "@-", "-m", "second, reworded"]);
    let mut app = App::new(vcs::open(&e.root).unwrap(), opts()).unwrap();
    let now = app.rows()[0].info.clone();
    assert_eq!(now.change, info.change, "same change");
    assert_ne!(now.commit, info.commit, "rewritten");
    keys(&mut app, "gg");
    press(&mut app, KeyCode::Enter);
    assert!(app.review_marked_rows().is_empty());
    let cs = Review::open(&cache, &e.root).unwrap().comments().to_vec();
    assert_eq!(cs.len(), 1);
    let rev = cs[0].rev.as_ref().unwrap();
    assert_eq!(rev.commit, info.commit, "original anchor kept");
    assert_eq!(rev.change, info.change);
    assert_eq!(rev.summary.as_deref(), Some("second"));
    assert_eq!(cs[0].excerpt, "+TWO");
}
