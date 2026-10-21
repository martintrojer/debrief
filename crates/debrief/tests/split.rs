//! Side by side: `t`, the minimum width and
//! the resize fallback, the paired rows, `Tab`, comments, `<leader>o`,
//! search, markers and the cursor across a toggle, on a real git repo.

mod common;

use std::path::PathBuf;

use common::{commit, git_repo, w};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use debrief::app::config::{Config, Layout};
use debrief::app::split::too_narrow;
use debrief::app::{App, AppEvent, DiffRow, Mode, RowKind, Screen, StartOptions};
use debrief::vcs::{self, LineKind};
use debrief_review::{Review, Side};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

struct Fixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    cache: PathBuf,
}

/// `base`, then `change`: x.txt replaces lines 2-3 (`r1` `r2`) with three
/// (`a1` `a2` `a3`) between context lines.
fn fixture() -> Fixture {
    fixture_of("x.txt", "c1\nr1\nr2\nc2\n", "c1\na1\na2\na3\nc2\n")
}

/// `base` with `file` = `old`, then `change` with `file` = `new`.
fn fixture_of(file: &str, old: &str, new: &str) -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    git_repo(
        &root,
        &[("base", vec![w(file, old)]), ("change", vec![w(file, new)])],
    );
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
        // These tests search the screen for short texts a commit ID in
        // the header row could contain: no header row.
        opts.config.view.header = false;
        let mut app = App::new(
            vcs::open(&self.root, vcs::OpenOptions::default()).unwrap(),
            opts,
        )
        .unwrap();
        app.event(AppEvent::Resize(200, 30));
        app
    }

    /// `change` open, side by side.
    fn open_split(&self) -> App {
        let mut app = self.app_with("[view]\nlayout = \"split\"\n");
        keys(&mut app, "gg");
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.screen(), Screen::Commit(0));
        assert!(app.commit_split(), "{}", app.status());
        app
    }

    fn comments(&self) -> Vec<debrief_review::Comment> {
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

fn row_of(rows: &[DiffRow], text: &str) -> usize {
    rows.iter()
        .position(|r| r.text == text && matches!(r.kind, RowKind::Line(_)))
        .unwrap_or_else(|| panic!("no line {text:?}"))
}

fn goto(app: &mut App, row: usize) {
    keys(app, &format!("{}G", row + 1));
    assert_eq!(app.commit_cursor().row, row);
}

/// The text of the cursor row on the cursor's side.
fn cur_text(app: &App) -> String {
    app.commit_rows()[app.commit_cursor().row].text.clone()
}

fn kinds_from(rows: &[DiffRow], from: usize) -> Vec<(RowKind, String)> {
    rows[from..]
        .iter()
        .map(|r| (r.kind, r.text.clone()))
        .collect()
}

#[test]
fn a_change_block_pairs_lines_and_pads_the_left_with_context_on_both_sides() {
    let f = fixture();
    let app = f.open_split();
    let (left, right) = (app.split_rows(Side::Old), app.split_rows(Side::New));
    assert_eq!(left.len(), right.len());
    let at = row_of(left, "c1");
    assert_eq!(at, row_of(right, "c1"), "context on both sides, same row");
    use LineKind::*;
    let line = |k, t: &str| (RowKind::Line(k), t.to_string());
    let filler = (RowKind::Filler, String::new());
    assert_eq!(
        kinds_from(left, at),
        [
            line(Context, "c1"),
            line(Removed, "r1"),
            line(Removed, "r2"),
            filler,
            line(Context, "c2")
        ]
    );
    assert_eq!(
        kinds_from(right, at),
        [
            line(Context, "c1"),
            line(Added, "a1"),
            line(Added, "a2"),
            line(Added, "a3"),
            line(Context, "c2")
        ]
    );
}

#[test]
fn each_side_draws_its_own_line_numbers() {
    let f = fixture();
    let mut app = f.open_split();
    let backend = TestBackend::new(200, 30);
    let mut term = Terminal::new(backend).unwrap();
    term.draw(|fr| app.render(fr)).unwrap();
    let buf = term.backend().buffer().clone();
    let text: Vec<String> = (0..30)
        .map(|y| (0..200).map(|x| buf[(x, y)].symbol()).collect::<String>())
        .collect();
    let row = text.iter().find(|l| l.contains("a3")).expect("a3 drawn");
    // Left: filler (no number); right: new line 4.
    assert!(row.contains("╱"), "{row}");
    assert!(row.contains("   4 + a3"), "{row}");
    let row = text.iter().find(|l| l.contains("c2")).unwrap();
    // c2 is old line 4, new line 5.
    assert!(row.contains("   4   c2"), "{row}");
    assert!(row.contains("   5   c2"), "{row}");
    let row = text.iter().find(|l| l.contains("r2")).unwrap();
    assert!(row.contains("   3 - r2"), "{row}");
    assert!(row.contains("   3 + a2"), "{row}");
}

#[test]
fn t_refuses_below_the_minimum_width_and_a_resize_falls_back_and_returns() {
    let f = fixture();
    let mut app = f.app_with("");
    app.event(AppEvent::Resize(120, 30));
    keys(&mut app, "gg");
    press(&mut app, KeyCode::Enter);
    keys(&mut app, "t");
    assert!(!app.commit_split());
    // 120 columns less the file list.
    let have = 120 - 17;
    assert_eq!(app.status(), too_narrow(140, have));
    app.event(AppEvent::Resize(200, 30));
    keys(&mut app, "t");
    assert!(app.commit_split(), "{}", app.status());
    app.event(AppEvent::Resize(120, 30));
    assert!(!app.commit_split(), "falls back to unified");
    assert!(
        app.status().contains("side by side needs 140"),
        "{}",
        app.status()
    );
    assert!(app.split_wanted());
    app.event(AppEvent::Resize(220, 30));
    assert!(app.commit_split(), "back to side by side");
    keys(&mut app, "t");
    assert!(!app.commit_split() && !app.split_wanted());
}

#[test]
fn the_layout_persists_for_the_session_and_comes_from_config() {
    let f = fixture();
    let mut app = f.app_with("");
    keys(&mut app, "gg");
    press(&mut app, KeyCode::Enter);
    keys(&mut app, "t");
    assert!(app.commit_split());
    press(&mut app, KeyCode::Esc);
    keys(&mut app, "j");
    press(&mut app, KeyCode::Enter);
    assert!(app.commit_split(), "the next commit opens side by side");
    assert_eq!(
        Config::parse("[view]\nlayout = \"split\"\n")
            .unwrap()
            .view
            .layout,
        Layout::Split
    );
}

#[test]
fn the_cursor_starts_on_the_new_side_and_tab_switches_incl_filler() {
    let f = fixture();
    let mut app = f.open_split();
    let a3 = row_of(app.commit_rows(), "a3");
    goto(&mut app, a3);
    assert_eq!(app.split_side(), Some(Side::New));
    press(&mut app, KeyCode::Tab);
    // The left of a3 is filler: the nearest old line in the block.
    assert_eq!(app.split_side(), Some(Side::Old));
    assert_eq!(cur_text(&app), "r2");
    press(&mut app, KeyCode::Tab);
    assert_eq!(app.split_side(), Some(Side::New));
    assert_eq!(cur_text(&app), "a2", "same row back");
    // j/k move rows on the current side; w within its text.
    keys(&mut app, "k");
    assert_eq!(cur_text(&app), "a1");
}

#[test]
fn a_comment_on_the_left_is_old_with_old_numbers_on_the_right_new() {
    let f = fixture();
    let mut app = f.open_split();
    let row = row_of(app.commit_rows(), "a1");
    goto(&mut app, row);
    press(&mut app, KeyCode::Tab);
    assert_eq!(cur_text(&app), "r1");
    keys(&mut app, "vjc");
    assert_eq!(app.mode(), Mode::Comment, "{}", app.status());
    keys(&mut app, "left");
    press(&mut app, KeyCode::Enter);
    goto(&mut app, row);
    press(&mut app, KeyCode::Tab);
    keys(&mut app, "cc");
    keys(&mut app, "right");
    press(&mut app, KeyCode::Enter);
    let cs = f.comments();
    assert_eq!(cs.len(), 2);
    assert_eq!(cs[0].side, Some(Side::Old));
    assert_eq!(cs[0].lines, (2, 3));
    assert_eq!(cs[0].excerpt, "-r1\n-r2");
    assert_eq!(cs[1].side, Some(Side::New));
    assert_eq!(cs[1].lines, (2, 2));
    // Markers draw on their own side; ]r and K act on the cursor's side.
    assert_eq!(app.split_marked_rows(Side::Old), [row, row + 1]);
    assert_eq!(app.split_marked_rows(Side::New), [row]);
    assert_eq!(app.split_side(), Some(Side::New));
    keys(&mut app, "gg]r");
    assert_eq!(app.commit_cursor().row, row);
    keys(&mut app, "K");
    assert_eq!(app.popup().unwrap(), "● line 2\nright");
}

#[test]
fn the_comment_box_sits_on_the_cursor_side_off_its_rows() {
    let f = fixture();
    let mut app = f.open_split();
    let row = row_of(app.commit_rows(), "a1");
    goto(&mut app, row);
    press(&mut app, KeyCode::Tab);
    keys(&mut app, "cc");
    let backend = TestBackend::new(200, 30);
    let mut term = Terminal::new(backend).unwrap();
    term.draw(|fr| app.render(fr)).unwrap();
    let area = ratatui::layout::Rect::new(0, 0, 200, 29);
    let diff = app.commit_layout(area).diff;
    let (left, _) = app.split_panes(diff);
    let b = app.comment_box(app.side_pane(diff)).unwrap();
    assert!(
        b.rect.x >= left.x && b.rect.right() <= left.right(),
        "{b:?}"
    );
    let y = (row - app.commit_scroll().0) as u16;
    assert!(
        b.rect.y > y || b.rect.bottom() <= y,
        "never covers its row: {b:?}"
    );
}

#[test]
fn leader_o_on_the_left_uses_the_new_side_neighbour_and_on_the_right_that_line() {
    let f = fixture();
    let mut app = f.open_split();
    let row = row_of(app.commit_rows(), "a2");
    goto(&mut app, row);
    assert_eq!(app.edit_target().unwrap().1, 3);
    press(&mut app, KeyCode::Tab);
    assert_eq!(cur_text(&app), "r2");
    // As in unified: a removed line opens at the next new line in its hunk.
    assert_eq!(app.edit_target().unwrap().1, 2);
    let c2 = row_of(app.commit_rows(), "c2");
    goto(&mut app, c2);
    assert_eq!(app.edit_target().unwrap().1, 5);
}

#[test]
fn search_matches_both_sides_and_lands_on_the_matching_one() {
    let f = fixture();
    let mut app = f.open_split();
    keys(&mut app, "gg/a3");
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.split_side(), Some(Side::New));
    assert_eq!(cur_text(&app), "a3");
    keys(&mut app, "gg/r2");
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.split_side(), Some(Side::Old));
    assert_eq!(cur_text(&app), "r2");
    // Each side keeps its own hits.
    assert!(!app.split_search_hits(Side::Old).is_empty());
    assert!(app.split_search_hits(Side::New).is_empty());
}

#[test]
fn toggling_keeps_the_cursor_on_its_line() {
    let f = fixture();
    let mut app = f.app_with("");
    keys(&mut app, "gg");
    press(&mut app, KeyCode::Enter);
    let r2 = row_of(app.commit_rows(), "r2");
    goto(&mut app, r2);
    keys(&mut app, "t");
    assert_eq!(app.split_side(), Some(Side::Old), "a removed line: left");
    assert_eq!(cur_text(&app), "r2");
    keys(&mut app, "t");
    assert_eq!(cur_text(&app), "r2");
    let a3 = row_of(app.commit_rows(), "a3");
    goto(&mut app, a3);
    keys(&mut app, "t");
    assert_eq!(app.split_side(), Some(Side::New));
    assert_eq!(cur_text(&app), "a3");
    // A left context line comes back as that line.
    let c2 = row_of(app.commit_rows(), "c2");
    goto(&mut app, c2);
    press(&mut app, KeyCode::Tab);
    keys(&mut app, "t");
    assert_eq!(cur_text(&app), "c2");
}

#[test]
fn t_context_keeps_the_side_and_line() {
    let f = fixture();
    let mut app = f.open_split();
    let row = row_of(app.commit_rows(), "a1");
    goto(&mut app, row);
    press(&mut app, KeyCode::Tab);
    keys(&mut app, "T");
    assert!(app.commit_split());
    assert_eq!(app.split_side(), Some(Side::Old));
    assert_eq!(cur_text(&app), "r1");
}

#[test]
fn visual_yank_copies_one_side_without_filler() {
    let f = fixture();
    let mut app = f.open_split();
    let row = row_of(app.commit_rows(), "a1");
    goto(&mut app, row);
    press(&mut app, KeyCode::Tab);
    keys(&mut app, "vjjjy");
    let out = String::from_utf8(app.take_output()).unwrap();
    assert!(out.starts_with("\x1b]52;c;"));
    assert_eq!(app.status(), "copied 4 lines");
}

#[test]
fn a_working_copy_comment_on_the_left_is_on_the_base_revision() {
    let f = fixture();
    std::fs::write(f.root.join("x.txt"), "c1\nw1\nc2\n").unwrap();
    let mut app = f.app_with("[view]\nlayout = \"split\"\n");
    keys(&mut app, "gg");
    press(&mut app, KeyCode::Enter);
    assert!(app.viewing_working_copy() && app.commit_split());
    let row = row_of(app.commit_rows(), "w1");
    goto(&mut app, row);
    keys(&mut app, "cc");
    keys(&mut app, "new");
    press(&mut app, KeyCode::Enter);
    press(&mut app, KeyCode::Tab);
    keys(&mut app, "cc");
    keys(&mut app, "old");
    press(&mut app, KeyCode::Enter);
    let cs = f.comments();
    assert_eq!((cs[0].rev.clone(), cs[0].side), (None, None));
    assert_eq!(cs[0].lines, (2, 2));
    assert_eq!(cs[1].side, Some(Side::Old));
    assert_eq!(
        cs[1].rev.as_ref().unwrap().summary.as_deref(),
        Some("change")
    );
    assert_eq!(cs[1].lines, (2, 2));
}

#[test]
fn tab_from_a_filler_stays_in_its_change_block() {
    let added: String = (1..=8).map(|i| format!("a{i}\n")).collect();
    let removed: String = (1..=8).map(|i| format!("r{i}\n")).collect();
    for ctx in ["", "T"] {
        // One removed, eight added: the left of a8 is filler, nearer the
        // context below than `r` above.
        let f = fixture_of("x.txt", "c1\nr\nc2\n", &format!("c1\n{added}c2\n"));
        let mut app = f.open_split();
        keys(&mut app, ctx);
        let row = row_of(app.commit_rows(), "a8");
        goto(&mut app, row);
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.split_side(), Some(Side::Old));
        assert_eq!(cur_text(&app), "r", "{ctx:?}");
        // Mirrored: eight removed, one added.
        let f = fixture_of("x.txt", &format!("c1\n{removed}c2\n"), "c1\na\nc2\n");
        let mut app = f.open_split();
        keys(&mut app, ctx);
        let row = row_of(app.split_rows(Side::Old), "r8");
        goto(&mut app, row);
        press(&mut app, KeyCode::Tab);
        assert_eq!(cur_text(&app), "r8");
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.split_side(), Some(Side::New));
        assert_eq!(cur_text(&app), "a", "{ctx:?}");
    }
}

#[test]
fn a_resize_below_the_minimum_falls_back_in_visual_mode_on_the_same_lines() {
    let f = fixture();
    let mut app = f.open_split();
    let row = row_of(app.commit_rows(), "a1");
    goto(&mut app, row);
    press(&mut app, KeyCode::Tab);
    keys(&mut app, "vj");
    app.event(AppEvent::Resize(120, 30));
    assert!(!app.commit_split(), "unified below the minimum");
    assert!(
        app.status().contains("side by side needs 140"),
        "{}",
        app.status()
    );
    assert_eq!(app.mode(), Mode::Visual);
    let (lo, hi) = app.selection_rows().unwrap();
    let rows = app.commit_rows();
    assert_eq!(
        (rows[lo].text.as_str(), rows[hi].text.as_str()),
        ("r1", "r2")
    );
    app.event(AppEvent::Resize(200, 30));
    assert!(app.commit_split());
    assert_eq!(app.split_side(), Some(Side::Old));
    let (lo, hi) = app.selection_rows().unwrap();
    let rows = app.commit_rows();
    assert_eq!(
        (rows[lo].text.as_str(), rows[hi].text.as_str()),
        ("r1", "r2")
    );
}

#[test]
fn a_resize_below_the_minimum_falls_back_while_commenting_and_the_box_follows() {
    let f = fixture();
    let mut app = f.open_split();
    let row = row_of(app.commit_rows(), "a3");
    goto(&mut app, row);
    keys(&mut app, "cc");
    app.event(AppEvent::Resize(120, 30));
    assert!(!app.commit_split(), "unified below the minimum");
    assert_eq!(app.mode(), Mode::Comment);
    let (lo, hi) = app.comment_rows().unwrap();
    assert_eq!(lo, hi);
    assert_eq!(app.commit_rows()[lo].text, "a3", "the box re-anchors");
    keys(&mut app, "hi");
    press(&mut app, KeyCode::Enter);
    let cs = f.comments();
    assert_eq!((cs[0].side, cs[0].lines), (Some(Side::New), (4, 4)));
    assert_eq!(cs[0].body, "hi");
}

#[test]
fn left_comments_on_a_renamed_file_use_the_old_path_and_show_markers() {
    let lines: String = (1..=20).map(|i| format!("line {i}\n")).collect();
    let f = fixture_of(
        "old.txt",
        &format!("{lines}first\n"),
        &format!("{lines}gone\n"),
    );
    std::fs::remove_file(f.root.join("old.txt")).unwrap();
    std::fs::write(f.root.join("new.txt"), format!("{lines}added\n")).unwrap();
    commit(&f.root, "rename");
    for ctx in ["", "T"] {
        let mut app = f.open_split();
        keys(&mut app, ctx);
        let ctx_row = row_of(app.split_rows(Side::Old), "line 20");
        let gone = row_of(app.split_rows(Side::Old), "gone");
        goto(&mut app, gone);
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.split_side(), Some(Side::Old));
        keys(&mut app, "kcc");
        keys(&mut app, "left context");
        press(&mut app, KeyCode::Enter);
        keys(&mut app, "jcc");
        keys(&mut app, "left removed");
        press(&mut app, KeyCode::Enter);
        let cs = f.comments();
        let n = cs.len();
        for c in &cs[n - 2..] {
            assert_eq!((c.path.as_str(), c.side), ("old.txt", Some(Side::Old)));
        }
        assert_eq!((cs[n - 2].lines, cs[n - 1].lines), ((20, 20), (21, 21)));
        assert_eq!(app.split_marked_rows(Side::Old), [ctx_row, gone]);
        keys(&mut app, "gg]r");
        assert_eq!(app.commit_cursor().row, ctx_row, "{ctx:?}");
        keys(&mut app, "K");
        assert!(app.popup().unwrap().contains("left context"), "{ctx:?}");
        // Start the next round with no comments.
        std::fs::remove_dir_all(&f.cache).unwrap();
    }
}

#[test]
fn a_left_context_line_is_coloured_by_the_old_side() {
    let f = fixture_of(
        "x.c",
        "/*\nint shared = 1;\n*/\n",
        "// head\nint shared = 1;\n// tail\n",
    );
    for ctx in ["", "T"] {
        let mut app = f.open_split();
        keys(&mut app, ctx);
        let row = row_of(app.split_rows(Side::Old), "int shared = 1;");
        let backend = TestBackend::new(200, 30);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|fr| app.render(fr)).unwrap();
        let buf = term.backend().buffer().clone();
        let diff = app
            .commit_layout(ratatui::layout::Rect::new(0, 0, 200, 29))
            .diff;
        let (l, r) = app.split_panes(diff);
        let y = diff.y + (row - app.commit_scroll().0) as u16;
        let fg_of = |pane: ratatui::layout::Rect, word: &str| {
            let line: String = (pane.x..pane.right())
                .map(|x| buf[(x, y)].symbol())
                .collect();
            let at = line
                .find(word)
                .unwrap_or_else(|| panic!("{word:?} in {line:?}"));
            buf[(pane.x + at as u16, y)].fg
        };
        // Left: inside the old `/*` comment; right: the keyword `int`.
        let comment = app
            .side_spans(&app.split_rows(Side::New)[row - 1], Some(Side::New))
            .and_then(|s| s[0].0.fg)
            .expect("// head coloured");
        assert_eq!(fg_of(l, "int"), comment, "{ctx:?}");
        assert_ne!(fg_of(r, "int"), comment, "{ctx:?}");
    }
}
