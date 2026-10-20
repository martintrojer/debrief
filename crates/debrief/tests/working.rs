//! The working copy in the app: git's
//! synthetic row, its commit view, comments on it, refresh; jj's `@`
//! after a snapshot. Temp repos and cache dirs; nothing sleeps except the
//! tree-watcher test, which waits on its debounced channel.

mod common;

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use common::{JjEnv, commit, git, have_jj};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use debrief::app::comment::{MERGE_OLD_SIDE, OUTSIDE_ROOT};
use debrief::app::watch::{TreeFilter, tree_watcher};
use debrief::app::{App, AppEvent, Mode, RowKind, Screen, StartOptions, WORKING_ID};
use debrief::seen::Status;
use debrief::vcs::{self, LineKind, OpenOptions};
use debrief_review::{Comment, Rev, Review, Side};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

struct Fixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    cache: PathBuf,
}

/// Two commits (`base`, `second`: a.txt = a1..a5); then, uncommitted,
/// a.txt line 2 is edited and line 4 removed, and new.txt is untracked.
fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir(&root).unwrap();
    git(&root, &["init", "-q"]);
    std::fs::write(root.join("old.txt"), "o\n").unwrap();
    commit(&root, "base");
    std::fs::write(root.join("a.txt"), "a1\na2\na3\na4\na5\n").unwrap();
    commit(&root, "second");
    std::fs::write(root.join("a.txt"), "a1\nA2\na3\na5\n").unwrap();
    std::fs::write(root.join("new.txt"), "n1\nn2\n").unwrap();
    Fixture {
        root: root.canonicalize().unwrap(),
        cache: tmp.path().join("cache"),
        _tmp: tmp,
    }
}

impl Fixture {
    fn app(&self) -> App {
        self.app_with(StartOptions::new(self.cache.clone()))
    }

    fn app_with(&self, opts: StartOptions) -> App {
        App::new(vcs::open(&self.root, OpenOptions::default()).unwrap(), opts).unwrap()
    }

    /// Options with `[view] follow_working` on.
    fn follow_opts(&self) -> StartOptions {
        let mut opts = StartOptions::new(self.cache.clone());
        opts.config.view.follow_working = true;
        opts
    }

    /// The app with the working copy open.
    fn open(&self) -> App {
        self.open_in(self.app())
    }

    /// The app with the working copy open, following the tree.
    fn open_following(&self) -> App {
        self.open_in(self.app_with(self.follow_opts()))
    }

    fn open_in(&self, mut app: App) -> App {
        keys(&mut app, "gg");
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.screen(), Screen::Commit(0));
        assert!(app.viewing_working_copy());
        app
    }

    fn comments(&self) -> Vec<Comment> {
        Review::open(&self.cache, &self.root)
            .unwrap()
            .comments()
            .to_vec()
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

fn goto(app: &mut App, row: usize) {
    keys(app, &format!("{}G", row + 1));
    assert_eq!(app.commit_cursor().row, row);
}

fn comment_on(app: &mut App, row: usize, body: &str) {
    goto(app, row);
    keys(app, "cc");
    assert_eq!(app.mode(), Mode::Comment, "prompt: {}", app.status());
    keys(app, body);
    press(app, KeyCode::Enter);
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

fn summaries(app: &App) -> Vec<String> {
    app.rows().iter().map(|r| r.info.summary.clone()).collect()
}

#[test]
fn dirty_tree_puts_the_working_copy_row_on_top() {
    let f = fixture();
    let mut app = f.app();
    assert_eq!(summaries(&app), ["Working copy", "second", "base"]);
    assert!(app.rows()[0].working);
    assert_eq!(app.rows()[0].info.commit, WORKING_ID);
    assert_eq!(app.working_copy().unwrap().files, 2);
    let s = screen(&mut app, 80, 6);
    // Row 0 is the branch chip; the working-copy row tops the log.
    assert!(s[0].starts_with(" ⎇ "), "{}", s[0]);
    assert!(s[1].starts_with("✎ Working copy  2 files"), "{}", s[1]);
    assert!(s[5].contains("0 new, working copy dirty"), "{}", s[5]);
    // Marking all seen skips the row; it never becomes new.
    keys(&mut app, "m");
    assert_eq!(app.status(), "marked 2 commits seen");
    assert_eq!(app.rows()[0].status, Status::Seen);
}

#[test]
fn clean_tree_has_no_row() {
    let f = fixture();
    git(&f.root, &["checkout", "-q", "--", "a.txt"]);
    std::fs::remove_file(f.root.join("new.txt")).unwrap();
    let mut app = f.app();
    assert_eq!(summaries(&app), ["second", "base"]);
    let s = screen(&mut app, 80, 6);
    assert!(!s[5].contains("working copy"), "{}", s[5]);
}

#[test]
fn new_commits_count_without_the_row_and_start_the_cursor() {
    let f = fixture();
    drop(f.app());
    std::fs::write(f.root.join("c.txt"), "c\n").unwrap();
    git(&f.root, &["add", "c.txt"]);
    git(&f.root, &["commit", "-q", "-m", "third"]);
    let app = f.app();
    assert_eq!(summaries(&app), ["Working copy", "third", "second", "base"]);
    assert_eq!(app.counts(), (1, 0));
    assert_eq!(app.cursor(), 1, "the oldest unseen commit, not the row");
}

#[test]
fn enter_shows_the_working_copy_diff_with_its_header() {
    let f = fixture();
    let mut app = f.open();
    let rows = app.commit_rows();
    assert_eq!(rows[0].kind, RowKind::Message);
    let head = app.rows()[1].info.commit[..8].to_string();
    assert_eq!(
        rows[0].text,
        format!("Working copy (uncommitted) on {head} second")
    );
    row_of(&app, "A2", RowKind::Line(LineKind::Added));
    row_of(&app, "a4", RowKind::Line(LineKind::Removed));
    row_of(&app, "── new.txt (added) ──", RowKind::FileHeader);
    row_of(&app, "n2", RowKind::Line(LineKind::Added));
    let s = screen(&mut app, 100, 12);
    assert!(s[11].contains("working copy"), "{}", s[11]);
    // ]] goes on to the commits, in log order.
    keys(&mut app, "]]");
    assert_eq!(app.screen(), Screen::Commit(1));
    assert!(!app.viewing_working_copy());
    keys(&mut app, "[[");
    assert!(app.viewing_working_copy());
    keys(&mut app, "[[");
    assert_eq!(app.status(), "no previous commit");
    keys(&mut app, "q");
    assert_eq!(app.cursor(), 0);
}

#[test]
fn comments_on_new_lines_are_working_copy_comments() {
    let f = fixture();
    let mut app = f.open();
    let r = row_of(&app, "A2", RowKind::Line(LineKind::Added));
    comment_on(&mut app, r, "why caps");
    let c = &f.comments()[0];
    assert_eq!(c.path, "a.txt");
    assert_eq!(c.rev, None);
    assert_eq!(c.side, None);
    assert_eq!(c.lines, (2, 2));
    assert_eq!(c.excerpt, "+A2");
    // A context line: also the file on disk.
    let r = row_of(&app, "a3", RowKind::Line(LineKind::Context));
    comment_on(&mut app, r, "ctx");
    assert_eq!(f.comments()[1].lines, (3, 3));
    assert_eq!(f.comments()[1].rev, None);
    assert_eq!(app.review_marked_rows().len(), 2);
}

#[test]
fn comments_on_removed_lines_anchor_to_head_old_side() {
    let f = fixture();
    let mut app = f.open();
    let head = app.rows()[1].info.clone();
    let r = row_of(&app, "a4", RowKind::Line(LineKind::Removed));
    comment_on(&mut app, r, "keep it");
    let c = &f.comments()[0];
    assert_eq!(c.side, Some(Side::Old));
    assert_eq!(c.lines, (4, 4));
    let rev = c.rev.as_ref().unwrap();
    assert_eq!(rev.commit, head.commit);
    assert_eq!(rev.summary.as_deref(), Some("second"));
    assert_eq!(app.review_marked_rows(), [r]);
}

#[test]
fn removed_lines_of_a_renamed_file_use_the_old_path() {
    let f = fixture();
    git(&f.root, &["checkout", "-q", "--", "a.txt"]);
    std::fs::remove_file(f.root.join("new.txt")).unwrap();
    // Staged rename, then an edit: diff-index pairs them up.
    git(&f.root, &["mv", "a.txt", "b.txt"]);
    std::fs::write(f.root.join("b.txt"), "a1\na2\na3\na5\n").unwrap();
    let mut app = f.open();
    let r = row_of(&app, "a4", RowKind::Line(LineKind::Removed));
    comment_on(&mut app, r, "gone");
    let c = &f.comments()[0];
    assert_eq!(c.path, "a.txt", "the path in HEAD");
    assert_eq!(c.side, Some(Side::Old));
    assert_eq!(app.review_marked_rows(), [r]);
}

#[test]
fn old_side_markers_never_match_a_renamed_new_path() {
    let f = fixture();
    git(&f.root, &["checkout", "-q", "--", "a.txt"]);
    std::fs::remove_file(f.root.join("new.txt")).unwrap();
    // HEAD deletes b.txt; an old-side comment on its line 4 stays valid.
    std::fs::write(f.root.join("b.txt"), "b1\nb2\nb3\nb4\nb5\n").unwrap();
    commit(&f.root, "add b");
    git(&f.root, &["rm", "-q", "b.txt"]);
    commit(&f.root, "delete b");
    let head = vcs::open(&f.root, OpenOptions::default())
        .unwrap()
        .log(None, 1)
        .unwrap()
        .remove(0);
    // The working tree renames a.txt -> b.txt and removes a4.
    git(&f.root, &["mv", "a.txt", "b.txt"]);
    std::fs::write(f.root.join("b.txt"), "a1\na2\na3\na5\n").unwrap();
    let mut review = Review::open(&f.cache, &f.root).unwrap();
    let rev = Rev {
        commit: head.commit,
        change: head.change,
        summary: None,
        branch: None,
    };
    review
        .add(Comment::on_diff(
            "b.txt",
            rev,
            Side::Old,
            4..=4,
            "-b4",
            "b4",
        ))
        .unwrap();
    drop(review);
    let app = f.open();
    let r = row_of(&app, "a4", RowKind::Line(LineKind::Removed));
    assert!(
        app.comments_at(r).is_empty(),
        "old side is a.txt, not b.txt"
    );
    assert!(app.review_marked_rows().is_empty());
}

#[test]
fn ramble_comments_show_as_markers() {
    let f = fixture();
    let mut review = Review::open(&f.cache, &f.root).unwrap();
    review
        .add(Comment::on_file("new.txt", 2..=2, "n2", "from ramble"))
        .unwrap();
    // A comment on another file, and one on an older commit, don't mark.
    review
        .add(Comment::on_file("old.txt", 1..=1, "o", "elsewhere"))
        .unwrap();
    drop(review);
    let mut app = f.open();
    let r = row_of(&app, "n2", RowKind::Line(LineKind::Added));
    assert_eq!(app.review_marked_rows(), [r]);
    goto(&mut app, r);
    assert_eq!(app.review_preview().as_deref(), Some("● from ramble"));
    // Not on a commit's view.
    keys(&mut app, "]]");
    assert!(app.review_marked_rows().is_empty());
}

#[test]
fn r_reloads_the_working_copy_in_place() {
    let f = fixture();
    let mut app = f.open();
    let r = row_of(&app, "n2", RowKind::Line(LineKind::Added));
    goto(&mut app, r);
    std::fs::write(f.root.join("new.txt"), "n1\nn2\nn3\n").unwrap();
    keys(&mut app, "r");
    row_of(&app, "n3", RowKind::Line(LineKind::Added));
    assert_eq!(app.commit_cursor().row, r, "cursor row kept");
}

#[test]
fn a_tree_change_leaves_the_open_working_copy_by_default() {
    let f = fixture();
    let mut app = f.open();
    let before = app.commit_rows().to_vec();
    std::fs::write(f.root.join("new.txt"), "n0\nn1\nn2\n").unwrap();
    app.event(AppEvent::WorkingChanged);
    assert_eq!(app.commit_rows(), before, "waits for r");
    keys(&mut app, "r");
    row_of(&app, "n0", RowKind::Line(LineKind::Added));
}

#[test]
fn a_tree_change_reloads_the_open_working_copy_in_place() {
    let f = fixture();
    let mut app = f.open_following();
    let r = row_of(&app, "n2", RowKind::Line(LineKind::Added));
    goto(&mut app, r);
    // An earlier file grows a line: the cursor follows its line down.
    std::fs::write(f.root.join("a.txt"), "a1\nA2\nX\na3\na5\n").unwrap();
    app.event(AppEvent::WorkingChanged);
    row_of(&app, "X", RowKind::Line(LineKind::Added));
    assert_eq!(
        app.commit_cursor().row,
        row_of(&app, "n2", RowKind::Line(LineKind::Added)),
        "cursor kept on its line"
    );
    assert_eq!(app.screen(), Screen::Commit(0));
}

#[test]
fn a_tree_change_waits_while_a_selection_or_draft_is_open() {
    let f = fixture();
    let mut app = f.open_following();
    let r = row_of(&app, "n2", RowKind::Line(LineKind::Added));
    goto(&mut app, r);
    keys(&mut app, "V");
    assert_eq!(app.mode(), Mode::Visual);
    let before = app.commit_rows().to_vec();
    std::fs::write(f.root.join("new.txt"), "n0\nn1\nn2\n").unwrap();
    app.event(AppEvent::WorkingChanged);
    assert_eq!(app.commit_rows(), before, "the selection's rows stay");
    assert_eq!(app.mode(), Mode::Visual);
    press(&mut app, KeyCode::Esc);
    app.event(AppEvent::WorkingChanged);
    row_of(&app, "n0", RowKind::Line(LineKind::Added));
}

#[test]
fn a_tree_change_to_clean_leaves_the_working_copy_view() {
    let f = fixture();
    let mut app = f.open_following();
    git(&f.root, &["checkout", "-q", "--", "a.txt"]);
    std::fs::remove_file(f.root.join("new.txt")).unwrap();
    app.event(AppEvent::WorkingChanged);
    assert_eq!(app.screen(), Screen::Log);
    assert_eq!(summaries(&app), ["second", "base"]);
}

#[test]
fn the_tree_watcher_reloads_the_open_view_off_the_ui_thread() {
    let f = fixture();
    let (mut app, reads, rx) = f.counting_with(f.follow_opts());
    keys(&mut app, "gg");
    press(&mut app, KeyCode::Enter);
    settle_events(&mut app, &rx);
    assert!(app.viewing_working_copy());
    reads.set(0);
    std::fs::write(f.root.join("new.txt"), "n0\nn1\nn2\n").unwrap();
    app.event(AppEvent::WorkingChanged);
    assert_eq!(reads.get(), 0, "read the working copy on the UI thread");
    settle_events(&mut app, &rx);
    row_of(&app, "n0", RowKind::Line(LineKind::Added));
    assert!(app.viewing_working_copy());
}

#[test]
fn r_on_a_clean_tree_leaves_the_working_copy_view() {
    let f = fixture();
    let mut app = f.open();
    git(&f.root, &["checkout", "-q", "--", "a.txt"]);
    std::fs::remove_file(f.root.join("new.txt")).unwrap();
    keys(&mut app, "r");
    assert_eq!(app.status(), "The working copy is clean");
    assert_eq!(app.screen(), Screen::Log, "the obsolete diff is closed");
    assert!(!app.viewing_working_copy());
    assert!(app.commit_rows().is_empty(), "nothing left to comment on");
    assert_eq!(summaries(&app), ["second", "base"]);
}

#[test]
fn a_commit_view_stays_frozen_on_reload() {
    let f = fixture();
    let mut app = f.app();
    keys(&mut app, "ggj");
    press(&mut app, KeyCode::Enter);
    let before = app.commit_rows().to_vec();
    std::fs::write(f.root.join("new.txt"), "changed\n").unwrap();
    keys(&mut app, "r");
    assert_eq!(app.commit_rows(), before);
    assert_eq!(app.screen(), Screen::Commit(1));
}

#[test]
fn working_changed_event_refreshes_only_the_row() {
    let f = fixture();
    git(&f.root, &["checkout", "-q", "--", "a.txt"]);
    std::fs::remove_file(f.root.join("new.txt")).unwrap();
    let mut app = f.app();
    keys(&mut app, "j");
    assert_eq!(summaries(&app), ["second", "base"]);
    std::fs::write(f.root.join("x.txt"), "x\n").unwrap();
    app.event(AppEvent::WorkingChanged);
    assert_eq!(summaries(&app), ["Working copy", "second", "base"]);
    assert_eq!(app.rows()[app.cursor()].info.summary, "base", "cursor kept");
}

#[test]
fn editor_jump_uses_the_file_on_disk_and_refuses_paths_outside() {
    let f = fixture();
    let outside = f.root.parent().unwrap().join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, f.root.join("link")).unwrap();
    let mut app = f.open();
    let r = row_of(&app, "A2", RowKind::Line(LineKind::Added));
    goto(&mut app, r);
    assert_eq!(app.edit_target(), Ok((f.root.join("a.txt"), 2)));
    // An untracked symlink shows its target; its canonical path leaves
    // the repo, so neither the editor nor a comment takes it.
    let target = outside.display().to_string();
    let link = row_of(&app, &target, RowKind::Line(LineKind::Added));
    goto(&mut app, link);
    assert_eq!(app.edit_target(), Err(OUTSIDE_ROOT.to_string()));
    keys(&mut app, "cc");
    assert_eq!(app.mode(), Mode::Normal);
    assert_eq!(app.status(), OUTSIDE_ROOT);
}

#[test]
fn untracked_binary_and_large_files_show_a_placeholder() {
    let f = fixture();
    std::fs::write(f.root.join("bin.dat"), b"a\0b").unwrap();
    std::fs::write(f.root.join("big.txt"), "x\n".repeat(600 * 1024)).unwrap();
    let app = f.open();
    let text: Vec<_> = app
        .commit_rows()
        .iter()
        .filter(|r| r.kind == RowKind::Binary)
        .map(|r| r.text.as_str())
        .collect();
    assert_eq!(text, [vcs::TOO_LARGE, "binary file not shown"]);
}

fn unborn(format: &str) -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir(&root).unwrap();
    git(&root, &["init", "-q", &format!("--object-format={format}")]);
    Fixture {
        root: root.canonicalize().unwrap(),
        cache: tmp.path().join("cache"),
        _tmp: tmp,
    }
}

#[test]
fn unborn_repos_start_and_show_the_working_copy() {
    for format in ["sha1", "sha256"] {
        let f = unborn(format);
        let mut app = f.app();
        assert!(app.rows().is_empty(), "{format}");
        let s = screen(&mut app, 80, 4);
        // Unborn: no branch to list, so the chip names HEAD.
        assert_eq!(s[0], " ⎇ HEAD", "{format}");
        assert!(s[1].contains("no commits"), "{format}: {}", s[1]);
        std::fs::write(f.root.join("first.txt"), "hello\n").unwrap();
        keys(&mut app, "r");
        assert_eq!(summaries(&app), ["Working copy"], "{format}");
        press(&mut app, KeyCode::Enter);
        assert_eq!(
            app.commit_rows()[0].text,
            "Working copy (uncommitted; no commits yet)"
        );
        let r = row_of(&app, "hello", RowKind::Line(LineKind::Added));
        comment_on(&mut app, r, "hi");
        assert_eq!(f.comments()[0].rev, None, "{format}");
    }
}

#[test]
fn launchers_reload_the_working_copy() {
    let f = fixture();
    let root = f.root.clone();
    let mut app = f.open().with_runner(move |_| {
        std::fs::write(root.join("new.txt"), "n1\nn2\nedited\n").unwrap();
        Ok(debrief::app::Exit::Code(0))
    });
    let r = row_of(&app, "n1", RowKind::Line(LineKind::Added));
    goto(&mut app, r);
    keys(&mut app, " o");
    app.run_pending_effect();
    row_of(&app, "edited", RowKind::Line(LineKind::Added));
    assert_eq!(app.commit_cursor().row, r);
}

/// Wait for the watcher to fire within a few seconds.
fn fires(rx: &mpsc::Receiver<()>) -> bool {
    rx.recv_timeout(Duration::from_secs(5)).is_ok()
}

fn drain(rx: &mpsc::Receiver<()>) {
    while rx.recv_timeout(Duration::from_millis(600)).is_ok() {}
}

fn watch(root: &Path) -> (debrief::app::watch::RepoWatcher, mpsc::Receiver<()>) {
    let (tx, rx) = mpsc::channel();
    let w = tree_watcher(root, move || {
        let _ = tx.send(());
    })
    .unwrap();
    drain(&rx);
    (w, rx)
}

#[test]
fn tree_watcher_fires_on_edits_and_skips_ignored_and_git_dir() {
    let f = fixture();
    std::fs::write(f.root.join(".gitignore"), "*.log\nbuild/\n").unwrap();
    commit(&f.root, "ignore");
    // Tracked although it matches a rule.
    std::fs::write(f.root.join("kept.log"), "k\n").unwrap();
    git(&f.root, &["add", "-f", "kept.log"]);
    commit(&f.root, "kept");
    std::fs::create_dir(f.root.join("build")).unwrap();
    let (_w, rx) = watch(&f.root);

    std::fs::write(f.root.join("x.log"), "ignored\n").unwrap();
    std::fs::write(f.root.join("build/out"), "ignored\n").unwrap();
    assert!(!fires(&rx), "ignored paths don't refresh");

    std::fs::write(f.root.join("kept.log"), "k2\n").unwrap();
    assert!(fires(&rx), "a tracked path refreshes even if ignored");
    drain(&rx);

    std::fs::write(f.root.join("a.txt"), "edited\n").unwrap();
    assert!(fires(&rx), "an edit refreshes");
    drain(&rx);

    // Changing the rules: the .gitignore event fires, and the new rule
    // applies afterwards.
    std::fs::write(f.root.join(".gitignore"), "*.log\nbuild/\n*.tmp\n").unwrap();
    assert!(fires(&rx));
    drain(&rx);
    std::fs::write(f.root.join("y.tmp"), "t\n").unwrap();
    assert!(!fires(&rx), "the rebuilt rules ignore *.tmp");

    // A nested .gitignore.
    std::fs::create_dir(f.root.join("sub")).unwrap();
    std::fs::write(f.root.join("sub/.gitignore"), "*.gen\n").unwrap();
    assert!(fires(&rx));
    drain(&rx);
    std::fs::write(f.root.join("sub/z.gen"), "g\n").unwrap();
    assert!(!fires(&rx), "nested rules apply");
    std::fs::write(f.root.join("sub/z.txt"), "g\n").unwrap();
    assert!(fires(&rx));
    drain(&rx);

    // info/exclude.
    std::fs::write(f.root.join(".git/info/exclude"), "*.ex\n").unwrap();
    drain(&rx);
    std::fs::write(f.root.join("w.ex"), "e\n").unwrap();
    assert!(!fires(&rx), "info/exclude applies");
}

#[test]
fn an_external_excludes_file_rebuilds_the_rules_and_refreshes() {
    let f = fixture();
    let excludes = f._tmp.path().join("global-ignore");
    std::fs::write(&excludes, "").unwrap();
    git(
        &f.root,
        &["config", "core.excludesFile", excludes.to_str().unwrap()],
    );
    let (_w, rx) = watch(&f.root);

    std::fs::write(&excludes, "*.tmp\n").unwrap();
    assert!(fires(&rx), "an ignore-file change refreshes the row");
    drain(&rx);
    std::fs::write(f.root.join("ignored.tmp"), "i\n").unwrap();
    assert!(!fires(&rx), "the rebuilt rules ignore *.tmp");

    // Dropping the rule reveals the file: the row must come back.
    std::fs::write(&excludes, "").unwrap();
    assert!(fires(&rx));
    drain(&rx);
    std::fs::write(f.root.join("ignored.tmp"), "i2\n").unwrap();
    assert!(fires(&rx), "no longer ignored");
}

#[test]
fn an_info_exclude_change_updates_the_live_row() {
    let f = fixture();
    git(&f.root, &["checkout", "-q", "--", "a.txt"]);
    std::fs::remove_file(f.root.join("new.txt")).unwrap();
    std::fs::write(f.root.join("scratch.tmp"), "s\n").unwrap();
    let mut app = f.app();
    assert_eq!(app.working_row_index(), Some(0));
    let (_w, rx) = watch(&f.root);
    std::fs::write(f.root.join(".git/info/exclude"), "scratch.tmp\n").unwrap();
    assert!(fires(&rx), "info/exclude refreshes the row");
    app.event(AppEvent::WorkingChanged);
    assert_eq!(
        app.working_row_index(),
        None,
        "only an ignored file is left"
    );
}

#[test]
fn a_tracked_symlink_retargeted_outside_the_root_fires() {
    let f = fixture();
    let one = f._tmp.path().join("one");
    let two = f._tmp.path().join("two");
    std::fs::write(&one, "1").unwrap();
    std::fs::write(&two, "2").unwrap();
    std::os::unix::fs::symlink(&one, f.root.join("link")).unwrap();
    commit(&f.root, "link");
    let (_w, rx) = watch(&f.root);
    std::fs::remove_file(f.root.join("link")).unwrap();
    std::os::unix::fs::symlink(&two, f.root.join("link")).unwrap();
    assert!(fires(&rx), "a tracked link is judged as the link");
}

#[test]
fn a_tracked_symlink_into_an_ignored_file_is_relevant() {
    let f = fixture();
    std::fs::write(f.root.join(".gitignore"), "*.ig\n").unwrap();
    std::fs::write(f.root.join("target.ig"), "t\n").unwrap();
    std::os::unix::fs::symlink("target.ig", f.root.join("link")).unwrap();
    commit(&f.root, "link");
    let mut filter = TreeFilter::new(&f.root).unwrap();
    assert!(filter.is_relevant(&f.root.join("link")));
    // And one pointing outside the root.
    let out = f._tmp.path().join("outside");
    std::fs::write(&out, "o\n").unwrap();
    std::os::unix::fs::symlink(&out, f.root.join("ext")).unwrap();
    commit(&f.root, "ext");
    let mut filter = TreeFilter::new(&f.root).unwrap();
    assert!(filter.is_relevant(&f.root.join("ext")));
}

// jj: @ is the working copy, after a snapshot.

fn jj_app(e: &JjEnv, opts: OpenOptions, revset: Option<&str>) -> App {
    let cache = e.root.parent().unwrap().join("cache");
    App::new(
        vcs::open(&e.root, opts).unwrap(),
        StartOptions {
            revset: revset.map(str::to_string),
            ..StartOptions::new(cache)
        },
    )
    .unwrap()
}

fn jj_sample() -> JjEnv {
    let e = JjEnv::new(false);
    e.write("a.txt", "one\ntwo\n");
    e.jj(&["--quiet", "commit", "-m", "first"]);
    e
}

fn added_texts(app: &App) -> Vec<String> {
    app.commit_rows()
        .iter()
        .filter(|r| r.kind == RowKind::Line(LineKind::Added))
        .map(|r| r.text.clone())
        .collect()
}

#[test]
fn jj_at_shows_the_edit_after_the_snapshot() {
    if !have_jj() {
        return;
    }
    let e = jj_sample();
    e.write("a.txt", "one\ntwo\nthree\n");
    let mut app = jj_app(&e, OpenOptions::default(), None);
    assert_eq!(summaries(&app), ["", "first"]);
    assert!(app.rows()[0].working, "@ is marked");
    assert!(!app.rows()[1].working);
    let s = screen(&mut app, 80, 6);
    assert!(s[0].starts_with(" ⎇ "), "{}", s[0]);
    assert!(s[1].starts_with("✎ "), "{}", s[1]);
    assert!(!s[2].starts_with("✎"), "{}", s[2]);
    keys(&mut app, "gg");
    press(&mut app, KeyCode::Enter);
    assert!(app.viewing_working_copy());
    assert_eq!(added_texts(&app), ["three"]);
    // Edit again; r re-snapshots and reloads @ in place.
    e.write("a.txt", "one\ntwo\nthree\nfour\n");
    let r = row_of(&app, "three", RowKind::Line(LineKind::Added));
    goto(&mut app, r);
    keys(&mut app, "r");
    assert_eq!(added_texts(&app), ["three", "four"]);
    assert_eq!(app.commit_cursor().row, r);
    // The open view is on the new @ commit.
    assert_eq!(app.commit_id(), Some(app.rows()[0].info.commit.as_str()));
}

#[test]
fn jj_no_snapshot_shows_at_as_of_the_last_command() {
    if !have_jj() {
        return;
    }
    let e = jj_sample();
    e.write("a.txt", "one\ntwo\nthree\n");
    let ops = e.op_count();
    let mut app = jj_app(&e, OpenOptions { snapshot: false }, None);
    keys(&mut app, "gg");
    press(&mut app, KeyCode::Enter);
    assert!(added_texts(&app).is_empty());
    keys(&mut app, "r");
    assert!(added_texts(&app).is_empty());
    assert_eq!(e.op_count(), ops);
}

#[test]
fn jj_working_row_is_at_not_the_top_row() {
    if !have_jj() {
        return;
    }
    let e = jj_sample();
    // A sibling of @ newer than it, so @ is not first in the log.
    e.jj(&["--quiet", "new", "root()", "-m", "side"]);
    e.write("s.txt", "s\n");
    e.jj(&["--quiet", "new", "description(glob:first*)"]);
    e.write("a.txt", "one\ntwo\nmore\n");
    e.jj(&[
        "--quiet",
        "describe",
        "-r",
        "description(glob:side*)",
        "-m",
        "side, later",
    ]);
    let app = jj_app(&e, OpenOptions::default(), Some("all() ~ root()"));
    let working: Vec<_> = app
        .rows()
        .iter()
        .filter(|r| r.working)
        .map(|r| r.info.summary.clone())
        .collect();
    assert_eq!(working, [""], "{:?}", summaries(&app));
    assert_eq!(app.rows()[0].info.summary, "side, later");
    // Not in the log: no working row.
    let app = jj_app(&e, OpenOptions::default(), Some("description(glob:first*)"));
    assert!(app.rows().iter().all(|r| !r.working));
}

#[test]
fn jj_comments_on_at() {
    if !have_jj() {
        return;
    }
    let e = jj_sample();
    e.write("a.txt", "one\nTWO\n");
    let mut app = jj_app(&e, OpenOptions::default(), None);
    let parent = app.rows()[1].info.clone();
    keys(&mut app, "gg");
    press(&mut app, KeyCode::Enter);
    let r = row_of(&app, "TWO", RowKind::Line(LineKind::Added));
    comment_on(&mut app, r, "new side");
    let r = row_of(&app, "two", RowKind::Line(LineKind::Removed));
    comment_on(&mut app, r, "old side");
    let cache = e.root.parent().unwrap().join("cache");
    let cs = Review::open(&cache, &e.root).unwrap().comments().to_vec();
    assert_eq!(cs[0].rev, None);
    assert_eq!(cs[0].lines, (2, 2));
    let rev = cs[1].rev.as_ref().unwrap();
    assert_eq!(rev.commit, parent.commit, "@-");
    assert_eq!(rev.change, parent.change);
    assert_eq!(cs[1].side, Some(Side::Old));
    assert_eq!(app.review_marked_rows().len(), 2);
}

#[test]
fn jj_merge_at_refuses_old_side_comments() {
    if !have_jj() {
        return;
    }
    let e = JjEnv::new(false);
    e.write("a.txt", "a\n");
    e.jj(&["--quiet", "commit", "-m", "left"]);
    e.jj(&["--quiet", "new", "root()", "-m", "right"]);
    e.write("b.txt", "b1\nb2\n");
    e.jj(&[
        "--quiet",
        "new",
        "description(glob:left*)",
        "description(glob:right*)",
    ]);
    e.write("b.txt", "b1\nB2\n");
    let mut app = jj_app(&e, OpenOptions::default(), None);
    let at = app.working_row_index().unwrap();
    assert_eq!(app.rows()[at].info.parents.len(), 2);
    keys(&mut app, &format!("{}G", at + 1));
    press(&mut app, KeyCode::Enter);
    assert!(app.viewing_working_copy());
    let r = row_of(&app, "B2", RowKind::Line(LineKind::Added));
    comment_on(&mut app, r, "fine");
    let r = row_of(&app, "b2", RowKind::Line(LineKind::Removed));
    goto(&mut app, r);
    keys(&mut app, "cc");
    assert_eq!(app.mode(), Mode::Normal);
    assert_eq!(app.status(), MERGE_OLD_SIDE);
}

#[test]
fn jj_snapshot_failure_shows_in_the_status_line() {
    if !have_jj() {
        return;
    }
    use std::os::unix::fs::PermissionsExt;
    let e = jj_sample();
    e.write("a.txt", "one\ntwo\nthree\n");
    let lock = |mode| {
        for d in ["op_store", "op_store/operations"] {
            let p = e.root.join(".jj/repo").join(d);
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(mode)).unwrap();
        }
    };
    lock(0o555);
    let app = jj_app(&e, OpenOptions::default(), None);
    lock(0o755);
    assert!(
        app.status().starts_with("jj snapshot failed: "),
        "{}",
        app.status()
    );
    assert_eq!(summaries(&app), ["", "first"], "the log still loads");
}

#[test]
fn search_and_filter_highlight_the_working_copy_row() {
    let f = fixture();
    let mut app = f.app();
    let yellow_at = |app: &mut App, needle: &str| {
        let mut t = Terminal::new(TestBackend::new(80, 12)).unwrap();
        t.draw(|fr| app.render(fr)).unwrap();
        let buf = t.backend().buffer();
        // Row 1: the working-copy row, under the branch chip.
        let line: String = (0..80).map(|x| buf[(x, 1)].symbol().to_string()).collect();
        let at = line.find(needle).unwrap();
        let x = line[..at].chars().count() as u16;
        buf[(x, 1)].bg == debrief::ui::palette::YELLOW
    };
    assert!(!yellow_at(&mut app, "Working"));
    keys(&mut app, "fworking");
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.rows().len(), 1);
    assert!(yellow_at(&mut app, "Working"), "filter hit highlighted");
    press(&mut app, KeyCode::Esc);
    keys(&mut app, "/copy");
    press(&mut app, KeyCode::Enter);
    assert!(yellow_at(&mut app, "copy"), "search hit highlighted");
}

/// A git [`vcs::Vcs`] counting its `working_copy` reads (the full diff,
/// every untracked file read).
struct Counting {
    inner: Box<dyn vcs::Vcs>,
    reads: std::rc::Rc<std::cell::Cell<usize>>,
}

impl vcs::Vcs for Counting {
    fn kind(&self) -> vcs::Kind {
        self.inner.kind()
    }
    fn root(&self) -> &Path {
        self.inner.root()
    }
    fn open_options(&self) -> OpenOptions {
        self.inner.open_options()
    }
    fn refresh(&self, snapshot: bool) -> anyhow::Result<()> {
        self.inner.refresh(snapshot)
    }
    fn working_copy(&self, context: vcs::Context) -> anyhow::Result<Option<vcs::WorkingCopy>> {
        self.reads.set(self.reads.get() + 1);
        self.inner.working_copy(context)
    }
    fn working_status(&self) -> anyhow::Result<Option<vcs::WorkingStatus>> {
        self.inner.working_status()
    }
    fn pinned_op(&self) -> Option<String> {
        self.inner.pinned_op()
    }
    fn working_copy_commit(&self) -> anyhow::Result<Option<String>> {
        self.inner.working_copy_commit()
    }
    fn log(&self, revset: Option<&str>, limit: usize) -> anyhow::Result<Vec<vcs::CommitInfo>> {
        self.inner.log(revset, limit)
    }
    fn show(&self, commit: &str, context: vcs::Context) -> anyhow::Result<vcs::CommitDiff> {
        self.inner.show(commit, context)
    }
    fn stat(&self, commit: &str) -> anyhow::Result<vcs::CommitStat> {
        self.inner.stat(commit)
    }
}

impl Fixture {
    /// The app on a [`Counting`] repo, with an event sender; its read
    /// count starts at zero after the start.
    fn counting(
        &self,
    ) -> (
        App,
        std::rc::Rc<std::cell::Cell<usize>>,
        mpsc::Receiver<AppEvent>,
    ) {
        self.counting_with(StartOptions::new(self.cache.clone()))
    }

    fn counting_with(
        &self,
        opts: StartOptions,
    ) -> (
        App,
        std::rc::Rc<std::cell::Cell<usize>>,
        mpsc::Receiver<AppEvent>,
    ) {
        let reads = std::rc::Rc::new(std::cell::Cell::new(0));
        let v = Counting {
            inner: vcs::open(&self.root, OpenOptions::default()).unwrap(),
            reads: reads.clone(),
        };
        let mut app = App::new(Box::new(v), opts).unwrap();
        let (tx, rx) = mpsc::channel();
        app.set_sender(tx);
        reads.set(0);
        (app, reads, rx)
    }
}

/// Feed every event the workers send until they go quiet.
fn settle_events(app: &mut App, rx: &mpsc::Receiver<AppEvent>) {
    while let Ok(ev) = rx.recv_timeout(Duration::from_secs(3)) {
        app.event(ev);
    }
}

#[test]
fn r_reads_the_working_row_off_the_ui_thread() {
    let f = fixture();
    git(&f.root, &["checkout", "-q", "--", "a.txt"]);
    std::fs::remove_file(f.root.join("new.txt")).unwrap();
    let before = common::git_dir_snapshot(&f.root.join(".git"));
    let (mut app, reads, rx) = f.counting();
    assert_eq!(summaries(&app), ["second", "base"]);
    std::fs::write(f.root.join("x.txt"), "x\n").unwrap();
    keys(&mut app, "r");
    assert_eq!(reads.get(), 0, "r read the working copy on the UI thread");
    settle_events(&mut app, &rx);
    assert_eq!(summaries(&app), ["Working copy", "second", "base"]);
    common::assert_same(&before, &common::git_dir_snapshot(&f.root.join(".git")));
}

#[test]
fn the_tree_watcher_reads_the_working_row_off_the_ui_thread() {
    let f = fixture();
    git(&f.root, &["checkout", "-q", "--", "a.txt"]);
    std::fs::remove_file(f.root.join("new.txt")).unwrap();
    let before = common::git_dir_snapshot(&f.root.join(".git"));
    let (mut app, reads, rx) = f.counting();
    std::fs::write(f.root.join("x.txt"), "x\n").unwrap();
    std::fs::write(f.root.join("y.txt"), "y\n").unwrap();
    app.event(AppEvent::WorkingChanged);
    assert_eq!(
        reads.get(),
        0,
        "the watcher read the working copy on the UI thread"
    );
    settle_events(&mut app, &rx);
    let s = screen(&mut app, 80, 6);
    assert!(s[1].starts_with("✎ Working copy  2 files"), "{}", s[1]);
    std::fs::remove_file(f.root.join("x.txt")).unwrap();
    std::fs::remove_file(f.root.join("y.txt")).unwrap();
    app.event(AppEvent::WorkingChanged);
    settle_events(&mut app, &rx);
    assert_eq!(summaries(&app), ["second", "base"]);
    assert_eq!(reads.get(), 0);
    common::assert_same(&before, &common::git_dir_snapshot(&f.root.join(".git")));
}

#[test]
fn a_touched_but_unchanged_file_leaves_the_tree_clean() {
    let f = fixture();
    git(&f.root, &["checkout", "-q", "--", "a.txt"]);
    std::fs::remove_file(f.root.join("new.txt")).unwrap();
    // Stat data changes, contents don't: still clean.
    let t = std::time::SystemTime::now() + Duration::from_secs(5);
    std::fs::File::options()
        .write(true)
        .open(f.root.join("a.txt"))
        .unwrap()
        .set_modified(t)
        .unwrap();
    let (mut app, _, rx) = f.counting();
    keys(&mut app, "r");
    settle_events(&mut app, &rx);
    assert_eq!(summaries(&app), ["second", "base"]);
}

#[test]
fn the_working_rows_stats_load_off_the_ui_thread() {
    let f = fixture();
    let (mut app, reads, rx) = f.counting();
    keys(&mut app, "gg");
    let t = std::time::Instant::now();
    app.tick(t);
    app.tick(t + debrief::app::stat::STAT_DELAY);
    assert_eq!(
        reads.get(),
        0,
        "the stat read the working copy on the UI thread"
    );
    settle_events(&mut app, &rx);
    let Some(debrief::app::StatPane::Ready(s)) = app.stat_pane() else {
        panic!("no stat: {:?}", app.stat_pane());
    };
    let files: Vec<_> = s
        .files
        .iter()
        .map(|f| (f.path.as_str(), f.added, f.removed))
        .collect();
    assert_eq!(
        files,
        [("a.txt", Some(1), Some(2)), ("new.txt", Some(2), Some(0))]
    );
}

#[test]
fn threaded_r_on_a_clean_tree_leaves_the_working_copy_view() {
    let f = fixture();
    let (mut app, _, rx) = f.counting();
    keys(&mut app, "gg");
    press(&mut app, KeyCode::Enter);
    settle_events(&mut app, &rx);
    assert!(app.viewing_working_copy());
    git(&f.root, &["checkout", "-q", "--", "a.txt"]);
    std::fs::remove_file(f.root.join("new.txt")).unwrap();
    keys(&mut app, "r");
    settle_events(&mut app, &rx);
    assert_eq!(app.screen(), Screen::Log, "the obsolete diff is closed");
    assert_eq!(app.status(), "The working copy is clean");
    assert_eq!(summaries(&app), ["second", "base"]);
}
