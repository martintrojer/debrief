//! Comments on diff lines: capture (`cc`, visual `c`), the prompt, the
//! gutter, `]r`, `K`, polling the shared batch, and a jj rewrite. A temp
//! repo and a temp cache dir per test; nothing sleeps.

mod common;

use std::path::{Path, PathBuf};
use std::time::Instant;

use common::{JjEnv, git_repo, have_jj, w};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use debrief::app::comment::{EMPTY_COMMENT, EXCERPT_LINES, NOT_A_LINE, SPANS_FILES};
use debrief::app::review::{MARKER, REVIEW_POLL};
use debrief::app::{App, AppEvent, CommentBox, Mode, RowKind, Screen, StartOptions};
use debrief::vcs::{self, LineKind};
use debrief_review::{Comment, Rev, Review, Side};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;

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
    let a: Vec<String> = (1..=60).map(|i| format!("a{i}")).collect();
    let base = vec![w("a.txt", a.join("\n") + "\n"), w("b.txt", "b1\nb2\n")];
    let mut a = a;
    a[39] = "a40 new".into();
    a.drain(19..22);
    a[2] = "a3 new".into();
    let big: String = (1..=50).map(|i| format!("g{i}\n")).collect();
    let change = vec![
        w("a.txt", a.join("\n") + "\n"),
        w("b.txt", "b1\nb2 new\n"),
        w("big.txt", big),
    ];
    git_repo(&root, &[("base", base), ("change", change)]);
    Fixture {
        root: root.canonicalize().unwrap(),
        cache: tmp.path().join("cache"),
        _tmp: tmp,
    }
}

fn app_on(root: &Path, cache: &Path) -> App {
    App::new(
        vcs::open(root, vcs::OpenOptions::default()).unwrap(),
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
    assert_eq!(app.comment_text(), Some(""));
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
            // The log lists the checked-out branch; the comment records it.
            branch: app.branch_name().map(str::to_string),
        })
    );
    assert!(app.branch_name().is_some(), "on a branch");
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
    assert_eq!(app.comment_text(), Some("q text"));
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
fn a_long_comment_in_the_popup_wraps_and_scrolls_to_its_tail() {
    let f = fixture();
    let mut app = f.open();
    let row = added(&app, "a40 new");
    goto(&mut app, row);
    keys(&mut app, "cc");
    let body = format!("{}TAIL_SENTINEL", "word ".repeat(600));
    type_and_save(&mut app, &body);
    keys(&mut app, "K");
    let s = screen(&mut app);
    let v = app.popup_view().unwrap();
    // 29 rows above the status line.
    assert_eq!(v.rect.height, 29 - 2, "capped at the screen height minus 2");
    assert!(v.max_scroll > 0);
    assert!(!s.iter().any(|l| l.contains("TAIL_SENTINEL")), "{s:#?}");
    keys(&mut app, "G");
    let s = screen(&mut app);
    assert!(s.iter().any(|l| l.contains("TAIL_SENTINEL")), "{s:#?}");
    keys(&mut app, "gg");
    assert_eq!(app.popup_view().unwrap().scroll, 0);
    keys(&mut app, "j");
    assert_eq!(app.popup_view().unwrap().scroll, 1);
    press_ctrl(&mut app, 'd');
    let half = app.popup_view().unwrap().scroll;
    assert!(half > 1);
    press_ctrl(&mut app, 'u');
    assert_eq!(app.popup_view().unwrap().scroll, 1);
    keys(&mut app, "k");
    assert_eq!(app.popup_view().unwrap().scroll, 0);
    keys(&mut app, "q");
    assert_eq!(app.mode(), Mode::Normal);
}

fn press_ctrl(app: &mut App, c: char) {
    app.key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL));
}

#[test]
fn gutter_marker_jump_preview_and_popup() {
    let f = fixture();
    // A two-line body comes from the editor; the prompt is one line.
    let script = f.root.parent().unwrap().join("ed.sh");
    std::fs::write(
        &script,
        "#!/bin/sh\nprintf 'first line\\nsecond' > \"$1\"\n",
    )
    .unwrap();
    let mut app = f.open().with_env({
        let s = format!("sh {}", script.display());
        move |k| (k == "EDITOR").then(|| s.clone())
    });
    let row = added(&app, "a40 new");
    goto(&mut app, row);
    keys(&mut app, "cc");
    press_ctrl(&mut app, 'e');
    app.run_pending_effect();
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
        branch: None,
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
    let mut app = App::new(
        vcs::open(&e.root, vcs::OpenOptions::default()).unwrap(),
        opts(),
    )
    .unwrap();
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
    let mut app = App::new(
        vcs::open(&e.root, vcs::OpenOptions::default()).unwrap(),
        opts(),
    )
    .unwrap();
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

// <leader>rl and <leader>rr.

fn app_with_send(f: &Fixture, command: &[&str]) -> App {
    let mut opts = StartOptions::new(f.cache.clone());
    opts.config.send.command = command.iter().map(|s| s.to_string()).collect();
    App::new(
        vcs::open(&f.root, vcs::OpenOptions::default()).unwrap(),
        opts,
    )
    .unwrap()
}

/// A diff comment on `commit` from another handle.
fn add_diff(f: &Fixture, commit: &str, path: &str, line: u32, body: &str) -> u64 {
    let rev = Rev {
        commit: commit.into(),
        change: None,
        summary: None,
        branch: None,
    };
    f.review()
        .add(Comment::on_diff(
            path,
            rev,
            Side::New,
            line..=line,
            "+x",
            body,
        ))
        .unwrap()
}

#[test]
fn the_picker_lists_the_whole_batch_and_goes_to_a_comment() {
    let f = fixture();
    let mut app = f.open();
    let top = app.rows()[0].info.commit.clone();
    add_diff(&f, &top, "a.txt", 37, "on a40");
    f.review()
        .add(Comment::on_file("notes.txt", 4..=5, "x", "from ramble"))
        .unwrap();
    keys(&mut app, "  ");
    assert_eq!(app.mode(), Mode::Normal, "<leader><leader> is unbound");
    keys(&mut app, " rl");
    assert_eq!(app.mode(), Mode::Picker);
    let v = app.picker().unwrap();
    let labels: Vec<_> = v.items.iter().map(|i| i.label.clone()).collect();
    assert_eq!(
        labels,
        [format!("a.txt:37 @{}", &top[..8]), "notes.txt:4-5".into()]
    );
    assert!(screen(&mut app).iter().any(|l| l.contains("from ramble")));

    // Typed text filters.
    keys(&mut app, "ramble");
    assert_eq!(app.picker().unwrap().items.len(), 1);
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.mode(), Mode::Normal);
    assert_eq!(app.status(), "Working-copy comment: notes.txt:4");

    // Enter on the diff comment, from another commit: opens its commit at the line.
    keys(&mut app, "]]");
    assert_eq!(app.screen(), Screen::Commit(1));
    keys(&mut app, " rl");
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.screen(), Screen::Commit(0));
    assert_eq!(app.commit_cursor().row, added(&app, "a40 new"));
}

#[test]
fn picker_enter_on_a_ramble_md_comment_hands_it_to_ramble() {
    let f = fixture();
    std::fs::write(f.root.join("plan.md"), "# plan\n").unwrap();
    f.review()
        .add(Comment::on_file("plan.md", 3..=4, "# plan", "tighten"))
        .unwrap();
    let mut app = f.open();
    keys(&mut app, " rl");
    press(&mut app, KeyCode::Enter);
    match app.pending_effect() {
        Some(debrief::app::Effect::Launch(l)) => {
            assert_eq!(l.name, "md");
            assert_eq!(
                l.argv,
                [
                    "ramble".to_string(),
                    "--line".to_string(),
                    "3".to_string(),
                    f.root.join("plan.md").display().to_string()
                ]
            );
        }
        other => panic!("{other:?} {}", app.status()),
    }
}

#[test]
fn picker_enter_on_a_commit_not_in_the_log_keeps_the_comment() {
    let f = fixture();
    add_diff(&f, &"f".repeat(40), "a.txt", 1, "stale");
    let mut app = f.open();
    keys(&mut app, " rl");
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.status(), "Comment's revision is not in the log");
    assert_eq!(app.screen(), Screen::Commit(0));
    assert_eq!(f.comments().len(), 1);
}

#[test]
fn picker_remove_and_edit() {
    let f = fixture();
    let app = f.open();
    let top = app.rows()[0].info.commit.clone();
    add_diff(&f, &top, "a.txt", 3, "one");
    add_diff(&f, &top, "b.txt", 2, "two");
    let script = f.root.parent().unwrap().join("ed.sh");
    std::fs::write(&script, "#!/bin/sh\nprintf 'better' > \"$1\"\n").unwrap();
    let s = format!("sh {}", script.display());
    let mut app = app.with_env(move |k| (k == "VISUAL").then(|| s.clone()));
    keys(&mut app, " rl");
    app.key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL));
    assert_eq!(app.status(), "Comment removed (1 in batch)");
    assert_eq!(app.picker().unwrap().items.len(), 1);
    app.key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL));
    app.run_pending_effect();
    assert_eq!(app.status(), "Comment edited");
    let cs = f.comments();
    assert_eq!(cs.len(), 1);
    assert_eq!(
        (cs[0].path.as_str(), cs[0].body.as_str()),
        ("b.txt", "better")
    );
    assert_eq!(app.picker().unwrap().items[0].detail, "better");
    press(&mut app, KeyCode::Esc);
    assert_eq!(app.mode(), Mode::Normal);
    assert_eq!(app.screen(), Screen::Commit(0));
}

#[test]
fn send_with_nothing_says_so() {
    let f = fixture();
    let mut app = f.open();
    keys(&mut app, " rr");
    assert_eq!(app.pending_effect(), None);
    assert_eq!(app.status(), "No comments to send");
}

#[test]
fn send_pipes_the_markdown_and_removes_what_it_sent() {
    let f = fixture();
    let out = f.root.parent().unwrap().join("sent.md");
    let cmd = format!("cat > '{}'", out.display());
    let mut app = app_with_send(&f, &["sh", "-c", &cmd]);
    keys(&mut app, "gg");
    press(&mut app, KeyCode::Enter);
    let r = added(&app, "a3 new");
    goto(&mut app, r);
    keys(&mut app, "cc");
    type_and_save(&mut app, "please explain");
    // From the log screen too.
    press(&mut app, KeyCode::Esc);
    assert_eq!(app.screen(), Screen::Log);
    keys(&mut app, " rr");
    assert!(app.pending_effect().is_some());
    app.run_pending_effect();
    assert_eq!(app.status(), "Review sent (1 comments)");
    let md = std::fs::read_to_string(&out).unwrap();
    assert!(md.starts_with("# Review feedback"), "{md}");
    assert!(md.contains("- File: a.txt (new side)"), "{md}");
    assert!(md.contains("please explain"), "{md}");
    assert!(f.comments().is_empty());
    assert!(app.take_output().is_empty(), "no clipboard");
}

#[test]
fn a_failing_command_copies_to_the_clipboard() {
    let f = fixture();
    let written = std::rc::Rc::new(std::cell::RefCell::new(Vec::<u8>::new()));
    let sink = written.clone();
    let mut app = app_with_send(&f, &["false"]).with_terminal(move |b| {
        sink.borrow_mut().extend_from_slice(b);
        Ok(())
    });
    let top = app.rows()[0].info.commit.clone();
    add_diff(&f, &top, "a.txt", 3, "x");
    keys(&mut app, " rr");
    app.run_pending_effect();
    assert_eq!(app.status(), "Review copied to clipboard (1 comments)");
    let out = String::from_utf8(written.borrow().clone()).unwrap();
    assert!(out.starts_with("\x1b]52;c;"), "{out:?}");
    assert!(app.take_output().is_empty(), "written directly, not queued");
    assert!(f.comments().is_empty());
}

#[test]
fn a_failing_clipboard_write_keeps_the_batch() {
    let f = fixture();
    let mut app = app_with_send(&f, &["false"])
        .with_terminal(|_| Err(std::io::Error::from(std::io::ErrorKind::BrokenPipe)));
    let top = app.rows()[0].info.commit.clone();
    add_diff(&f, &top, "a.txt", 3, "x");
    keys(&mut app, " rr");
    app.run_pending_effect();
    assert!(
        app.status().starts_with("Review not sent: "),
        "{}",
        app.status()
    );
    assert!(app.status().contains("roken pipe"), "{}", app.status());
    assert_eq!(f.comments().len(), 1, "batch kept");
}

#[test]
fn a_comment_added_during_a_send_survives() {
    let f = fixture();
    let app = app_with_send(&f, &["true"]);
    let top = app.rows()[0].info.commit.clone();
    let sent = add_diff(&f, &top, "a.txt", 3, "sent");
    let (cache, root, commit) = (f.cache.clone(), f.root.clone(), top.clone());
    let mut app = app.with_after_export(move || {
        let rev = Rev {
            commit: commit.clone(),
            change: None,
            summary: None,
            branch: None,
        };
        Review::open(&cache, &root)
            .unwrap()
            .add(Comment::on_diff(
                "b.txt",
                rev,
                Side::New,
                2..=2,
                "+b2",
                "late",
            ))
            .unwrap();
    });
    keys(&mut app, " rr");
    app.run_pending_effect();
    assert_eq!(app.status(), "Review sent (1 comments)");
    let cs = f.comments();
    assert_eq!(cs.len(), 1);
    assert_ne!(cs[0].id, sent);
    assert_eq!(cs[0].body, "late");
}

#[test]
fn an_open_picker_follows_the_batch_on_the_tick() {
    let f = fixture();
    let mut app = f.open();
    keys(&mut app, " rl");
    assert!(app.picker().unwrap().items.is_empty());
    let top = app.rows()[0].info.commit.clone();
    add_diff(&f, &top, "a.txt", 3, "arrived");
    let now = Instant::now();
    app.tick(now + REVIEW_POLL);
    let v = app.picker().unwrap();
    assert_eq!(v.items.len(), 1);
    assert_eq!(v.items[0].detail, "arrived");
}

// Bracketed paste.

fn paste(app: &mut App, text: &str) {
    app.event(AppEvent::Paste(text.into()));
}

#[test]
fn a_multi_line_paste_keeps_its_newlines_in_the_comment_box() {
    let f = fixture();
    let mut app = f.open();
    let r = added(&app, "a3 new");
    goto(&mut app, r);
    keys(&mut app, "cc");
    keys(&mut app, "see ");
    paste(&mut app, "first\r\nsecond\tthird\rG");
    assert_eq!(app.mode(), Mode::Comment, "the newline ran no Enter");
    assert_eq!(app.comment_text(), Some("see first\nsecond third\nG"));
    assert_eq!(app.commit_cursor().row, r, "no key ran");
    press(&mut app, KeyCode::Enter);
    assert_eq!(f.comments()[0].body, "see first\nsecond third\nG");
}

#[test]
fn a_paste_lands_in_the_search_prompt_and_the_picker_filter() {
    let f = fixture();
    let mut app = f.open();
    keys(&mut app, "/");
    paste(&mut app, "a40\nnew");
    assert_eq!(app.search_prompt().as_deref(), Some("/a40 new"));
    assert_eq!(app.mode(), Mode::Search);
    assert!(!app.search_hits().is_empty());
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.commit_cursor().row, added(&app, "a40 new"));

    f.review()
        .add(Comment::on_file("notes.txt", 1..=1, "x", "from ramble"))
        .unwrap();
    f.review()
        .add(Comment::on_file("other.txt", 1..=1, "x", "else"))
        .unwrap();
    keys(&mut app, " rl");
    paste(&mut app, "from\nramble");
    let v = app.picker().unwrap();
    assert_eq!(v.input, "from ramble");
    assert_eq!(v.items.len(), 1);
    assert_eq!(app.mode(), Mode::Picker);
}

#[test]
fn a_paste_outside_a_prompt_does_nothing() {
    let f = fixture();
    let mut app = f.open();
    let row = app.commit_cursor().row;
    paste(&mut app, "Gcc\rq");
    assert_eq!(app.mode(), Mode::Normal);
    assert_eq!(app.screen(), Screen::Commit(0));
    assert_eq!(app.commit_cursor().row, row);
    press(&mut app, KeyCode::Esc);
    paste(&mut app, "q\n");
    assert!(!app.should_quit());
    assert_eq!(app.screen(), Screen::Log);
}

// Comment kinds.

fn app_with_kinds(f: &Fixture, toml: &str) -> App {
    let mut opts = StartOptions::new(f.cache.clone());
    opts.config = debrief::app::config::Config::parse(toml).unwrap();
    let mut app = App::new(
        vcs::open(&f.root, vcs::OpenOptions::default()).unwrap(),
        opts,
    )
    .unwrap();
    keys(&mut app, "gg");
    press(&mut app, KeyCode::Enter);
    app
}

#[test]
fn tab_cycles_the_kind_and_the_comment_saves_it() {
    let f = fixture();
    let mut app = f.open();
    let r = added(&app, "a3 new");
    goto(&mut app, r);
    keys(&mut app, "cc");
    let prompt = |app: &App| app.comment_title().unwrap();
    assert_eq!(prompt(&app), " comment L3 ");
    let labels: Vec<String> = (0..5)
        .map(|_| {
            press(&mut app, KeyCode::Tab);
            prompt(&app)
        })
        .collect();
    assert_eq!(
        labels,
        [
            " comment [ISSUE] L3 ",
            " comment [SUGGESTION] L3 ",
            " comment [QUESTION] L3 ",
            " comment [NIT] L3 ",
            " comment L3 ",
        ]
    );
    press(&mut app, KeyCode::BackTab);
    assert_eq!(prompt(&app), " comment [NIT] L3 ");
    press(&mut app, KeyCode::BackTab);
    press(&mut app, KeyCode::BackTab);
    assert_eq!(prompt(&app), " comment [SUGGESTION] L3 ");
    keys(&mut app, "try this");
    press(&mut app, KeyCode::Enter);
    assert_eq!(f.comments()[0].kind.as_deref(), Some("suggestion"));
    assert_eq!(f.comments()[0].body, "try this");

    // The next comment starts untyped.
    keys(&mut app, "cc");
    assert_eq!(prompt(&app), " comment L3 ");
    type_and_save(&mut app, "plain");
    assert_eq!(f.comments()[1].kind, None);
}

#[test]
fn configured_kinds_replace_the_builtins_and_none_make_tab_inert() {
    let f = fixture();
    let mut app = app_with_kinds(&f, "[review]\nkinds = [{ id = \"praise\" }]\n");
    let r = added(&app, "a3 new");
    goto(&mut app, r);
    keys(&mut app, "cc");
    press(&mut app, KeyCode::Tab);
    assert_eq!(app.comment_title().unwrap(), " comment [PRAISE] L3 ");
    press(&mut app, KeyCode::Tab);
    assert_eq!(app.comment_title().unwrap(), " comment L3 ");

    let mut app = app_with_kinds(&f, "[review]\nkinds = []\n");
    goto(&mut app, r);
    keys(&mut app, "cc");
    press(&mut app, KeyCode::Tab);
    press(&mut app, KeyCode::BackTab);
    assert_eq!(app.comment_title().unwrap(), " comment L3 ");
    type_and_save(&mut app, "untyped");
    assert_eq!(f.comments()[0].kind, None);
}

#[test]
fn ctrl_e_keeps_the_kind() {
    let f = fixture();
    let script = f.root.parent().unwrap().join("ed.sh");
    std::fs::write(&script, "#!/bin/sh\nprintf 'from the editor' > \"$1\"\n").unwrap();
    let mut app = f.open().with_env({
        let s = format!("sh {}", script.display());
        move |k| (k == "EDITOR").then(|| s.clone())
    });
    let r = added(&app, "a3 new");
    goto(&mut app, r);
    keys(&mut app, "cc");
    press(&mut app, KeyCode::Tab);
    press_ctrl(&mut app, 'e');
    app.run_pending_effect();
    let c = &f.comments()[0];
    assert_eq!(c.body, "from the editor");
    assert_eq!(c.kind.as_deref(), Some("issue"));
}

#[test]
fn kinds_show_in_the_picker_the_popup_and_the_preview() {
    let f = fixture();
    let mut app = f.open();
    let row = added(&app, "a40 new");
    goto(&mut app, row);
    keys(&mut app, "cc");
    press(&mut app, KeyCode::Tab);
    type_and_save(&mut app, "broken");
    assert_eq!(app.review_preview().as_deref(), Some("● [ISSUE] broken"));
    keys(&mut app, "K");
    assert_eq!(app.popup(), Some("● line 37 [ISSUE]\nbroken"));
    press(&mut app, KeyCode::Esc);
    keys(&mut app, " rl");
    assert_eq!(app.picker().unwrap().items[0].detail, "[ISSUE] broken");
    assert!(
        screen(&mut app)
            .iter()
            .any(|l| l.contains("[ISSUE] broken"))
    );
    // The label filters too.
    keys(&mut app, "issue");
    assert_eq!(app.picker().unwrap().items.len(), 1);
}

#[test]
fn send_exports_the_label_and_a_legend_of_the_configured_kinds() {
    let f = fixture();
    let out = f.root.parent().unwrap().join("sent.md");
    let cmd = format!("cat > '{}'", out.display());
    let mut opts = StartOptions::new(f.cache.clone());
    opts.config = debrief::app::config::Config::parse(&format!(
        "[send]\ncommand = [\"sh\", \"-c\", \"{}\"]\n\
         [review]\nkinds = [{{ id = \"praise\", definition = \"Keep doing this.\" }}, {{ id = \"issue\" }}]\n",
        cmd.replace('"', "\\\"")
    ))
    .unwrap();
    let mut app = App::new(
        vcs::open(&f.root, vcs::OpenOptions::default()).unwrap(),
        opts,
    )
    .unwrap();
    keys(&mut app, "gg");
    press(&mut app, KeyCode::Enter);
    let r = added(&app, "a3 new");
    goto(&mut app, r);
    keys(&mut app, "cc");
    press(&mut app, KeyCode::Tab);
    type_and_save(&mut app, "nice");
    keys(&mut app, " rr");
    app.run_pending_effect();
    assert_eq!(app.status(), "Review sent (1 comments)");
    let md = std::fs::read_to_string(&out).unwrap();
    assert!(md.contains("## Item 1 [PRAISE]"), "{md}");
    assert!(md.contains("- PRAISE: Keep doing this."), "{md}");
    assert!(!md.contains("ISSUE"), "only used kinds: {md}");
}

// Editing in the prompt (the shared text box).

fn press_mod(app: &mut App, code: KeyCode, m: KeyModifiers) {
    app.key(KeyEvent::new(code, m));
}

#[test]
fn the_prompt_edits_at_the_cursor() {
    let f = fixture();
    let mut app = f.open();
    let r = added(&app, "a3 new");
    goto(&mut app, r);
    keys(&mut app, "cc");
    keys(&mut app, "why this here");
    press_mod(&mut app, KeyCode::Char('b'), KeyModifiers::ALT);
    press_mod(&mut app, KeyCode::Char('b'), KeyModifiers::ALT);
    keys(&mut app, "is ");
    assert_eq!(app.comment_text().unwrap(), "why is this here");
    press_ctrl(&mut app, 'a');
    press(&mut app, KeyCode::Delete);
    keys(&mut app, "W");
    press(&mut app, KeyCode::End);
    press_ctrl(&mut app, 'w');
    keys(&mut app, "there?");
    assert_eq!(app.comment_text().unwrap(), "Why is this there?");
    press(&mut app, KeyCode::Home);
    press_mod(&mut app, KeyCode::Char('f'), KeyModifiers::ALT);
    press(&mut app, KeyCode::Right);
    press(&mut app, KeyCode::Right);
    press_ctrl(&mut app, 'u');
    press(&mut app, KeyCode::Left);
    press(&mut app, KeyCode::Backspace);
    assert_eq!(app.comment_text().unwrap(), " this there?");
    press(&mut app, KeyCode::Delete);
    // Pasted text goes in at the cursor too.
    paste(&mut app, "is ");
    assert_eq!(app.comment_text().unwrap(), "is this there?");
    press(&mut app, KeyCode::Enter);
    assert_eq!(f.comments()[0].body, "is this there?");
}

// The comment box.

/// Draw at `w`x`h`; the screen's lines, the box (as the app places it)
/// and the terminal cursor.
fn draw_box(app: &mut App, w: u16, h: u16) -> (Vec<String>, Option<CommentBox>, (u16, u16)) {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| app.render(f)).unwrap();
    let buf = t.backend().buffer().clone();
    let lines = (0..h)
        .map(|y| (0..w).map(|x| buf[(x, y)].symbol()).collect::<String>())
        .collect();
    let pane = app.commit_layout(Rect::new(0, 0, w, h - 1)).diff;
    let pos = t.get_cursor_position().unwrap();
    (lines, app.comment_box(pane), (pos.x, pos.y))
}

/// Screen row of diff row `row` (below the header row while it shows).
fn on_screen(app: &App, row: usize) -> u16 {
    let top = u16::from(app.config().view.header);
    top + (row - app.commit_scroll().0) as u16
}

#[test]
fn the_box_opens_below_a_selection_in_the_middle_and_keeps_it_highlighted() {
    let f = fixture();
    let mut app = f.open();
    let (a, b) = (context(&app, "a2"), added(&app, "a3 new"));
    select_and_comment(&mut app, a, b);
    let (lines, bx, cur) = draw_box(&mut app, 100, 30);
    let bx = bx.expect("box open");
    let pane = app.commit_layout(Rect::new(0, 0, 100, 29)).diff;
    let (ya, yb) = (on_screen(&app, a), on_screen(&app, b));
    assert_eq!(bx.rect.y, yb + 1, "directly below the last row");
    assert_eq!(bx.rect.width, pane.width - 2, "the pane less a margin");
    assert_eq!(bx.rect.x, pane.x + 1);
    assert_eq!(bx.rect.height, 3, "one text row");
    assert_eq!(bx.title, " comment L2-3 ");
    let top = &lines[bx.rect.y as usize];
    assert!(top.contains("╭ comment L2-3 "), "{top}");
    assert!(lines[bx.rect.y as usize + 2].contains("Enter save"));
    // The rows stay drawn and selected while typing.
    assert!(lines[ya as usize].contains("a2"));
    assert!(lines[yb as usize].contains("a3 new"));
    let mut t = Terminal::new(TestBackend::new(100, 30)).unwrap();
    t.draw(|f| app.render(f)).unwrap();
    let sel = t.backend().buffer()[(pane.x + 2, yb)].bg;
    let other = t.backend().buffer()[(pane.x + 2, yb + 5)].bg;
    assert_ne!(sel, other, "selected background");
    assert_eq!(sel, t.backend().buffer()[(pane.x + 2, ya)].bg);
    keys(&mut app, "ab");
    let (lines, bx, cur2) = draw_box(&mut app, 100, 30);
    let bx = bx.unwrap();
    assert_eq!(cur, (bx.rect.x + 1, bx.rect.y + 1), "cursor in the box");
    assert_eq!(cur2, (bx.rect.x + 3, bx.rect.y + 1));
    assert!(lines[bx.rect.y as usize + 1].contains("│ab"));
}

#[test]
fn the_box_opens_above_rows_at_the_bottom() {
    let f = fixture();
    let mut app = f.open();
    app.event(AppEvent::Resize(100, 30));
    keys(&mut app, "G");
    let r = app.commit_cursor().row;
    keys(&mut app, "cc");
    assert_eq!(app.mode(), Mode::Comment, "{}", app.status());
    let (lines, bx, _) = draw_box(&mut app, 100, 30);
    let bx = bx.unwrap();
    let y = on_screen(&app, r);
    assert_eq!(y, 28, "the last diff row is at the bottom");
    assert_eq!(bx.rect.bottom(), y, "directly above the row");
    assert!(lines[y as usize].contains("g50"), "the row is not covered");
}

#[test]
fn a_multi_line_body_is_saved_with_its_newlines_and_the_box_grows() {
    let f = fixture();
    let mut app = f.open();
    let r = added(&app, "a3 new");
    goto(&mut app, r);
    keys(&mut app, "cc");
    keys(&mut app, "one");
    press_ctrl(&mut app, 'j');
    keys(&mut app, "two");
    press_mod(&mut app, KeyCode::Enter, KeyModifiers::ALT);
    keys(&mut app, "three");
    let (_, bx, cur) = draw_box(&mut app, 100, 30);
    let bx = bx.unwrap();
    assert_eq!(bx.lines, ["one", "two", "three"]);
    assert_eq!(bx.rect.height, 5);
    assert_eq!(cur, (bx.rect.x + 6, bx.rect.y + 3));
    // Up moves by row.
    press(&mut app, KeyCode::Up);
    keys(&mut app, "!");
    assert_eq!(app.comment_text(), Some("one\ntwo!\nthree"));
    // Up to 8 rows, then it scrolls with the cursor.
    press(&mut app, KeyCode::Down);
    for i in 0..9 {
        press(&mut app, KeyCode::End);
        press_ctrl(&mut app, 'j');
        keys(&mut app, &format!("l{i}"));
    }
    let (_, bx, cur) = draw_box(&mut app, 100, 30);
    let bx = bx.unwrap();
    assert_eq!(bx.rect.height, 10, "8 text rows");
    assert_eq!(bx.lines.last().map(String::as_str), Some("l8"));
    assert_eq!(cur.1, bx.rect.y + 8, "cursor on the last shown row");
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.mode(), Mode::Normal);
    let body = &f.comments()[0].body;
    assert!(body.starts_with("one\ntwo!\nthree\nl0\n"), "{body:?}");
    assert_eq!(body.lines().count(), 12);
}

#[test]
fn esc_closes_the_box_and_drops_the_highlight() {
    let f = fixture();
    let mut app = f.open();
    let (a, b) = (context(&app, "a2"), added(&app, "a3 new"));
    select_and_comment(&mut app, a, b);
    keys(&mut app, "draft");
    press(&mut app, KeyCode::Esc);
    let (lines, bx, _) = draw_box(&mut app, 100, 30);
    assert!(bx.is_none());
    assert!(app.comment_rows().is_none());
    assert!(!lines.iter().any(|l| l.contains("comment L")));
    assert!(f.comments().is_empty());
}

#[test]
fn the_box_follows_a_resize() {
    let f = fixture();
    let mut app = f.open();
    let r = added(&app, "a40 new");
    goto(&mut app, r);
    keys(&mut app, "cc");
    keys(&mut app, &"word ".repeat(20));
    let (_, wide, _) = draw_box(&mut app, 100, 30);
    let wide = wide.unwrap();
    app.event(AppEvent::Resize(50, 12));
    let (lines, narrow, _) = draw_box(&mut app, 50, 12);
    let narrow = narrow.unwrap();
    let pane = app.commit_layout(Rect::new(0, 0, 50, 11)).diff;
    assert_eq!(narrow.rect.width, pane.width - 2);
    assert!(narrow.rect.height > wide.rect.height, "rewrapped taller");
    let y = on_screen(&app, r);
    assert!(
        narrow.rect.bottom() <= y || narrow.rect.y > y,
        "the row is not covered: {:?} row {y}",
        narrow.rect
    );
    assert!(lines[y as usize].contains("a40 new"));
    assert!(narrow.rect.bottom() <= 11, "inside the pane");
    press(&mut app, KeyCode::Enter);
    assert_eq!(f.comments()[0].body, "word ".repeat(20).trim());
}

/// The box covers none of rows `lo..=hi` that are on screen.
fn off_the_rows(app: &App, bx: &CommentBox, lo: usize, hi: usize, pane_y: u16) -> bool {
    let s = app.commit_scroll().0;
    (lo.max(s)..=hi).all(|r| {
        let y = pane_y + (r - s) as u16;
        y < bx.rect.y || y >= bx.rect.bottom()
    })
}

#[test]
fn a_selection_taller_than_the_screen_shows_its_last_rows_above_the_box() {
    let f = fixture();
    let mut app = f.open();
    let (a, b) = (added(&app, "g1"), added(&app, "g50"));
    app.event(AppEvent::Resize(60, 12));
    select_and_comment(&mut app, a, b);
    assert_eq!(app.mode(), Mode::Comment, "{}", app.status());
    let (lines, bx, _) = draw_box(&mut app, 60, 12);
    let bx = bx.unwrap();
    let pane = app.commit_layout(Rect::new(0, 0, 60, 11)).diff;
    assert!(off_the_rows(&app, &bx, a, b, pane.y), "{:?}", bx.rect);
    let y = on_screen(&app, b);
    assert_eq!(bx.rect.y, y + 1, "the last row just above the box");
    assert!(lines[y as usize].contains("g50"), "{}", lines[y as usize]);
    // The box grows; the rows move up out of its way.
    for _ in 0..3 {
        press_ctrl(&mut app, 'j');
    }
    let (lines, bx, _) = draw_box(&mut app, 60, 12);
    let bx = bx.unwrap();
    assert_eq!(bx.rect.height, 6);
    assert!(off_the_rows(&app, &bx, a, b, pane.y), "{:?}", bx.rect);
    assert!(lines[bx.rect.y as usize - 1].contains("g50"));
}

#[test]
fn a_one_row_comment_at_heights_6_and_8_never_covers_its_row() {
    let f = fixture();
    let mut app = f.open();
    let first = added(&app, "g1");
    for (w, h) in [(40, 6), (100, 6), (40, 8), (100, 8)] {
        app.event(AppEvent::Resize(w, h));
        for r in first..first + 12 {
            for (anchor, body) in [("zt", 1), ("zz", 1), ("zb", 1), ("zz", 3), ("zb", 3)] {
                goto(&mut app, r);
                keys(&mut app, anchor);
                keys(&mut app, "cc");
                assert_eq!(app.mode(), Mode::Comment, "{}", app.status());
                // A three-line body: a 5-row box.
                for _ in 1..body {
                    press_ctrl(&mut app, 'j');
                }
                let (lines, bx, _) = draw_box(&mut app, w, h);
                let bx = bx.unwrap();
                let pane = app.commit_layout(Rect::new(0, 0, w, h - 1)).diff;
                assert!(
                    off_the_rows(&app, &bx, r, r, pane.y),
                    "{w}x{h} row {r} {anchor} body {body}: {:?} scroll {}",
                    bx.rect,
                    app.commit_scroll().0
                );
                let y = on_screen(&app, r);
                let text = &app.commit_rows()[r].text;
                assert!(lines[y as usize].contains(text.as_str()));
                press(&mut app, KeyCode::Esc);
            }
        }
    }
}

#[test]
fn page_keys_scroll_the_diff_behind_the_box_which_follows_its_rows() {
    let f = fixture();
    let mut app = f.open();
    app.event(AppEvent::Resize(60, 12));
    let r = added(&app, "g20");
    goto(&mut app, r);
    keys(&mut app, "zz");
    keys(&mut app, "ccdraft");
    let (_, bx, _) = draw_box(&mut app, 60, 12);
    let before = app.commit_scroll().0;
    assert_eq!(bx.unwrap().rect.y, on_screen(&app, r) + 1, "below the row");
    // Down a page: the row scrolls off the top, the box pins there.
    press(&mut app, KeyCode::PageDown);
    assert_eq!(app.mode(), Mode::Comment);
    let after = app.commit_scroll().0;
    assert!(after > r, "scrolled past the row: {before} -> {after}");
    let (lines, bx, _) = draw_box(&mut app, 60, 12);
    let bx = bx.unwrap();
    let top = app.commit_layout(Rect::new(0, 0, 60, 11)).diff.y;
    assert_eq!(bx.rect.y, top, "pinned to the top edge");
    let l = &lines[top as usize + 1];
    assert!(l.contains("draft"), "{l}");
    // Back up: the box is beside its row again, the text kept.
    press(&mut app, KeyCode::PageUp);
    let (_, bx, _) = draw_box(&mut app, 60, 12);
    let bx = bx.unwrap();
    assert!(app.commit_scroll().0 <= r);
    let y = on_screen(&app, r);
    assert!(
        bx.rect.y == y + 1 || bx.rect.bottom() == y,
        "{:?} row {y}",
        bx.rect
    );
    assert_eq!(app.comment_text(), Some("draft"));
    press(&mut app, KeyCode::Enter);
    assert_eq!(f.comments()[0].body, "draft");
}

#[test]
fn the_box_is_drawn_over_everything_with_the_file_list_shown_and_resized() {
    let f = fixture();
    let mut app = f.open();
    app.event(AppEvent::Resize(100, 30));
    assert!(
        app.commit_layout(Rect::new(0, 0, 100, 29))
            .sidebar
            .is_some()
    );
    let r = added(&app, "a3 new");
    goto(&mut app, r);
    keys(&mut app, "ccdraft");
    for (w, h) in [(100, 30), (60, 12), (40, 12)] {
        app.event(AppEvent::Resize(w, h));
        assert_eq!(app.mode(), Mode::Comment);
        let (lines, bx, cur) = draw_box(&mut app, w, h);
        let bx = bx.unwrap();
        let top = &lines[bx.rect.y as usize];
        assert!(top.contains("╭ comment L3 "), "{w}x{h} title: {top}");
        let body = &lines[bx.rect.y as usize + 1];
        assert!(body.contains("│draft"), "{w}x{h} draft: {body}");
        assert_eq!(cur, (bx.rect.x + 6, bx.rect.y + 1), "{w}x{h} cursor");
    }
}

/// The box in the diff pane after a scroll, at `w`x`h`: off every selected
/// row on screen unless rows `lo..=hi` fill the pane (then at its bottom);
/// the cursor on screen and off the box. The scroll.
fn check_after_scroll(app: &mut App, w: u16, h: u16, lo: usize, hi: usize) -> usize {
    let (_, bx, _) = draw_box(app, w, h);
    let bx = bx.unwrap();
    let pane = app.commit_layout(Rect::new(0, 0, w, h - 1)).diff;
    let s = app.commit_scroll().0;
    let ph = usize::from(pane.height);
    let fills = lo <= s && hi + 1 >= s + ph;
    if fills {
        assert_eq!(bx.rect.bottom(), pane.bottom(), "filled: at the bottom");
    } else {
        assert!(
            off_the_rows(app, &bx, lo, hi, pane.y),
            "scroll {s}: {:?} over rows {lo}..={hi}",
            bx.rect
        );
    }
    let cur = app.commit_cursor().row;
    assert!((s..s + ph).contains(&cur), "cursor {cur} on screen at {s}");
    let y = pane.y + (cur - s) as u16;
    assert!(
        y < bx.rect.y || y >= bx.rect.bottom(),
        "cursor {cur} under {:?} scroll {s} rows {lo}..={hi} pane {pane:?}",
        bx.rect
    );
    s
}

#[test]
fn page_up_scrolls_a_selection_taller_than_the_screen_behind_the_box() {
    let f = fixture();
    let mut app = f.open();
    app.event(AppEvent::Resize(100, 24));
    let first = added(&app, "g1");
    // Selections of 30, 47 and 50 rows in a 100x24 terminal.
    for k in [30, 47, 50] {
        let (lo, hi) = (first, first + k - 1);
        select_and_comment(&mut app, lo, hi);
        assert_eq!(app.mode(), Mode::Comment, "{}", app.status());
        let mut s = check_after_scroll(&mut app, 100, 24, lo, hi);
        let start = s;
        // Up page by page to the top of the diff: each moves the view.
        while s > 0 {
            press(&mut app, KeyCode::PageUp);
            let t = check_after_scroll(&mut app, 100, 24, lo, hi);
            assert!(t < s, "{k} rows: PageUp from {s} went to {t}");
            s = t;
        }
        // And back down to the end, through where it started.
        loop {
            press(&mut app, KeyCode::PageDown);
            let t = check_after_scroll(&mut app, 100, 24, lo, hi);
            if t == s {
                break;
            }
            assert!(t > s, "{k} rows: PageDown from {s} went to {t}");
            s = t;
        }
        assert!(s >= start, "{k} rows: PageDown back to {s} (from {start})");
        assert_eq!(app.mode(), Mode::Comment);
        press(&mut app, KeyCode::Esc);
    }
}

#[test]
fn a_scrolled_selection_keeps_the_box_off_its_rows_at_small_heights() {
    let f = fixture();
    let mut app = f.open();
    let first = added(&app, "g1");
    for h in [6, 8, 12] {
        app.event(AppEvent::Resize(60, h));
        for (a, b) in [(0, 49), (10, 39), (5, 25)] {
            let (lo, hi) = (first + a, first + b);
            select_and_comment(&mut app, lo, hi);
            let mut seen = vec![check_after_scroll(&mut app, 60, h, lo, hi)];
            for k in std::iter::repeat_n(KeyCode::PageUp, 24)
                .chain(std::iter::repeat_n(KeyCode::PageDown, 48))
            {
                press(&mut app, k);
                seen.push(check_after_scroll(&mut app, 60, h, lo, hi));
            }
            // Up to the top of the diff, back down at least to where the
            // box opened.
            let (top, end) = (seen[24], *seen.last().unwrap());
            assert_eq!(top, 0, "h {h} rows {lo}..={hi}: {seen:?}");
            assert!(end >= seen[0], "h {h} rows {lo}..={hi}: {seen:?}");
            press(&mut app, KeyCode::Esc);
        }
    }
}
