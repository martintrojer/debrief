//! Line wrapping in the unified diff (`[view] wrap`, `<leader>w`): long
//! rows break into screen lines with a blank gutter, the view never
//! scrolls sideways, the view scrolls by screen lines (into a row taller
//! than the pane too) and every motion keeps the cursor on screen, clicks
//! and drags land on a continuation line's columns, the comment box sits
//! by a wrapped row and pages by lines, the scrollbar counts lines, a
//! resize keeps the top row, and side by side does not wrap.

mod common;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use common::{ctrl, git_repo, keys, press, w};
use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use debrief::app::commit::gutter_width;
use debrief::app::config::Config;
use debrief::app::{App, AppEvent, FOLLOW_GRACE, Mode, RowKind, Screen, StartOptions};
use debrief::vcs;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;

const COLS: u16 = 100;
const ROWS: u16 = 30;
const WRAP: &str = "[view]\nwrap = true\n";

struct Fixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    cache: PathBuf,
}

/// The long text line `i` of a.txt gets: words, then `END{i}`.
fn long(i: usize) -> String {
    let words: String = (0..40).map(|k| format!("w{k} ")).collect();
    format!("long {i} {words}END{i}")
}

/// Line 100 of a.txt: taller than the pane at [`COLS`].
fn huge() -> String {
    let words: String = (0..700).map(|k| format!("h{k} ")).collect();
    format!("huge {words}END100")
}

/// The last line `change` appends to a.txt: also taller than the pane.
fn last() -> String {
    format!("{}ZEND", "z ".repeat(2000))
}

/// A repo of `base` then `change` (log row 0), `a.txt` going from
/// `base` to `change` lines.
fn repo(base: &[String], change: &[String]) -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    git_repo(
        &root,
        &[
            ("base", vec![w("a.txt", base.join("\n") + "\n")]),
            ("change", vec![w("a.txt", change.join("\n") + "\n")]),
        ],
    );
    Fixture {
        root: root.canonicalize().unwrap(),
        cache: tmp.path().join("cache"),
        _tmp: tmp,
    }
}

/// Every fourth of a.txt's 120 lines becomes a long line (several screen
/// lines), line 100 a [`huge`] one, and [`last`] is appended.
fn fixture() -> Fixture {
    let base: Vec<String> = (1..=120).map(|i| format!("line {i}")).collect();
    let mut change: Vec<String> = (1..=120)
        .map(|i| match i {
            100 => huge(),
            i if i % 4 == 0 => long(i),
            i => format!("line {i}"),
        })
        .collect();
    change.push(last());
    repo(&base, &change)
}

impl Fixture {
    /// `change` open in a `cols` x [`ROWS`] terminal, drawn once.
    fn open(&self, toml: &str, cols: u16) -> App {
        let mut opts = StartOptions::new(self.cache.clone());
        opts.config = Config::parse(toml).unwrap();
        opts.config.view.header = false;
        let mut app = App::new(
            vcs::open(&self.root, vcs::OpenOptions::default()).unwrap(),
            opts,
        )
        .unwrap();
        app.event(AppEvent::Resize(cols, ROWS));
        keys(&mut app, "gg");
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.screen(), Screen::Commit(0));
        screen(&mut app);
        app
    }
}

fn screen(app: &mut App) -> Vec<String> {
    let (w, h) = app.size();
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
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

fn diff_rect(app: &App) -> Rect {
    let (w, h) = app.size();
    app.commit_layout(Rect::new(0, 0, w, h - 1)).diff
}

fn view_h(app: &App) -> usize {
    diff_rect(app).height as usize
}

/// The first text column of the diff pane.
fn text_x(app: &App) -> usize {
    diff_rect(app).x as usize + gutter_width(app.commit_num_width())
}

/// Screen lines rows `from..to` take.
fn lines(app: &App, from: usize, to: usize) -> usize {
    (from..to).map(|r| app.commit_segments(r).len()).sum()
}

/// The first screen line of `row`.
fn line_of(app: &App, row: usize) -> usize {
    lines(app, 0, row)
}

/// The screen line of the cursor.
fn cursor_line(app: &App) -> usize {
    let c = app.commit_cursor();
    let segs = app.commit_segments(c.row);
    line_of(app, c.row) + segs.iter().filter(|&&s| s <= c.col).count() - 1
}

/// The screen row of screen line `l` (it must be on screen).
fn y_of(app: &App, l: usize) -> u16 {
    let top = app.commit_scroll().0;
    assert!(l >= top && l < top + view_h(app), "line {l} off screen");
    diff_rect(app).y + (l - top) as u16
}

/// The row of a.txt's line `text` starts with.
fn row_of(app: &App, text: &str) -> usize {
    app.commit_rows()
        .iter()
        .position(|r| matches!(r.kind, RowKind::Line(_)) && r.text.starts_with(text))
        .unwrap_or_else(|| panic!("no row {text}"))
}

fn goto(app: &mut App, row: usize) {
    keys(app, &format!("{}gg", row + 1));
    assert_eq!(app.commit_cursor().row, row);
}

/// The cursor's line is on screen, and its whole row when it fits.
fn assert_cursor_shown(app: &App, after: &str) {
    let (top, h) = (app.commit_scroll().0, view_h(app));
    let l = cursor_line(app);
    assert!(
        l >= top && l < top + h,
        "{after}: cursor line {l} off the view {top}..{}",
        top + h
    );
    let row = app.commit_cursor().row;
    let (start, n) = (line_of(app, row), app.commit_segments(row).len());
    if n <= h {
        assert!(
            start >= top && start + n <= top + h,
            "{after}: row {row} (lines {start}..{}) not whole in {top}..{}",
            start + n,
            top + h
        );
    }
    if app.commit_wraps() {
        assert_eq!(app.commit_scroll().1, 0, "{after}: no sideways scroll");
    }
}

fn mouse(app: &mut App, kind: MouseEventKind, x: u16, y: u16, at: Instant) {
    let ev = MouseEvent {
        kind,
        column: x,
        row: y,
        modifiers: KeyModifiers::NONE,
    };
    app.mouse(ev, at);
}

fn click(app: &mut App, x: u16, y: u16) {
    let at = Instant::now();
    mouse(app, MouseEventKind::Down(MouseButton::Left), x, y, at);
    mouse(app, MouseEventKind::Up(MouseButton::Left), x, y, at);
}

#[test]
fn long_rows_wrap_after_a_blank_gutter() {
    let f = fixture();
    let mut app = f.open(WRAP, COLS);
    assert!(app.commit_wraps());
    let r = row_of(&app, "long 8 ");
    goto(&mut app, r);
    keys(&mut app, "$");
    let s = screen(&mut app);
    assert_eq!(app.commit_scroll().1, 0, "$ does not scroll sideways");
    let segs = app.commit_segments(r);
    assert!(segs.len() >= 3, "the row wraps: {segs:?}");
    let y = y_of(&app, line_of(&app, r)) as usize;
    assert!(s[y].contains("long 8 "), "{}", s[y]);
    let end = y + segs.len() - 1;
    assert!(s[end].trim_end().ends_with("END8"), "{}", s[end]);
    // A continuation line's gutter is blank, its text the row's next
    // columns.
    let (dx, tx) = (diff_rect(&app).x as usize, text_x(&app));
    let gutter: String = s[y + 1].chars().skip(dx).take(tx - dx).collect();
    assert!(gutter.trim().is_empty(), "{gutter:?}");
    let text = &app.commit_rows()[r].text;
    let want: String = text.chars().skip(segs[1]).take(segs[2] - segs[1]).collect();
    let got: String = s[y + 1].chars().skip(tx).take(want.len()).collect();
    assert_eq!(got, want);
}

#[test]
fn scroll_motions_keep_the_cursor_on_screen() {
    let f = fixture();
    let mut app = f.open(WRAP, COLS);
    let h = view_h(&app);
    let last = app.commit_rows().len() - 1;
    let total = lines(&app, 0, last + 1);
    let step = |app: &mut App, what: &str, f: &dyn Fn(&mut App)| {
        f(app);
        screen(app);
        assert_cursor_shown(app, what);
    };
    step(&mut app, "G", &|a| keys(a, "G"));
    assert_eq!(app.commit_cursor().row, last);
    step(&mut app, "$", &|a| keys(a, "$"));
    assert_eq!(
        app.commit_scroll().0,
        total - h,
        "$ on the last row: the end"
    );
    step(&mut app, "gg", &|a| keys(a, "gg"));
    assert_eq!(app.commit_scroll().0, 0);
    // C-f pages a screenful of lines at a time to the end; C-b back.
    for _ in 0..80 {
        let before = app.commit_scroll().0;
        step(&mut app, "C-f", &|a| ctrl(a, 'f'));
        if app.commit_cursor().row == last {
            break;
        }
        assert_eq!(app.commit_scroll().0, before + h, "C-f: a page of lines");
    }
    assert_eq!(app.commit_cursor().row, last, "C-f reaches the end");
    for _ in 0..80 {
        step(&mut app, "C-b", &|a| ctrl(a, 'b'));
    }
    assert_eq!(app.commit_cursor().row, 0, "C-b reaches the top");
    for k in ["C-d", "C-d", "C-u", "C-e", "C-e", "C-y"] {
        let c = k.chars().last().unwrap();
        step(&mut app, k, &|a| ctrl(a, c));
    }
    for k in ["40j", "zz", "zb", "zt", "H", "L", "M", "15k", "3L", "2H"] {
        step(&mut app, k, &|a| keys(a, k));
    }
    let r = row_of(&app, "long 60 ");
    let n = app.commit_segments(r).len();
    goto(&mut app, r);
    keys(&mut app, "zb");
    assert_eq!(app.commit_scroll().0 + h, line_of(&app, r) + n, "zb");
    keys(&mut app, "zt");
    assert_eq!(app.commit_scroll().0, line_of(&app, r), "zt");
    keys(&mut app, "zz");
    let above = line_of(&app, r) - app.commit_scroll().0;
    let below = h - above - n;
    assert!(
        above.abs_diff(below) <= 1,
        "zz: {above} above, {below} below"
    );
}

#[test]
fn a_row_taller_than_the_view_scrolls_into_view_line_by_line() {
    let f = fixture();
    let mut app = f.open(WRAP, COLS);
    let h = view_h(&app);
    let r = row_of(&app, "huge ");
    let n = app.commit_segments(r).len();
    assert!(n > h + 5, "taller than the view: {n} lines in {h}");
    goto(&mut app, r);
    keys(&mut app, "zt");
    assert_eq!(app.commit_scroll().0, line_of(&app, r));
    // `$`: the view follows the cursor to the row's last line.
    keys(&mut app, "$");
    let s = screen(&mut app);
    assert_cursor_shown(&app, "$");
    assert_eq!(cursor_line(&app), line_of(&app, r) + n - 1);
    let y = y_of(&app, cursor_line(&app)) as usize;
    assert!(s[y].trim_end().ends_with("END100"), "{}", s[y]);
    // C-d / C-u walk through the row half a page at a time.
    keys(&mut app, "0");
    keys(&mut app, "zt");
    let mut prev = (app.commit_scroll().0, cursor_line(&app));
    for _ in 0..3 {
        ctrl(&mut app, 'd');
        screen(&mut app);
        assert_cursor_shown(&app, "C-d");
        let now = (app.commit_scroll().0, cursor_line(&app));
        assert_eq!(now, (prev.0 + h / 2, prev.1 + h / 2), "C-d moves on");
        assert_eq!(app.commit_cursor().row, r, "still in the tall row");
        prev = now;
    }
    ctrl(&mut app, 'u');
    assert_eq!(cursor_line(&app), prev.1 - h / 2, "C-u moves back");
    // C-e / C-y scroll a line within it; the cursor stays on screen.
    for _ in 0..h {
        ctrl(&mut app, 'e');
        screen(&mut app);
        assert_cursor_shown(&app, "C-e");
    }
    // j leaves the row, k comes back to it.
    goto(&mut app, r);
    keys(&mut app, "j");
    assert_eq!(app.commit_cursor().row, r + 1);
    keys(&mut app, "k");
    screen(&mut app);
    assert_cursor_shown(&app, "k");
}

#[test]
fn leader_w_toggles_and_side_by_side_never_wraps() {
    let f = fixture();
    let mut app = f.open("", 160);
    let r = row_of(&app, "long 8 ");
    assert!(!app.commit_wraps());
    assert_eq!(app.commit_segments(r).len(), 1);
    keys(&mut app, " w");
    screen(&mut app);
    assert!(app.commit_wraps());
    assert!(app.commit_segments(r).len() > 1);
    keys(&mut app, "t");
    screen(&mut app);
    assert!(app.commit_split());
    assert!(!app.commit_wraps(), "side by side does not wrap");
    keys(&mut app, "t");
    screen(&mut app);
    assert!(app.commit_wraps(), "back to unified, wrapped again");
    keys(&mut app, " w");
    screen(&mut app);
    assert!(!app.commit_wraps());
}

#[test]
fn turning_wrap_on_or_resizing_keeps_the_top_row() {
    let f = fixture();
    let mut app = f.open("", COLS);
    let r = row_of(&app, "long 60 ");
    goto(&mut app, r);
    keys(&mut app, "zt");
    screen(&mut app);
    assert_eq!(app.commit_top(), (r, 0));
    keys(&mut app, " w");
    screen(&mut app);
    assert_eq!(app.commit_top(), (r, 0), "wrap on: same top row");
    assert_eq!(app.commit_scroll().0, line_of(&app, r));
    app.event(AppEvent::Resize(140, ROWS));
    screen(&mut app);
    assert_eq!(app.commit_top(), (r, 0), "wider: same top row");
    assert_eq!(app.commit_scroll().0, line_of(&app, r));
    assert_cursor_shown(&app, "resize");
    keys(&mut app, " w");
    screen(&mut app);
    assert_eq!(app.commit_top(), (r, 0), "wrap off: same top row");
    assert_eq!(app.commit_scroll().0, r);
}

#[test]
fn a_click_on_a_continuation_line_lands_on_its_columns() {
    let f = fixture();
    let mut app = f.open(WRAP, COLS);
    let r = row_of(&app, "long 8 ");
    let segs = app.commit_segments(r);
    let (x, y) = (text_x(&app) as u16 + 3, y_of(&app, line_of(&app, r) + 1));
    click(&mut app, x, y);
    assert_eq!(app.commit_cursor().row, r);
    assert_eq!(app.commit_cursor().col, segs[1] + 3);
}

#[test]
fn a_click_past_an_early_break_stays_on_its_line() {
    // One of the two breaks a cell early (a wide grapheme would cross
    // the edge), whichever the text width's parity.
    let wide = ["日".repeat(100), format!("a{}", "日".repeat(100))];
    let f = repo(&["x".to_string()], &wide);
    let mut app = f.open(WRAP, COLS);
    let tw = diff_rect(&app).width as usize - gutter_width(app.commit_num_width());
    let r = (0..app.commit_rows().len())
        .find(|&r| app.commit_segments(r).get(1) == Some(&(tw - 1)))
        .expect("a row breaking a cell early");
    let (x, y) = ((text_x(&app) + tw - 1) as u16, y_of(&app, line_of(&app, r)));
    click(&mut app, x, y);
    assert_eq!(app.commit_cursor().row, r);
    assert!(
        app.commit_cursor().col < tw - 1,
        "on the line's last grapheme, not the next line's first: {}",
        app.commit_cursor().col
    );
    assert_eq!(cursor_line(&app), line_of(&app, r));
}

#[test]
fn a_drag_onto_a_continuation_line_selects_to_its_row() {
    let f = fixture();
    let mut app = f.open(WRAP, COLS);
    let (a, b) = (row_of(&app, "line 2"), row_of(&app, "long 4 "));
    let x = text_x(&app) as u16;
    let at = Instant::now();
    let y = y_of(&app, line_of(&app, a));
    mouse(&mut app, MouseEventKind::Down(MouseButton::Left), x, y, at);
    let y = y_of(&app, line_of(&app, b) + 1);
    mouse(&mut app, MouseEventKind::Drag(MouseButton::Left), x, y, at);
    assert_eq!(app.mode(), Mode::Visual);
    assert_eq!(app.selection_rows(), Some((a, b)));
}

#[test]
fn the_scrollbar_counts_screen_lines() {
    // Fewer rows than the pane is high, more lines.
    let base: Vec<String> = (1..=6).map(|i| format!("line {i}")).collect();
    let change: Vec<String> = (1..=6)
        .map(|i| format!("{i} {}", "x ".repeat(400)))
        .collect();
    let f = repo(&base, &change);
    let mut app = f.open(WRAP, COLS);
    let h = view_h(&app);
    assert!(app.commit_rows().len() < h);
    assert!(app.commit_total_lines() > h);
    ctrl(&mut app, 'e');
    let s = screen(&mut app);
    let d = diff_rect(&app);
    let x = (d.x + d.width - 1) as usize;
    let thumb = (d.y..d.y + d.height)
        .filter(|&y| s[y as usize].chars().nth(x) == Some('┃'))
        .count();
    assert!(thumb > 0 && thumb < h, "a thumb on the right edge: {thumb}");
}

/// The box sits right by rows `y0..y0 + n` (on screen), above or below.
fn assert_box_beside(app: &App, y0: u16, n: u16) {
    let b = app
        .comment_box(diff_rect(app))
        .expect("the box is open")
        .rect;
    assert!(
        b.y == y0 + n || b.y + b.height == y0,
        "the box at {}..{} is not beside the lines {y0}..{}",
        b.y,
        b.y + b.height,
        y0 + n
    );
}

#[test]
fn the_comment_box_sits_beside_a_wrapped_row() {
    let f = fixture();
    let mut app = f.open(WRAP, COLS);
    let r = row_of(&app, "long 60 ");
    let n = app.commit_segments(r).len();
    goto(&mut app, r);
    keys(&mut app, "zz");
    keys(&mut app, "cc");
    assert_eq!(app.mode(), Mode::Comment);
    screen(&mut app);
    assert_box_beside(&app, y_of(&app, line_of(&app, r)), n as u16);
}

#[test]
fn the_comment_box_scrolls_a_selection_ending_in_a_wrapped_row_into_room() {
    let f = fixture();
    let mut app = f.open(WRAP, COLS);
    let h = view_h(&app);
    let r = row_of(&app, "long 60 ");
    let n = app.commit_segments(r).len();
    goto(&mut app, r);
    keys(&mut app, "zb");
    // A selection up from the row filling all but a line of the view:
    // no room for the box above or below it.
    keys(&mut app, "V");
    let mut top = r;
    while lines(&app, top - 1, r + 1) < h {
        keys(&mut app, "k");
        top -= 1;
    }
    let before = app.commit_scroll().0;
    keys(&mut app, "c");
    assert_eq!(app.mode(), Mode::Comment);
    screen(&mut app);
    assert!(app.commit_scroll().0 > before, "scrolled to make room");
    // The selection's last lines sit just above the box.
    let end = line_of(&app, r) + n;
    let b = app
        .comment_box(diff_rect(&app))
        .expect("the box is open")
        .rect;
    assert_eq!(b.y, y_of(&app, end - 1) + 1, "the box right under the row");
}

#[test]
fn the_comment_box_on_a_row_taller_than_the_view_keeps_the_row() {
    let f = fixture();
    let mut app = f.open(WRAP, COLS);
    let r = row_of(&app, "huge ");
    goto(&mut app, r);
    keys(&mut app, "zt");
    keys(&mut app, "cc");
    screen(&mut app);
    keys(&mut app, "note");
    screen(&mut app);
    assert_eq!(app.mode(), Mode::Comment);
    assert_eq!(app.commit_cursor().row, r, "the cursor stays on the row");
    let (top, h) = (app.commit_scroll().0, view_h(&app));
    let (start, n) = (line_of(&app, r), app.commit_segments(r).len());
    assert!(
        start + n > top && start < top + h,
        "the row is on screen: lines {start}..{} in {top}..{}",
        start + n,
        top + h
    );
    assert_cursor_shown(&app, "cc");
}

#[test]
fn a_comment_on_a_last_row_taller_than_the_view_does_not_panic() {
    let f = fixture();
    let mut app = f.open(WRAP, COLS);
    keys(&mut app, "G");
    screen(&mut app);
    keys(&mut app, "cc");
    screen(&mut app);
    keys(&mut app, "x");
    screen(&mut app);
    assert_eq!(app.mode(), Mode::Comment);
    assert_cursor_shown(&app, "cc on the last row");
}

#[test]
fn following_the_editor_scrolls_only_to_an_off_screen_row() {
    let f = fixture();
    let mut app = f.open(WRAP, COLS);
    let later = || Instant::now() + FOLLOW_GRACE * 2 + Duration::from_millis(1);
    let path = f.root.join("a.txt");
    // Line 60's row is far below the view: centred.
    app.editor_at(&path, 60, later());
    screen(&mut app);
    let r = row_of(&app, "long 60 ");
    assert_eq!(app.commit_cursor().row, r);
    let top = app.commit_scroll().0;
    assert!(line_of(&app, r) > top + 3, "centred, not at the edge");
    assert_cursor_shown(&app, "follow");
    // A row whose first line is on screen: the view stays.
    let near = app.commit_rows()[app.commit_top().0 + 1].clone();
    let line = app.commit_diff().unwrap().files[near.file.unwrap()].hunks[near.hunk.unwrap()].lines
        [near.line.unwrap()]
    .new
    .unwrap();
    app.editor_at(&path, line as usize, later());
    assert_eq!(app.commit_scroll().0, top, "visible: no scroll");
}

#[test]
fn c_e_and_c_y_always_move_the_view() {
    let f = fixture();
    for (cols, rows) in [(COLS, ROWS), (60, 12), (40, 8), (12, 12)] {
        let mut app = f.open(WRAP, cols);
        app.event(AppEvent::Resize(cols, rows));
        screen(&mut app);
        let max = app.commit_total_lines() - view_h(&app);
        for r in 0..app.commit_rows().len() {
            for place in ["zt", "zb", "zz"] {
                for col in ["0", "$"] {
                    goto(&mut app, r);
                    keys(&mut app, place);
                    keys(&mut app, col);
                    screen(&mut app);
                    let at = format!("{cols}x{rows} row {r} {place}{col}");
                    let s = app.commit_scroll().0;
                    ctrl(&mut app, 'e');
                    screen(&mut app);
                    assert_cursor_shown(&app, &format!("C-e {at}"));
                    assert!(
                        app.commit_scroll().0 > s || s >= max,
                        "C-e stuck: {at} at {s}"
                    );
                    let s = app.commit_scroll().0;
                    ctrl(&mut app, 'y');
                    screen(&mut app);
                    assert_cursor_shown(&app, &format!("C-y {at}"));
                    assert!(
                        app.commit_scroll().0 < s || s == 0,
                        "C-y stuck: {at} at {s}"
                    );
                }
            }
        }
    }
}

#[test]
fn h_m_l_do_not_scroll() {
    let f = fixture();
    let mut app = f.open(WRAP, COLS);
    let r = row_of(&app, "long 60 ");
    for start in ["zt", "zb", "zz"] {
        goto(&mut app, r);
        keys(&mut app, start);
        // A line down, so the top row may be cut off.
        ctrl(&mut app, 'e');
        screen(&mut app);
        for k in ["H", "M", "L", "3H", "3L"] {
            let s = app.commit_scroll().0;
            keys(&mut app, k);
            screen(&mut app);
            assert_eq!(app.commit_scroll().0, s, "{start} C-e {k} scrolled");
            assert_cursor_shown(&app, k);
        }
    }
}

#[test]
fn half_pages_keep_the_desired_column() {
    let f = fixture();
    let mut app = f.open(WRAP, COLS);
    // "line N" rows are short: `$` sticks to each line's end.
    let r = row_of(&app, "line 9");
    goto(&mut app, r);
    keys(&mut app, "$");
    ctrl(&mut app, 'd');
    ctrl(&mut app, 'u');
    keys(&mut app, "j");
    let c = app.commit_cursor();
    let len = app.commit_rows()[c.row].text.chars().count();
    assert_eq!(c.col, len - 1, "still at the line's end");
}

#[test]
fn paging_with_the_comment_box_open_scrolls_by_lines() {
    let f = fixture();
    let mut app = f.open(WRAP, COLS);
    let r = row_of(&app, "long 80 ");
    goto(&mut app, r);
    keys(&mut app, "cc");
    screen(&mut app);
    let mut prev = app.commit_scroll().0;
    for _ in 0..3 {
        press(&mut app, KeyCode::PageDown);
        screen(&mut app);
        let now = app.commit_scroll().0;
        assert!(now > prev, "PageDown moved the view: {prev} -> {now}");
        assert_cursor_shown(&app, "PageDown");
        prev = now;
    }
    for _ in 0..3 {
        press(&mut app, KeyCode::PageUp);
        screen(&mut app);
        let now = app.commit_scroll().0;
        assert!(now < prev, "PageUp moved the view: {prev} -> {now}");
        assert_cursor_shown(&app, "PageUp");
        prev = now;
    }
    assert_eq!(app.mode(), Mode::Comment);
}

#[test]
fn a_drag_past_an_edge_scrolls_one_line_to_the_next_whole_row() {
    let f = fixture();
    let mut app = f.open(WRAP, COLS);
    let d = diff_rect(&app);
    let x = text_x(&app) as u16;
    let mid = d.y + d.height / 2;
    let rows = app.commit_rows().len();
    for r in (4..rows - 4).step_by(3) {
        for (place, up) in [("zt", true), ("zb", false)] {
            if app.mode() == Mode::Visual {
                press(&mut app, KeyCode::Esc);
            }
            goto(&mut app, r);
            keys(&mut app, place);
            // Two lines on, so the row at the edge may stay cut off when
            // the drag scrolls back one.
            ctrl(&mut app, if up { 'e' } else { 'y' });
            ctrl(&mut app, if up { 'e' } else { 'y' });
            screen(&mut app);
            let s = app.commit_scroll().0;
            let at = Instant::now();
            mouse(
                &mut app,
                MouseEventKind::Down(MouseButton::Left),
                x,
                mid,
                at,
            );
            let edge = if up { d.y } else { d.y + d.height };
            mouse(
                &mut app,
                MouseEventKind::Drag(MouseButton::Left),
                x,
                edge,
                at,
            );
            mouse(&mut app, MouseEventKind::Up(MouseButton::Left), x, edge, at);
            screen(&mut app);
            let want = if up { s.saturating_sub(1) } else { s + 1 };
            let what = format!("row {r} {place}, drag {}", if up { "up" } else { "down" });
            assert_eq!(app.commit_scroll().0, want, "{what}: one line");
            assert_cursor_shown(&app, &what);
        }
    }
}

#[test]
fn paging_with_the_comment_box_open_leaves_the_view_where_it_went() {
    let f = fixture();
    for (cols, rows) in [(40, 10), (60, 12), (COLS, ROWS)] {
        let mut app = f.open(WRAP, cols);
        app.event(AppEvent::Resize(cols, rows));
        screen(&mut app);
        for r in 0..app.commit_rows().len() {
            if !matches!(app.commit_rows()[r].kind, RowKind::Line(_)) {
                continue;
            }
            for k in [KeyCode::PageUp, KeyCode::PageDown] {
                if app.mode() == Mode::Comment {
                    press(&mut app, KeyCode::Esc);
                }
                goto(&mut app, r);
                keys(&mut app, "$cc");
                screen(&mut app);
                assert_eq!(app.mode(), Mode::Comment);
                press(&mut app, k);
                let s = app.commit_scroll().0;
                let what = format!("{cols}x{rows} row {r} {k:?}");
                assert_cursor_shown(&app, &what);
                screen(&mut app);
                assert_eq!(app.commit_scroll().0, s, "{what}: the draw moved the view");
            }
        }
    }
}

#[test]
fn a_drag_below_onto_a_tall_row_selects_to_the_pointers_line() {
    let f = fixture();
    let mut app = f.open(WRAP, COLS);
    let r = row_of(&app, "huge ");
    goto(&mut app, r - 1);
    keys(&mut app, "zb");
    keys(&mut app, "$");
    screen(&mut app);
    let d = diff_rect(&app);
    let (x, at) = (text_x(&app) as u16 + 20, Instant::now());
    let s = app.commit_scroll().0;
    mouse(
        &mut app,
        MouseEventKind::Down(MouseButton::Left),
        x,
        d.y + d.height / 2,
        at,
    );
    for k in 1..=3 {
        mouse(
            &mut app,
            MouseEventKind::Drag(MouseButton::Left),
            x,
            d.bottom(),
            at,
        );
        screen(&mut app);
        assert_eq!(app.commit_scroll().0, s + k, "one line a step");
        assert_eq!(app.commit_cursor().row, r);
        assert_eq!(
            cursor_line(&app),
            s + k + view_h(&app) - 1,
            "on the bottom line"
        );
    }
}

/// A xorshift generator: the property test below is the same every run.
struct Rng(u64);

impl Rng {
    fn n(&mut self, m: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % m as u64) as usize
    }
}

/// Some place on screen keeps the view still: a whole row, or a line of
/// a row taller than the view. (In a pane too short for any, every
/// motion has to scroll to show the cursor's row.)
fn a_place_on_screen(app: &App) -> bool {
    let (top, h) = (app.commit_scroll().0, view_h(app));
    let mut start = 0;
    for r in 0..app.commit_rows().len() {
        let n = app.commit_segments(r).len();
        if start < top + h && start + n > top && (n > h || (start >= top && start + n <= top + h)) {
            return true;
        }
        start += n;
    }
    false
}

#[test]
fn random_keys_and_drags_keep_the_cursor_on_screen_and_the_draw_still() {
    // Wrapped and not: the comment box and the scroll keys share the code.
    const KEYS: &[&str] = &[
        "j",
        "k",
        "5j",
        "9k",
        "w",
        "b",
        "e",
        "$",
        "0",
        "l",
        "20l",
        "gg",
        "G",
        "H",
        "M",
        "L",
        "3H",
        "2L",
        "zz",
        "zt",
        "zb",
        "C-d",
        "C-u",
        "C-f",
        "C-b",
        "C-e",
        "C-y",
        "]c",
        "[c",
        "}",
        "{",
        "V",
        "Esc",
        "cc",
        "x",
        "PgDn",
        "PgUp",
        "Esc",
        "drag-up",
        "drag-down",
        "wheel",
    ];
    let f = fixture();
    let sizes = [(100, 30), (40, 8), (20, 5), (25, 4), (60, 12), (80, 4)];
    let runs = sizes.iter().flat_map(|&s| [(WRAP, s), ("", s)]);
    for (seed, (toml, (cols, rows))) in runs.enumerate() {
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15 ^ (seed as u64 + 1));
        let mut app = f.open(toml, cols);
        app.event(AppEvent::Resize(cols, rows));
        screen(&mut app);
        let mut hist: Vec<&str> = Vec::new();
        for _ in 0..400 {
            let k = KEYS[rng.n(KEYS.len())];
            hist.push(k);
            let d = diff_rect(&app);
            let (x, mid) = (
                text_x(&app).min(cols as usize - 1) as u16,
                d.y + d.height / 2,
            );
            let before = app.commit_scroll().0;
            let normal = app.mode() == Mode::Normal;
            match k {
                "Esc" if normal => continue,
                "Esc" => press(&mut app, KeyCode::Esc),
                "PgDn" => press(&mut app, KeyCode::PageDown),
                "PgUp" => press(&mut app, KeyCode::PageUp),
                "drag-up" | "drag-down" if normal => {
                    let edge = if k == "drag-up" { d.y } else { d.bottom() };
                    let at = Instant::now();
                    mouse(
                        &mut app,
                        MouseEventKind::Down(MouseButton::Left),
                        x,
                        mid,
                        at,
                    );
                    mouse(
                        &mut app,
                        MouseEventKind::Drag(MouseButton::Left),
                        x,
                        edge,
                        at,
                    );
                    mouse(&mut app, MouseEventKind::Up(MouseButton::Left), x, edge, at);
                }
                "wheel" if normal => {
                    mouse(&mut app, MouseEventKind::ScrollDown, x, mid, Instant::now())
                }
                "drag-up" | "drag-down" | "wheel" => continue,
                k if k.contains("C-") => ctrl(&mut app, k.chars().last().unwrap()),
                k => keys(&mut app, k),
            }
            assert_eq!(app.screen(), Screen::Commit(0), "left the commit: {hist:?}");
            let s = app.commit_scroll().0;
            let recent = &hist[hist.len().saturating_sub(8)..];
            let what = format!("{cols}x{rows} {toml:?} after {recent:?}");
            screen(&mut app);
            // The view moves on even where no row fits on screen whole.
            let (now, max) = (
                app.commit_scroll().0,
                app.commit_total_lines() - view_h(&app).min(app.commit_total_lines()),
            );
            if normal && k == "C-e" {
                assert!(
                    now > before || before >= max,
                    "{what}: C-e stuck at {before}"
                );
            }
            if normal && k == "C-y" {
                assert!(now < before || before == 0, "{what}: C-y stuck at {before}");
            }
            if !a_place_on_screen(&app) {
                continue;
            }
            assert_eq!(now, s, "{what}: the draw moved the view");
            assert_cursor_shown(&app, &what);
            if matches!(k, "H" | "M" | "L" | "3H" | "2L") {
                assert_eq!(s, before, "{what}: {k} scrolled");
            }
        }
    }
}
