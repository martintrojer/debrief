//! The log's branch: the current one at start, `--branch`, `<leader>b`
//! on both screens, the working-copy row, comments and sessions recording
//! it, and a branch that goes away.

mod common;

use std::path::Path;

use common::{JjEnv, git, git_branch_repo, have_jj, keys, press};
use crossterm::event::KeyCode;
use debrief::app::{App, Mode, PickerKind, RowKind, Screen, StartOptions};
use debrief::vcs::{self, LineKind, OpenOptions};

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
    let (tmp, root) = git_branch_repo();
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
    let (tmp, root) = git_branch_repo();
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
    let (tmp, root) = git_branch_repo();
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
    let (tmp, root) = git_branch_repo();
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
    let (tmp, root) = git_branch_repo();
    let mut app = open_with(&root, opts(tmp.path())).unwrap();
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.screen(), Screen::Commit(0));
    let before = app.commit_rows().to_vec();
    pick(&mut app, "feat");
    assert!(matches!(app.screen(), Screen::Commit(_)));
    assert_eq!(app.commit_rows(), before, "the open diff stays");
    assert_eq!(rows(&app), ["f2", "f1"]);
    // No row is marked open: the new history lacks the commit.
    assert_eq!(app.commit_list_open_row(), None);
    // `]]` / `[[` step from its old place; feat/x shares no commit with
    // main's [second, first], so there is nothing to step to either way.
    let open = app.commit_id().map(str::to_string);
    keys(&mut app, "]]");
    assert_eq!(
        app.commit_id().map(str::to_string),
        open,
        "nothing older listed"
    );
    assert!(app.status().contains("no next commit"), "{}", app.status());
}

#[test]
fn bracket_bracket_steps_from_a_hidden_commits_old_place() {
    let (tmp, root) = git_branch_repo();
    let mut o = opts(tmp.path());
    o.branch = Some("feat/x".into());
    o.config.log.trunk = Some("main~1".into());
    let mut app = open_with(&root, o).unwrap();
    assert_eq!(rows(&app), ["f2", "f1", "second"]);
    // Open f1, then list HEAD (main: second, first): f1 is gone.
    keys(&mut app, "j");
    press(&mut app, KeyCode::Enter);
    pick(&mut app, "HEAD");
    assert_eq!(rows(&app), ["second", "first"]);
    assert_eq!(app.commit_list_open_row(), None);
    // f1 sat above second: `]]` steps to second.
    keys(&mut app, "]]");
    assert_eq!(app.commit_id(), Some(app.rows()[0].info.commit.as_str()));
}

#[test]
fn a_gone_branch_falls_back_to_the_current_one() {
    let (tmp, root) = git_branch_repo();
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
    let (tmp, root) = git_branch_repo();
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

fn screen(app: &mut App, w: u16, h: u16) -> Vec<String> {
    let mut t = ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
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
fn the_header_names_the_branch_and_drops_it_when_cramped() {
    let (tmp, root) = git_branch_repo();
    let mut app = open_with(&root, opts(tmp.path())).unwrap();
    pick(&mut app, "feat");
    press(&mut app, KeyCode::Enter);
    let wide = screen(&mut app, 100, 10);
    assert!(wide[0].contains(" ⎇ feat/x"), "{:?}", wide[0]);
    assert!(wide[0].contains("f2"), "{:?}", wide[0]);
    // Too narrow for the branch beside the subject: the subject stays.
    let narrow = screen(&mut app, 30, 10);
    assert!(!narrow[0].contains('⎇'), "{:?}", narrow[0]);
    assert!(narrow[0].contains("f2"), "{:?}", narrow[0]);
}

#[test]
fn branch_names_are_exact_not_revisions() {
    let (tmp, root) = git_branch_repo();
    let mut o = opts(tmp.path());
    o.branch = Some("main~1".into());
    assert!(open_with(&root, o).is_err(), "main~1 is not a branch");
}

#[test]
fn a_trunk_that_cannot_be_read_keeps_the_branch() {
    let (tmp, root) = git_branch_repo();
    git(&root, &["branch", "-q", "base", "main~1"]);
    let mut o = opts(tmp.path());
    o.config.log.trunk = Some("base".into());
    let mut app = open_with(&root, o).unwrap();
    pick(&mut app, "feat");
    assert_eq!(rows(&app), ["f2", "f1", "second"]);
    git(&root, &["branch", "-q", "-D", "base"]);
    keys(&mut app, "r");
    assert_eq!(app.branch_name(), Some("feat/x"), "the branch still exists");
    assert!(app.status().contains("trunk base"), "{}", app.status());
    assert!(!app.status().contains("gone"), "{}", app.status());
}

#[test]
fn a_configured_revset_stays_pickable_under_branch() {
    use clap::Parser;
    let (tmp, root) = git_branch_repo();
    let cfg = tmp.path().join("config.toml");
    std::fs::write(&cfg, "[log]\nrevset = \"main~1\"\n[view]\nstats = false\n").unwrap();
    let cli = debrief::cli::Cli::try_parse_from([
        "debrief",
        "--branch",
        "feat/x",
        "--config",
        cfg.to_str().unwrap(),
    ])
    .unwrap();
    let o = cli
        .start_options(
            Some(tmp.path().join("cache")),
            Some(tmp.path().join("state")),
            None,
        )
        .unwrap();
    let mut app = open_with(&root, o).unwrap();
    assert_eq!(rows(&app), ["f2", "f1"], "--branch starts the log");
    pick(&mut app, "revset");
    assert_eq!(app.branch_label(), "revset: main~1");
    assert_eq!(rows(&app), ["first"]);
}

#[test]
fn paging_a_branch_pages_that_branch() {
    let (tmp, root) = git_branch_repo();
    git(&root, &["checkout", "-q", "feat/x"]);
    for i in 3..6 {
        std::fs::write(root.join("f.txt"), format!("{i}\n")).unwrap();
        common::commit(&root, &format!("f{i}"));
    }
    git(&root, &["checkout", "-q", "main"]);
    let mut o = opts(tmp.path());
    o.limit = 2;
    let mut app = open_with(&root, o).unwrap();
    pick(&mut app, "feat");
    assert_eq!(rows(&app), ["f5", "f4"]);
    keys(&mut app, "G");
    keys(&mut app, "G");
    keys(&mut app, "G");
    assert_eq!(
        rows(&app),
        ["f5", "f4", "f3", "f2", "f1"],
        "feat's pages, not main's"
    );
}

#[test]
fn quit_save_keeps_the_branch_each_commit_was_reviewed_under() {
    let (tmp, root) = git_branch_repo();
    let mut app = open_with(&root, opts(tmp.path())).unwrap();
    pick(&mut app, "feat");
    press(&mut app, KeyCode::Enter);
    let r = app
        .commit_rows()
        .iter()
        .position(|r| r.text == "2" && r.kind == RowKind::Line(LineKind::Added))
        .unwrap();
    keys(&mut app, &format!("{}G", r + 1));
    keys(&mut app, "cc");
    keys(&mut app, "ok");
    press(&mut app, KeyCode::Enter);
    let f2 = app.commit_id().unwrap().to_string();
    press(&mut app, KeyCode::Esc);
    // Over on main, another commit changed (its view), then quit-save.
    pick(&mut app, "main");
    press(&mut app, KeyCode::Enter);
    keys(&mut app, "G");
    press(&mut app, KeyCode::Esc);
    keys(&mut app, "q");
    keys(&mut app, "s");
    let store = debrief::session::Store::new(&tmp.path().join("state"), &root);
    let s = store.load(&f2).unwrap().expect("f2's session");
    assert_eq!(s.branch.as_deref(), Some("feat/x"), "reviewed under feat/x");
}

#[test]
fn saving_keeps_the_branch_a_commit_was_opened_under() {
    let (tmp, root) = git_branch_repo();
    let mut app = open_with(&root, opts(tmp.path())).unwrap();
    pick(&mut app, "feat");
    press(&mut app, KeyCode::Enter);
    let f2 = app.commit_id().unwrap().to_string();
    // Pick main with f2 still open, then save it: no comment says where
    // it was reviewed, the visit does.
    pick(&mut app, "main");
    assert_eq!(app.commit_id(), Some(f2.as_str()));
    keys(&mut app, " ss");
    let store = debrief::session::Store::new(&tmp.path().join("state"), &root);
    let s = store.load(&f2).unwrap().expect("f2's session");
    assert_eq!(s.branch.as_deref(), Some("feat/x"), "reviewed under feat/x");
}

#[test]
fn resaving_under_head_records_no_branch() {
    let (tmp, root) = git_branch_repo();
    let mut app = open_with(&root, opts(tmp.path())).unwrap();
    press(&mut app, KeyCode::Enter);
    let id = app.commit_id().unwrap().to_string();
    keys(&mut app, " ss");
    let store = debrief::session::Store::new(&tmp.path().join("state"), &root);
    assert_eq!(
        store.load(&id).unwrap().unwrap().branch.as_deref(),
        Some("main")
    );
    press(&mut app, KeyCode::Esc);
    pick(&mut app, "HEAD");
    press(&mut app, KeyCode::Enter);
    keys(&mut app, "G");
    keys(&mut app, " ss");
    assert_eq!(store.load(&id).unwrap().unwrap().branch, None);
}

#[test]
fn a_working_copy_session_opens_after_its_branch_switch_with_a_sender() {
    let (tmp, root) = git_branch_repo();
    std::fs::write(root.join("dirty.txt"), "d\n").unwrap();
    let mut app = open_with(&root, opts(tmp.path())).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    app.set_sender(tx);
    press(&mut app, KeyCode::Enter);
    while let Ok(ev) = rx.recv_timeout(std::time::Duration::from_secs(3)) {
        app.event(ev);
    }
    assert!(app.viewing_working_copy());
    keys(&mut app, " ss");
    press(&mut app, KeyCode::Esc);
    pick(&mut app, "feat");
    while let Ok(ev) = rx.recv_timeout(std::time::Duration::from_secs(3)) {
        app.event(ev);
    }
    keys(&mut app, " rs");
    press(&mut app, KeyCode::Enter);
    while let Ok(ev) = rx.recv_timeout(std::time::Duration::from_secs(3)) {
        app.event(ev);
    }
    assert_eq!(app.branch_name(), Some("main"));
    assert!(app.viewing_working_copy(), "status: {}", app.status());
    assert!(matches!(app.screen(), Screen::Commit(_)));
}
