//! Saved review sessions: `<leader>ss`, restore on
//! open (view and comments), `N` on the log, the quit prompt, the
//! `<leader>rs` list, and a jj rewrite. A temp repo, cache and state dir
//! per test.

mod common;

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use common::{JjEnv, commit, ctrl, git, have_jj, keys, press};
use crossterm::event::KeyCode;
use debrief::app::session::{OPEN_TO_SAVE, QUIT_PROMPT, SESSION_DELETED, SESSION_NOT_IN_LOG};
use debrief::app::{App, AppEvent, Focus, Mode, PickerKind, RowKind, Screen, StartOptions};
use debrief::session::{Anchor, Store, WORKING};
use debrief::vcs::{self, Context, LineKind};
use debrief_review::{Comment, Rev, Review, Side};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

struct Fixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    cache: PathBuf,
}

/// `base`, `middle` (a.txt line 3), then `top`: a.txt edits line 40 and
/// drops 20-22; b.txt edits line 2.
fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir(&root).unwrap();
    git(&root, &["init", "-q"]);
    let mut a: Vec<String> = (1..=60).map(|i| format!("a{i}")).collect();
    std::fs::write(root.join("a.txt"), a.join("\n") + "\n").unwrap();
    std::fs::write(root.join("b.txt"), "b1\nb2\n").unwrap();
    commit(&root, "base");
    a[2] = "a3 new".into();
    std::fs::write(root.join("a.txt"), a.join("\n") + "\n").unwrap();
    commit(&root, "middle");
    a[39] = "a40 new".into();
    a.drain(19..22);
    std::fs::write(root.join("a.txt"), a.join("\n") + "\n").unwrap();
    std::fs::write(root.join("b.txt"), "b1\nb2 new\n").unwrap();
    commit(&root, "top");
    Fixture {
        root: root.canonicalize().unwrap(),
        cache: tmp.path().join("cache"),
        _tmp: tmp,
    }
}

impl Fixture {
    fn app(&self) -> App {
        app_at(&self.root, &self.cache)
    }

    fn state(&self) -> PathBuf {
        self.cache.join("state")
    }

    fn store(&self) -> Store {
        Store::new(&self.state(), &self.root)
    }

    fn review(&self) -> Review {
        Review::open(&self.cache, &self.root).unwrap()
    }

    fn comments(&self) -> Vec<Comment> {
        self.review().comments().to_vec()
    }
}

fn app_at(root: &Path, cache: &Path) -> App {
    App::new(
        vcs::open(root, vcs::OpenOptions::default()).unwrap(),
        StartOptions::new(cache.to_path_buf()),
    )
    .unwrap()
}

/// Open log row `i`.
fn open(app: &mut App, i: usize) {
    keys(app, &format!("{}G", i + 1));
    press(app, KeyCode::Enter);
    assert!(matches!(app.screen(), Screen::Commit(_)));
}

fn row_of(app: &App, text: &str, kind: RowKind) -> usize {
    app.commit_rows()
        .iter()
        .position(|r| r.text == text && r.kind == kind)
        .unwrap_or_else(|| panic!("no {kind:?} row {text:?}"))
}

fn goto(app: &mut App, row: usize) {
    keys(app, &format!("{}G", row + 1));
    assert_eq!(app.commit_cursor().row, row);
}

fn cursor_text(app: &App) -> String {
    app.commit_rows()[app.commit_cursor().row].text.clone()
}

fn comment_here(app: &mut App, body: &str) {
    keys(app, "cc");
    assert_eq!(app.mode(), Mode::Comment, "{}", app.status());
    keys(app, body);
    press(app, KeyCode::Enter);
    assert_eq!(app.mode(), Mode::Normal);
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

fn diff_comment(commit: &str, path: &str, line: u32, body: &str) -> Comment {
    let rev = Rev {
        commit: commit.into(),
        change: None,
        summary: None,
    };
    Comment::on_diff(path, rev, Side::New, line..=line, "+x", body)
}

/// Comment on "a40 new" of `top` (row 0), save, and quit cleanly.
fn save_top_with_comment(f: &Fixture) -> (String, u64) {
    let mut app = f.app();
    open(&mut app, 0);
    let r = row_of(&app, "a40 new", RowKind::Line(LineKind::Added));
    goto(&mut app, r);
    comment_here(&mut app, "why");
    keys(&mut app, " ss");
    assert_eq!(app.status(), "Session saved (1 comment)");
    let id = f.comments()[0].id;
    (app.rows()[0].info.commit.clone(), id)
}

#[test]
fn save_writes_one_file_per_commit_under_the_state_dir() {
    let f = fixture();
    let mut app = f.app();
    keys(&mut app, " ss");
    assert_eq!(app.status(), OPEN_TO_SAVE);
    assert!(!f.state().exists(), "nothing written before the first save");
    let (top, id) = save_top_with_comment(&f);
    let path = f.store().path(&top).unwrap();
    assert!(path.starts_with(f.state().join("sessions")), "{path:?}");
    assert!(path.is_file());
    let s = f.store().load(&top).unwrap().unwrap();
    assert_eq!(s.version, 1);
    assert_eq!(s.root, f.root);
    assert_eq!(s.commit, top);
    assert_eq!(s.summary, "top");
    assert_eq!(s.comments.len(), 1);
    assert_eq!(s.comments[0].comment.id, id);
    assert_eq!(s.comments[0].sent, None);
    assert_eq!(
        s.view.anchor,
        Some(Anchor::Line {
            path: "a.txt".into(),
            side: Side::New,
            line: 37,
        })
    );
    // Saving again overwrites the one file.
    let mut app = f.app();
    open(&mut app, 0);
    keys(&mut app, "gg ss");
    let files = std::fs::read_dir(f.store().dir()).unwrap().count();
    assert_eq!(files, 1);
    let s = f.store().load(&top).unwrap().unwrap();
    assert_eq!(s.view.row, 0);
    assert_eq!(s.view.anchor, Some(Anchor::Row(0)));
}

#[test]
fn reopening_restores_the_view_and_says_so() {
    let f = fixture();
    let (_, _) = save_top_with_comment(&f);
    let mut app = f.app();
    open(&mut app, 0);
    assert_eq!(cursor_text(&app), "a40 new");
    let st = app.status();
    assert!(
        st.starts_with("Restored session (saved ") && st.ends_with("s ago, 1 comment)"),
        "{st}"
    );
}

#[test]
fn the_view_restores_after_an_async_load_with_context_layout_side_and_column() {
    let f = fixture();
    let mut app = f.app();
    app.event(AppEvent::Resize(220, 40));
    open(&mut app, 0);
    keys(&mut app, "tT");
    // The cursor to the old side, then onto the removed line.
    press(&mut app, KeyCode::Tab);
    let r = row_of(&app, "a40", RowKind::Line(LineKind::Removed));
    goto(&mut app, r);
    keys(&mut app, "ll");
    assert_eq!(app.split_side(), Some(Side::Old));
    assert_eq!(app.context(), Context::Full);
    keys(&mut app, " ss");
    drop(app);

    let mut app = f.app();
    let (tx, rx) = mpsc::channel();
    app.set_sender(tx);
    app.event(AppEvent::Resize(220, 40));
    assert_eq!(app.context(), Context::Default);
    open(&mut app, 0);
    assert!(app.commit_loading());
    assert_eq!(app.context(), Context::Full, "set before the load");
    assert!(app.split_wanted());
    app.event(rx.recv_timeout(Duration::from_secs(20)).unwrap());
    assert!(!app.commit_loading());
    assert!(app.commit_split());
    assert_eq!(app.split_side(), Some(Side::Old));
    assert_eq!(cursor_text(&app), "a40");
    assert_eq!(app.commit_cursor().col, 2);
    assert!(
        app.commit_rows().iter().any(|r| r.text == "a30"),
        "full file"
    );
}

#[test]
fn view_changes_while_the_diff_loads_are_saved_when_it_closes() {
    let f = fixture();
    let (top, _) = save_top_with_comment(&f);
    let saved = f.store().load(&top).unwrap().unwrap().view;
    assert_eq!(saved.context, debrief::session::ViewContext::Hunks);
    let mut app = f.app();
    let (tx, _rx) = mpsc::channel();
    app.set_sender(tx);
    open(&mut app, 0);
    assert!(app.commit_loading());
    keys(&mut app, "T");
    press(&mut app, KeyCode::Esc);
    keys(&mut app, "qs");
    assert!(app.should_quit(), "{}", app.status());
    let s = f.store().load(&top).unwrap().unwrap().view;
    assert_eq!(s.context, debrief::session::ViewContext::Full, "T kept");
    assert_eq!(s.anchor, saved.anchor, "the saved cursor stays");
    assert_eq!((s.row, s.col), (saved.row, saved.col));
}

#[test]
fn a_narrow_terminal_restores_unified_and_keeps_the_saved_layout() {
    let f = fixture();
    let mut app = f.app();
    app.event(AppEvent::Resize(220, 40));
    open(&mut app, 0);
    keys(&mut app, "t");
    let r = row_of(&app, "b2 new", RowKind::Line(LineKind::Added));
    goto(&mut app, r);
    keys(&mut app, " ss");
    drop(app);

    let mut app = f.app();
    app.event(AppEvent::Resize(100, 40));
    open(&mut app, 0);
    assert!(app.split_wanted(), "the saved layout stays wanted");
    assert!(!app.commit_split(), "unified while too narrow");
    assert_eq!(cursor_text(&app), "b2 new");
    app.event(AppEvent::Resize(220, 40));
    assert!(app.commit_split(), "side by side once wide enough");
    assert_eq!(cursor_text(&app), "b2 new");
}

#[test]
fn the_view_captured_this_run_wins_over_the_saved_one() {
    let f = fixture();
    let _ = save_top_with_comment(&f);
    let mut app = f.app();
    open(&mut app, 0);
    keys(&mut app, "gg");
    press(&mut app, KeyCode::Esc);
    open(&mut app, 0);
    assert_eq!(app.commit_cursor().row, 0);
}

#[test]
fn a_gone_file_falls_back_to_row_zero() {
    let f = fixture();
    let (top, _) = save_top_with_comment(&f);
    let mut s = f.store().load(&top).unwrap().unwrap();
    s.view.anchor = Some(Anchor::Line {
        path: "gone.txt".into(),
        side: Side::New,
        line: 3,
    });
    s.view.row = 9999;
    f.store().save(&s).unwrap();
    let mut app = f.app();
    open(&mut app, 0);
    assert_eq!(app.commit_cursor().row, 0);
}

#[test]
fn comments_come_back_into_an_empty_batch_with_their_ids() {
    let f = fixture();
    let (top, id) = save_top_with_comment(&f);
    f.review().clear().unwrap();
    let mut app = f.app();
    open(&mut app, 0);
    let cs = f.comments();
    assert_eq!(cs.len(), 1);
    assert_eq!(cs[0].id, id);
    assert_eq!(cs[0].rev.as_ref().unwrap().commit, top);
    assert_eq!(app.review_marked_rows().len(), 1);
    // Reopening doesn't duplicate them.
    press(&mut app, KeyCode::Esc);
    open(&mut app, 0);
    drop(app);
    let mut app = f.app();
    open(&mut app, 0);
    assert_eq!(f.comments().len(), 1);
}

#[test]
fn comments_come_back_into_a_batch_that_has_some() {
    let f = fixture();
    let (top, id) = save_top_with_comment(&f);
    let other = f
        .review()
        .add(diff_comment(&top, "b.txt", 2, "other"))
        .unwrap();
    f.review().edit(id, "edited since").unwrap();
    let mut app = f.app();
    open(&mut app, 0);
    let cs = f.comments();
    assert_eq!(cs.len(), 2, "no duplicates: {cs:?}");
    assert!(cs.iter().any(|c| c.id == id && c.body == "edited since"));
    assert!(cs.iter().any(|c| c.id == other));
}

#[test]
fn a_clashing_id_gets_a_fresh_one_and_the_session_follows() {
    let f = fixture();
    let (top, id) = save_top_with_comment(&f);
    f.review().clear().unwrap();
    // Another comment took the id (a wiped lock file).
    for e in std::fs::read_dir(f.cache.join("reviews")).unwrap() {
        let p = e.unwrap().path();
        if p.extension().unwrap() == "lock" {
            std::fs::remove_file(p).unwrap();
        }
    }
    let mut clash = diff_comment(&top, "b.txt", 2, "clash");
    clash.id = id;
    f.review().restore(&[clash]).unwrap();
    let mut app = f.app();
    open(&mut app, 0);
    let cs = f.comments();
    assert_eq!(cs.len(), 2);
    let why = cs.iter().find(|c| c.body == "why").unwrap();
    assert_ne!(why.id, id);
    let s = f.store().load(&top).unwrap().unwrap();
    assert_eq!(s.comments[0].comment.id, why.id, "session rewritten");
}

#[test]
fn the_log_marks_sessions_with_their_unsent_count() {
    let f = fixture();
    let (top, _) = save_top_with_comment(&f);
    let mut app = f.app();
    let s = screen(&mut app, 80, 8);
    assert!(s[0].contains("1 top"), "{}", s[0]);
    assert!(!s[1].contains(''), "{}", s[1]);
    // Kept by a filter.
    keys(&mut app, "ftop");
    press(&mut app, KeyCode::Enter);
    let s = screen(&mut app, 80, 8);
    assert!(s[0].contains("1 top"), "{}", s[0]);
    // No comments: the bare marker.
    let mut s = f.store().load(&top).unwrap().unwrap();
    s.comments.clear();
    f.store().save(&s).unwrap();
    let mut app = f.app();
    let s = screen(&mut app, 80, 8);
    assert!(s[0].contains(" top"), "{}", s[0]);
}

#[test]
fn quit_without_review_work_quits_at_once() {
    let f = fixture();
    let mut app = f.app();
    open(&mut app, 1);
    keys(&mut app, "jjj");
    press(&mut app, KeyCode::Esc);
    keys(&mut app, "q");
    assert!(
        app.should_quit(),
        "view changes alone, no session: no prompt"
    );
}

/// An app with an unsaved comment on `top`, back on the log.
fn with_unsaved(f: &Fixture) -> App {
    let mut app = f.app();
    open(&mut app, 0);
    let r = row_of(&app, "b2 new", RowKind::Line(LineKind::Added));
    goto(&mut app, r);
    comment_here(&mut app, "unsaved");
    press(&mut app, KeyCode::Esc);
    app
}

#[test]
fn quit_prompt_esc_stays_and_q_quits() {
    let f = fixture();
    let mut app = with_unsaved(&f);
    keys(&mut app, "q");
    assert!(!app.should_quit());
    assert_eq!(app.mode(), Mode::QuitPrompt);
    assert_eq!(app.status(), QUIT_PROMPT);
    let s = screen(&mut app, 80, 8);
    assert!(s[7].contains(QUIT_PROMPT), "{}", s[7]);
    keys(&mut app, "x");
    assert_eq!(app.mode(), Mode::QuitPrompt, "other keys do nothing");
    press(&mut app, KeyCode::Esc);
    assert_eq!(app.mode(), Mode::Normal);
    assert!(!app.should_quit());
    keys(&mut app, "q");
    keys(&mut app, "q");
    assert!(app.should_quit(), "a second q quits");
    assert!(!f.state().exists(), "nothing saved");
}

#[test]
fn quit_prompt_shows_over_a_search_or_filter_prompt() {
    for (commit, prompt) in [(true, "/needle"), (false, "ftop"), (false, "/top")] {
        let f = fixture();
        let mut app = with_unsaved(&f);
        if commit {
            open(&mut app, 0);
        }
        keys(&mut app, prompt);
        ctrl(&mut app, 'c');
        assert_eq!(app.mode(), Mode::QuitPrompt);
        let s = screen(&mut app, 120, 20);
        assert!(s[19].contains(QUIT_PROMPT), "{prompt}: {}", s[19]);
    }
}

#[test]
fn quit_prompt_c_c_twice_quits_from_the_commit_view() {
    let f = fixture();
    let mut app = with_unsaved(&f);
    open(&mut app, 0);
    keys(&mut app, "q");
    assert_eq!(app.screen(), Screen::Log, "q in the commit view goes back");
    open(&mut app, 0);
    ctrl(&mut app, 'c');
    assert!(!app.should_quit());
    assert_eq!(app.mode(), Mode::QuitPrompt);
    ctrl(&mut app, 'c');
    assert!(app.should_quit());
}

#[test]
fn quit_prompt_s_saves_every_unsaved_commit_and_quits() {
    let f = fixture();
    let mut app = with_unsaved(&f);
    let top = app.rows()[0].info.commit.clone();
    let middle = app.rows()[1].info.commit.clone();
    // A comment on `middle` from before this run, never saved.
    f.review()
        .add(diff_comment(&middle, "a.txt", 3, "old"))
        .unwrap();
    open(&mut app, 1);
    press(&mut app, KeyCode::Esc);
    keys(&mut app, "q");
    assert_eq!(app.mode(), Mode::QuitPrompt);
    keys(&mut app, "s");
    assert!(app.should_quit(), "{}", app.status());
    let s = f.store().load(&top).unwrap().unwrap();
    assert_eq!(s.comments[0].comment.body, "unsaved");
    assert_eq!(s.view.anchor.as_ref().map(|_| ()), Some(()));
    let s = f.store().load(&middle).unwrap().unwrap();
    assert_eq!(s.comments[0].comment.body, "old");
}

#[test]
fn quit_prompt_s_that_fails_does_not_quit() {
    let f = fixture();
    let mut app = with_unsaved(&f);
    // A file where the state dir should be.
    std::fs::create_dir_all(f.cache.clone()).unwrap();
    std::fs::write(f.state(), "not a dir").unwrap();
    keys(&mut app, "qs");
    assert!(!app.should_quit());
    assert!(
        app.status().starts_with("session not saved: "),
        "{}",
        app.status()
    );
}

/// Make the batch file unreadable JSON.
fn corrupt_batch(f: &Fixture) {
    let p = std::fs::read_dir(f.cache.join("reviews"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.extension().is_some_and(|x| x == "json"))
        .unwrap();
    std::fs::write(p, "{not json").unwrap();
}

#[test]
fn quit_prompt_s_with_an_unreadable_batch_does_not_save_or_quit() {
    let f = fixture();
    let mut app = with_unsaved(&f);
    let top = app.rows()[0].info.commit.clone();
    keys(&mut app, "q");
    assert_eq!(app.mode(), Mode::QuitPrompt);
    corrupt_batch(&f);
    keys(&mut app, "s");
    assert!(!app.should_quit());
    assert!(
        app.status().starts_with("session not saved: "),
        "{}",
        app.status()
    );
    assert!(f.store().load(&top).unwrap().is_none(), "no stale save");
}

#[test]
fn leader_ss_with_an_unreadable_batch_does_not_save() {
    let f = fixture();
    let mut app = with_unsaved(&f);
    let top = app.rows()[0].info.commit.clone();
    open(&mut app, 0);
    corrupt_batch(&f);
    keys(&mut app, " ss");
    assert!(
        app.status().starts_with("session not saved: "),
        "{}",
        app.status()
    );
    assert!(f.store().load(&top).unwrap().is_none(), "no stale save");
}

#[test]
fn quit_prompt_y_copies_marks_sent_and_quits() {
    let f = fixture();
    let (top, id) = save_top_with_comment(&f);
    let written = Rc::new(RefCell::new(Vec::<u8>::new()));
    let sink = written.clone();
    let mut app = f.app().with_terminal(move |b| {
        sink.borrow_mut().extend_from_slice(b);
        Ok(())
    });
    open(&mut app, 0);
    keys(&mut app, "k");
    press(&mut app, KeyCode::Esc);
    keys(&mut app, "q");
    assert_eq!(app.mode(), Mode::QuitPrompt, "a session's view changed");
    keys(&mut app, "y");
    assert!(app.should_quit());
    let out = String::from_utf8(written.borrow().clone()).unwrap();
    assert!(out.starts_with("\x1b]52;c;"), "{out:?}");
    assert!(f.comments().is_empty(), "copied comments leave the batch");
    let s = f.store().load(&top).unwrap().unwrap();
    assert_eq!(s.comments[0].comment.id, id);
    assert!(s.comments[0].sent.is_some(), "marked sent");
    // A sent comment is never restored and the marker counts unsent ones.
    let mut app = f.app();
    let s = screen(&mut app, 80, 8);
    assert!(s[0].contains(" top"), "{}", s[0]);
    open(&mut app, 0);
    assert!(f.comments().is_empty());
}

#[test]
fn a_send_clears_the_warning_and_marks_the_session_sent() {
    let f = fixture();
    let (top, _) = save_top_with_comment(&f);
    let mut opts = StartOptions::new(f.cache.clone());
    opts.config.send.command = vec!["true".into()];
    let mut app = App::new(
        vcs::open(&f.root, vcs::OpenOptions::default()).unwrap(),
        opts,
    )
    .unwrap();
    open(&mut app, 0);
    comment_here(&mut app, "more");
    keys(&mut app, " rr");
    app.run_pending_effect();
    assert!(app.status().starts_with("Review sent"), "{}", app.status());
    let s = f.store().load(&top).unwrap().unwrap();
    assert!(s.comments.iter().all(|c| c.sent.is_some()));
    press(&mut app, KeyCode::Esc);
    keys(&mut app, "q");
    assert!(app.should_quit(), "sent since the last change: no prompt");
}

/// Make writes to the session of `key` fail: a directory where its temp
/// file goes.
fn break_session_write(f: &Fixture, key: &str) {
    let mut tmp = f.store().path(key).unwrap().into_os_string();
    tmp.push(".tmp");
    std::fs::create_dir(tmp).unwrap();
}

#[test]
fn quit_prompt_y_that_cannot_mark_sent_keeps_the_batch_and_does_not_quit() {
    let f = fixture();
    let (top, _) = save_top_with_comment(&f);
    let mut app = f.app().with_terminal(|_| Ok(()));
    open(&mut app, 0);
    keys(&mut app, "k");
    press(&mut app, KeyCode::Esc);
    keys(&mut app, "q");
    assert_eq!(app.mode(), Mode::QuitPrompt);
    break_session_write(&f, &top);
    keys(&mut app, "y");
    assert!(!app.should_quit());
    assert!(
        app.status().contains("session not updated"),
        "{}",
        app.status()
    );
    assert_eq!(f.comments().len(), 1, "still in the batch");
    assert_eq!(f.store().load(&top).unwrap().unwrap().unsent_count(), 1);
    // Still unsaved work: q asks again.
    keys(&mut app, "q");
    assert_eq!(app.mode(), Mode::QuitPrompt);
}

#[test]
fn a_send_that_cannot_mark_sent_keeps_the_batch() {
    let f = fixture();
    let (top, _) = save_top_with_comment(&f);
    let mut opts = StartOptions::new(f.cache.clone());
    opts.config.send.command = vec!["true".into()];
    let mut app = App::new(
        vcs::open(&f.root, vcs::OpenOptions::default()).unwrap(),
        opts,
    )
    .unwrap();
    break_session_write(&f, &top);
    keys(&mut app, " rr");
    app.run_pending_effect();
    assert!(
        app.status().contains("session not updated"),
        "{}",
        app.status()
    );
    assert_eq!(f.comments().len(), 1, "still in the batch");
    assert_eq!(f.store().load(&top).unwrap().unwrap().unsent_count(), 1);
}

#[test]
fn session_list_opens_and_deletes() {
    let f = fixture();
    let (top, _) = save_top_with_comment(&f);
    let mut app = f.app();
    keys(&mut app, "G");
    keys(&mut app, " rs");
    assert_eq!(app.mode(), Mode::Picker);
    let v = app.picker().unwrap();
    assert_eq!(v.kind, PickerKind::Sessions);
    assert_eq!(v.title, "Review sessions");
    assert_eq!(v.items.len(), 1);
    assert_eq!(v.items[0].key, top);
    assert!(v.items[0].label.ends_with(" top"), "{}", v.items[0].label);
    assert!(
        v.items[0].detail.ends_with("ago, 1 comment"),
        "{}",
        v.items[0].detail
    );
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.screen(), Screen::Commit(0));
    assert_eq!(cursor_text(&app), "a40 new");
    keys(&mut app, " rs");
    // C-e does nothing on a session.
    ctrl(&mut app, 'e');
    assert!(app.pending_effect().is_none());
    ctrl(&mut app, 'd');
    assert_eq!(app.status(), SESSION_DELETED);
    assert!(app.picker().unwrap().items.is_empty());
    assert!(f.store().load(&top).unwrap().is_none());
    press(&mut app, KeyCode::Esc);
    let s = screen(&mut app, 80, 8);
    assert!(!s.iter().any(|l| l.contains('')), "{s:?}");
}

#[test]
fn session_list_counts_sent_comments() {
    let f = fixture();
    let (top, _) = save_top_with_comment(&f);
    let mut s = f.store().load(&top).unwrap().unwrap();
    let mut sent = s.comments[0].clone();
    sent.comment.id += 1;
    sent.sent = Some(1);
    s.comments.push(sent);
    f.store().save(&s).unwrap();
    let mut app = f.app();
    keys(&mut app, " rs");
    let v = app.picker().unwrap();
    assert!(
        v.items[0].detail.ends_with("2 comments (1 sent)"),
        "{}",
        v.items[0].detail
    );
}

#[test]
fn a_corrupt_session_is_reported_and_skipped() {
    let f = fixture();
    let (top, _) = save_top_with_comment(&f);
    std::fs::write(f.store().path("deadbeef").unwrap(), "{oops").unwrap();
    let mut app = f.app();
    assert!(
        app.status().starts_with("session skipped: "),
        "{}",
        app.status()
    );
    keys(&mut app, " rs");
    assert_eq!(app.picker().unwrap().items.len(), 1);
    press(&mut app, KeyCode::Esc);
    assert!(app.saved_session(&top).is_some());
    keys(&mut app, " rs");
    assert!(
        !app.status().starts_with("session skipped"),
        "reported once: {}",
        app.status()
    );
}

#[test]
fn the_working_copy_has_one_session_file() {
    let f = fixture();
    std::fs::write(f.root.join("b.txt"), "b1\nb2 new\nb3\n").unwrap();
    let mut app = f.app();
    assert!(app.rows()[0].working);
    open(&mut app, 0);
    let r = row_of(&app, "b3", RowKind::Line(LineKind::Added));
    goto(&mut app, r);
    comment_here(&mut app, "wc");
    keys(&mut app, " ss");
    assert_eq!(app.status(), "Session saved (1 comment)");
    assert!(f.store().dir().join("working-copy.json").is_file());
    let s = f.store().load(WORKING).unwrap().unwrap();
    assert_eq!(s.commit, WORKING);
    assert_eq!(s.comments[0].comment.rev, None);
    press(&mut app, KeyCode::Esc);
    let s = screen(&mut app, 80, 8);
    assert!(s[0].ends_with("1"), "{}", s[0]);
}

#[test]
fn the_working_copy_session_opens_behind_a_filter() {
    let f = fixture();
    std::fs::write(f.root.join("b.txt"), "b1\nb2 new\nb3\n").unwrap();
    let mut app = f.app();
    open(&mut app, 0);
    keys(&mut app, " ss");
    press(&mut app, KeyCode::Esc);
    keys(&mut app, "ftop");
    press(&mut app, KeyCode::Enter);
    assert!(!app.rows().iter().any(|r| r.working));
    keys(&mut app, " rs");
    assert_eq!(app.picker().unwrap().items[0].key, WORKING);
    press(&mut app, KeyCode::Enter);
    assert!(
        matches!(app.screen(), Screen::Commit(_)),
        "{}",
        app.status()
    );
    assert!(app.viewing_working_copy());
}

#[test]
fn the_jj_working_copy_session_opens_behind_a_filter() {
    if !have_jj() {
        return;
    }
    let e = JjEnv::new(false);
    e.write("a.txt", "one\ntwo\n");
    e.jj(&["--quiet", "commit", "-m", "base"]);
    e.write("a.txt", "one\nTHREE\n");
    e.jj(&["--quiet", "describe", "-m", "working-change"]);
    let cache = e.root.parent().unwrap().join("cache");
    let mut app = app_at(&e.root, &cache);
    let i = app.working_row_index().unwrap();
    open(&mut app, i);
    keys(&mut app, " ss");
    press(&mut app, KeyCode::Esc);
    keys(&mut app, "fbase");
    press(&mut app, KeyCode::Enter);
    assert!(!app.rows().iter().any(|r| r.working));
    keys(&mut app, " rs");
    press(&mut app, KeyCode::Enter);
    assert!(
        matches!(app.screen(), Screen::Commit(_)),
        "{}",
        app.status()
    );
    assert!(app.viewing_working_copy());
}

#[test]
fn files_focus_comes_back() {
    let f = fixture();
    let mut app = f.app();
    app.event(AppEvent::Resize(120, 30));
    open(&mut app, 0);
    ctrl(&mut app, 'w');
    keys(&mut app, "h");
    assert_eq!(app.focus(), Focus::Files);
    keys(&mut app, " ss");
    drop(app);
    let mut app = f.app();
    app.event(AppEvent::Resize(120, 30));
    open(&mut app, 0);
    assert_eq!(app.focus(), Focus::Files);
}

#[test]
fn a_jj_rewrite_leaves_the_session_on_the_old_commit() {
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
    let new_app = || {
        App::new(
            vcs::open(&e.root, vcs::OpenOptions::default()).unwrap(),
            opts(),
        )
        .unwrap()
    };
    let mut app = new_app();
    open(&mut app, 0);
    let old = app.rows()[0].info.commit.clone();
    let r = row_of(&app, "TWO", RowKind::Line(LineKind::Added));
    goto(&mut app, r);
    comment_here(&mut app, "shout");
    keys(&mut app, " ss");
    drop(app);

    e.jj(&["--quiet", "describe", "-r", "@-", "-m", "second, reworded"]);
    let mut app = new_app();
    assert_ne!(app.rows()[0].info.commit, old, "rewritten");
    let s = screen(&mut app, 80, 8);
    assert!(!s.iter().any(|l| l.contains('')), "{s:?}");
    keys(&mut app, " rs");
    let v = app.picker().unwrap();
    assert_eq!(v.items.len(), 1);
    assert_eq!(v.items[0].key, old);
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.status(), SESSION_NOT_IN_LOG);
    assert_eq!(app.screen(), Screen::Log);
}

/// Delete the batch file (a batch that reads as empty).
fn drop_batch(f: &Fixture) {
    for e in std::fs::read_dir(f.cache.join("reviews")).unwrap() {
        let p = e.unwrap().path();
        if p.extension().is_some_and(|x| x == "json") {
            std::fs::remove_file(p).unwrap();
        }
    }
}

#[test]
fn a_batch_unreadable_at_startup_restores_nothing_and_saves_nothing() {
    let f = fixture();
    let (top, id) = save_top_with_comment(&f);
    corrupt_batch(&f);
    let mut app = f.app();
    open(&mut app, 0);
    assert!(
        app.status().contains("1 comment not restored: "),
        "{}",
        app.status()
    );
    keys(&mut app, " ss");
    assert!(
        app.status()
            .starts_with("session not saved: review batch: "),
        "{}",
        app.status()
    );
    let s = f.store().load(&top).unwrap().unwrap();
    assert_eq!(s.unsent().map(|c| c.id).collect::<Vec<_>>(), [id]);
    keys(&mut app, "k");
    press(&mut app, KeyCode::Esc);
    keys(&mut app, "q");
    assert_eq!(app.mode(), Mode::QuitPrompt);
    keys(&mut app, "s");
    assert!(!app.should_quit());
    assert!(
        app.status()
            .starts_with("session not saved: review batch: "),
        "{}",
        app.status()
    );
    assert_eq!(f.store().load(&top).unwrap().unwrap().unsent_count(), 1);
}

#[test]
fn a_failed_restore_is_retried_before_a_save() {
    let f = fixture();
    let (top, id) = save_top_with_comment(&f);
    let mut app = f.app();
    corrupt_batch(&f);
    open(&mut app, 0);
    assert!(
        app.status().contains("1 comment not restored: "),
        "{}",
        app.status()
    );
    // The batch reads again (empty): the save brings the comment back first.
    drop_batch(&f);
    keys(&mut app, " ss");
    assert_eq!(app.status(), "Session saved (1 comment)");
    assert_eq!(f.comments().iter().map(|c| c.id).collect::<Vec<_>>(), [id]);
    assert_eq!(f.store().load(&top).unwrap().unwrap().unsent_count(), 1);
}

/// Mark every comment of `key`'s session sent on disk, as another
/// debrief's send does, and empty the batch.
fn send_elsewhere(f: &Fixture, key: &str) {
    let mut s = f.store().load(key).unwrap().unwrap();
    for c in &mut s.comments {
        c.sent = Some(1);
    }
    f.store().save(&s).unwrap();
    f.review().clear().unwrap();
}

#[test]
fn a_comment_sent_by_another_debrief_is_not_restored() {
    let f = fixture();
    let (top, _) = save_top_with_comment(&f);
    let mut app = f.app();
    send_elsewhere(&f, &top);
    open(&mut app, 0);
    assert!(f.comments().is_empty(), "{:?}", f.comments());
}

#[test]
fn a_save_keeps_a_comment_another_debrief_sent() {
    let f = fixture();
    let (top, id) = save_top_with_comment(&f);
    let mut app = f.app();
    open(&mut app, 0);
    send_elsewhere(&f, &top);
    keys(&mut app, " ss");
    assert_eq!(app.status(), "Session saved (0 comments)");
    let s = f.store().load(&top).unwrap().unwrap();
    assert_eq!(s.comments.len(), 1);
    assert_eq!(s.comments[0].comment.id, id);
    assert!(s.comments[0].sent.is_some());
}

#[test]
fn a_send_keeps_comments_another_debrief_saved() {
    let f = fixture();
    let (top, _) = save_top_with_comment(&f);
    let mut opts = StartOptions::new(f.cache.clone());
    opts.config.send.command = vec!["true".into()];
    let mut app = App::new(
        vcs::open(&f.root, vcs::OpenOptions::default()).unwrap(),
        opts,
    )
    .unwrap();
    // Another debrief saved a second comment, since gone from the batch.
    let mut s = f.store().load(&top).unwrap().unwrap();
    let mut other = s.comments[0].clone();
    other.comment.id += 1;
    other.comment.body = "elsewhere".into();
    s.comments.push(other);
    f.store().save(&s).unwrap();
    keys(&mut app, " rr");
    app.run_pending_effect();
    assert!(app.status().starts_with("Review sent"), "{}", app.status());
    let s = f.store().load(&top).unwrap().unwrap();
    assert_eq!(s.comments.len(), 2, "{s:?}");
    let unsent: Vec<_> = s.unsent().map(|c| c.body.as_str()).collect();
    assert_eq!(unsent, ["elsewhere"]);
}

#[test]
fn removing_a_saved_comment_of_an_unvisited_commit_is_unsaved_work() {
    let f = fixture();
    let (top, _) = save_top_with_comment(&f);
    let mut app = f.app();
    keys(&mut app, " rl");
    assert_eq!(app.picker().unwrap().items.len(), 1);
    ctrl(&mut app, 'd');
    assert!(f.comments().is_empty());
    press(&mut app, KeyCode::Esc);
    keys(&mut app, "q");
    assert_eq!(app.mode(), Mode::QuitPrompt, "{}", app.status());
    keys(&mut app, "s");
    assert!(app.should_quit(), "{}", app.status());
    assert_eq!(f.store().load(&top).unwrap().unwrap().unsent_count(), 0);
    let mut app = f.app();
    open(&mut app, 0);
    assert!(f.comments().is_empty(), "not restored");
}

#[test]
fn t_while_a_restored_diff_loads_keeps_the_saved_cursor() {
    let f = fixture();
    let _ = save_top_with_comment(&f);
    let mut app = f.app();
    let (tx, rx) = mpsc::channel();
    app.set_sender(tx);
    open(&mut app, 0);
    assert!(app.commit_loading());
    keys(&mut app, "T");
    while app.commit_loading() {
        app.event(rx.recv_timeout(Duration::from_secs(20)).unwrap());
    }
    assert_eq!(app.context(), Context::Full);
    assert_eq!(cursor_text(&app), "a40 new");
}

#[test]
fn the_working_copy_session_holds_old_side_comments_on_its_base() {
    let f = fixture();
    std::fs::write(f.root.join("b.txt"), "b2 new\n").unwrap();
    let mut app = f.app();
    let head = app.rows()[1].info.commit.clone();
    open(&mut app, 0);
    let r = row_of(&app, "b1", RowKind::Line(LineKind::Removed));
    goto(&mut app, r);
    comment_here(&mut app, "gone");
    keys(&mut app, " ss");
    assert_eq!(app.status(), "Session saved (1 comment)");
    let s = f.store().load(WORKING).unwrap().unwrap();
    let c = &s.comments[0].comment;
    assert_eq!(c.side, Some(Side::Old));
    assert_eq!(c.rev.as_ref().unwrap().commit, head);
    drop(app);
    f.review().clear().unwrap();
    let mut app = f.app();
    open(&mut app, 0);
    assert_eq!(f.comments().len(), 1, "restored");
    assert_eq!(app.review_marked_rows().len(), 1);
}

#[test]
fn a_save_replaces_a_corrupt_session_file() {
    let f = fixture();
    let (top, _) = save_top_with_comment(&f);
    std::fs::write(f.store().path(&top).unwrap(), "{oops").unwrap();
    let mut app = f.app();
    open(&mut app, 0);
    keys(&mut app, " ss");
    assert_eq!(app.status(), "Session saved (1 comment)");
    assert_eq!(f.store().load(&top).unwrap().unwrap().unsent_count(), 1);
}

#[test]
fn a_send_that_cannot_read_a_session_keeps_the_batch() {
    let f = fixture();
    let (top, _) = save_top_with_comment(&f);
    let mut opts = StartOptions::new(f.cache.clone());
    opts.config.send.command = vec!["true".into()];
    let mut app = App::new(
        vcs::open(&f.root, vcs::OpenOptions::default()).unwrap(),
        opts,
    )
    .unwrap();
    // The session can't be read: a directory where its file was.
    let path = f.store().path(&top).unwrap();
    let saved = std::fs::read(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    keys(&mut app, " rr");
    app.run_pending_effect();
    assert!(
        app.status().contains("session not updated"),
        "{}",
        app.status()
    );
    assert_eq!(f.comments().len(), 1, "still in the batch");
    std::fs::remove_dir(&path).unwrap();
    std::fs::write(&path, saved).unwrap();
    assert_eq!(f.store().load(&top).unwrap().unwrap().unsent_count(), 1);
}

#[test]
fn a_send_skips_a_file_that_is_not_a_session() {
    let f = fixture();
    let (top, _) = save_top_with_comment(&f);
    let mut opts = StartOptions::new(f.cache.clone());
    opts.config.send.command = vec!["true".into()];
    let mut app = App::new(
        vcs::open(&f.root, vcs::OpenOptions::default()).unwrap(),
        opts,
    )
    .unwrap();
    std::fs::write(f.store().path("deadbeef").unwrap(), "{oops").unwrap();
    keys(&mut app, " rr");
    app.run_pending_effect();
    assert!(app.status().starts_with("Review sent"), "{}", app.status());
    assert!(f.comments().is_empty());
    assert_eq!(f.store().load(&top).unwrap().unwrap().unsent_count(), 0);
}
