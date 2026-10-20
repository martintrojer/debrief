//! The log's branch: the current one at start, `--branch`, `<leader>b`
//! on both screens, the working-copy row, comments and sessions recording
//! it, and a branch that goes away.

mod common;

use std::path::{Path, PathBuf};

use common::{JjEnv, commit, git, have_jj, keys, press};
use crossterm::event::KeyCode;
use debrief::app::{App, Mode, PickerKind, RowKind, Screen, StartOptions};
use debrief::vcs::{self, LineKind, OpenOptions};

/// `main`: first, second; `feat/x` off second: f1, f2; checked out on
/// `main`.
fn repo() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir(&root).unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    std::fs::write(root.join("a.txt"), "1\n").unwrap();
    commit(&root, "first");
    std::fs::write(root.join("a.txt"), "2\n").unwrap();
    commit(&root, "second");
    git(&root, &["checkout", "-q", "-b", "feat/x"]);
    std::fs::write(root.join("f.txt"), "1\n").unwrap();
    commit(&root, "f1");
    std::fs::write(root.join("f.txt"), "2\n").unwrap();
    commit(&root, "f2");
    git(&root, &["checkout", "-q", "main"]);
    let root = root.canonicalize().unwrap();
    (tmp, root)
}

fn opts(tmp: &Path) -> StartOptions {
    let mut o = StartOptions::new(tmp.join("cache"));
    o.state_dir = tmp.join("state");
    o.config.view.stats = false;
    o
}

fn open_with(root: &Path, o: StartOptions) -> anyhow::Result<App> {
    App::new(vcs::open(root, OpenOptions::default()).unwrap(), o)
}

fn rows(app: &App) -> Vec<String> {
    app.rows().iter().map(|r| r.info.summary.clone()).collect()
}

/// `<leader>b`, type `filter`, Enter.
fn pick(app: &mut App, filter: &str) {
    keys(app, " b");
    assert_eq!(app.picker().map(|p| p.kind), Some(PickerKind::Branches));
    keys(app, filter);
    press(app, KeyCode::Enter);
}

#[test]
fn starts_on_the_current_branch_and_picks_another() {
    let (tmp, root) = repo();
    let mut app = open_with(&root, opts(tmp.path())).unwrap();
    assert_eq!(app.branch_name(), Some("main"));
    assert_eq!(app.current_branch(), Some("main"));
    assert_eq!(rows(&app), ["second", "first"]);
    // The picker lists the default history, then the branches, the
    // current one first; the selected one is marked.
    keys(&mut app, " b");
    let labels: Vec<String> = app
        .picker()
        .unwrap()
        .items
        .iter()
        .map(|i| i.label.clone())
        .collect();
    assert_eq!(labels, ["  HEAD", "● main", "  feat/x"]);
    press(&mut app, KeyCode::Esc);
    pick(&mut app, "feat");
    assert_eq!(app.branch_name(), Some("feat/x"));
    assert_eq!(rows(&app), ["f2", "f1"], "only what trunk lacks");
    pick(&mut app, "HEAD");
    assert_eq!(app.branch_name(), None);
    assert_eq!(app.branch_label(), "HEAD");
    assert_eq!(rows(&app), ["second", "first"]);
}

#[test]
fn branch_flag_selects_and_an_unknown_one_fails() {
    let (tmp, root) = repo();
    let mut o = opts(tmp.path());
    o.branch = Some("feat/x".into());
    let app = open_with(&root, o).unwrap();
    assert_eq!(rows(&app), ["f2", "f1"]);
    let mut o = opts(tmp.path());
    o.branch = Some("nope".into());
    assert!(open_with(&root, o).is_err());
}

#[test]
fn a_revset_starts_selected_and_stays_pickable() {
    let (tmp, root) = repo();
    let mut o = opts(tmp.path());
    o.revset = Some("main~1".into());
    let mut app = open_with(&root, o).unwrap();
    assert_eq!(app.branch_label(), "revset: main~1");
    assert_eq!(rows(&app), ["first"]);
    pick(&mut app, "feat");
    assert_eq!(rows(&app), ["f2", "f1"]);
    pick(&mut app, "revset");
    assert_eq!(rows(&app), ["first"]);
}

#[test]
fn the_working_copy_row_shows_only_on_the_checked_out_branch() {
    let (tmp, root) = repo();
    std::fs::write(root.join("dirty.txt"), "d\n").unwrap();
    let mut app = open_with(&root, opts(tmp.path())).unwrap();
    assert!(app.rows()[0].working);
    pick(&mut app, "feat");
    assert!(!app.rows().iter().any(|r| r.working), "{:?}", rows(&app));
    keys(&mut app, "r");
    assert!(!app.rows().iter().any(|r| r.working), "r keeps it hidden");
    pick(&mut app, "main");
    assert!(app.rows()[0].working);
}

#[test]
fn the_commit_view_switches_and_keeps_the_open_diff() {
    let (tmp, root) = repo();
    let mut app = open_with(&root, opts(tmp.path())).unwrap();
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.screen(), Screen::Commit(0));
    let before = app.commit_rows().to_vec();
    pick(&mut app, "feat");
    assert!(matches!(app.screen(), Screen::Commit(_)));
    assert_eq!(app.commit_rows(), before, "the open diff stays");
    assert_eq!(rows(&app), ["f2", "f1"]);
    // `]]` steps into the new list from the old place.
    keys(&mut app, "]]");
    assert!(matches!(app.screen(), Screen::Commit(_)));
}

#[test]
fn a_gone_branch_falls_back_to_the_current_one() {
    let (tmp, root) = repo();
    let mut app = open_with(&root, opts(tmp.path())).unwrap();
    pick(&mut app, "feat");
    git(&root, &["branch", "-q", "-D", "feat/x"]);
    keys(&mut app, "r");
    assert_eq!(app.branch_name(), Some("main"));
    assert_eq!(app.status(), "branch feat/x is gone");
    assert_eq!(rows(&app), ["second", "first"]);
}

#[test]
fn comments_and_sessions_record_the_branch() {
    let (tmp, root) = repo();
    let mut app = open_with(&root, opts(tmp.path())).unwrap();
    pick(&mut app, "feat");
    press(&mut app, KeyCode::Enter);
    // f2's added line.
    let r = app
        .commit_rows()
        .iter()
        .position(|r| r.text == "2" && r.kind == RowKind::Line(LineKind::Added))
        .expect("f2's added line");
    keys(&mut app, &format!("{}G", r + 1));
    keys(&mut app, "cc");
    assert_eq!(app.mode(), Mode::Comment, "{}", app.status());
    keys(&mut app, "ok");
    press(&mut app, KeyCode::Enter);
    let review = debrief_review::Review::open(&tmp.path().join("cache"), &root).unwrap();
    let c = &review.comments()[0];
    assert_eq!(c.rev.as_ref().unwrap().branch.as_deref(), Some("feat/x"));
    assert!(
        review.to_markdown(None, &[]).contains("- Branch: feat/x\n"),
        "the export names it"
    );
    keys(&mut app, " ss");
    // Back on main, then the session takes the log to its branch.
    press(&mut app, KeyCode::Esc);
    pick(&mut app, "main");
    assert_eq!(app.branch_name(), Some("main"));
    keys(&mut app, " rs");
    let detail = app.picker().unwrap().items[0].detail.clone();
    assert!(detail.contains("on feat/x"), "{detail}");
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.branch_name(), Some("feat/x"));
    assert!(matches!(app.screen(), Screen::Commit(_)));
}

#[test]
fn jj_starts_on_the_bookmark_under_at_and_lists_at_with_it() {
    if !have_jj() {
        return;
    }
    let e = JjEnv::new(false);
    e.write("a.txt", "1\n");
    e.jj(&["--quiet", "commit", "-m", "first"]);
    e.write("a.txt", "2\n");
    e.jj(&["--quiet", "commit", "-m", "second"]);
    e.jj(&["--quiet", "bookmark", "create", "main", "-r", "@-"]);
    e.write("f.txt", "1\n");
    e.jj(&["--quiet", "commit", "-m", "f1"]);
    e.jj(&["--quiet", "bookmark", "create", "feat", "-r", "@-"]);
    let tmp = tempfile::tempdir().unwrap();
    let mut app = open_with(&e.root, opts(tmp.path())).unwrap();
    // `@` (empty, on f1) has no bookmark; `@-` has `feat`.
    assert_eq!(app.branch_name(), Some("feat"));
    assert_eq!(rows(&app), ["", "f1"], "@ and what trunk lacks");
    pick(&mut app, "main");
    assert_eq!(rows(&app), ["second", "first"]);
}
