//! The mouse (`[mouse] enabled`): hit-testing over the drawn layout (the
//! log and its stats pane, the file list on either side, the commit list,
//! side by side, an auto peek over the diff, the overlays), clicks and
//! double-clicks, dragging a line selection with edge scrolling, the wheel
//! over each pane, the picker and help, mouse events ignored while typing,
//! and capture off when disabled.

mod common;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use common::{commit, ctrl, git, keys, press};
use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use debrief::app::config::Config;
use debrief::app::run::capture_wanted;
use debrief::app::{App, AppEvent, Focus, Hit, MULTI_CLICK, Mode, RowKind, Screen, StartOptions};
use debrief::vcs;
use debrief_review::{Comment, Review, Side};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;

const COLS: u16 = 120;
const ROWS: u16 = 30;
/// Filler commits under `change`.
const FILLERS: usize = 40;
/// Files `change` adds under `src/`.
const SRC_FILES: usize = 40;

struct Fixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    cache: PathBuf,
}

/// `base`, `filler 1` .. `filler 40`, then `change` on top (log row 0):
/// a.txt edits every fifth of its 200 lines (a long diff), b.txt edits
/// its one line, and `src/f00.rs` .. `src/f39.rs` are added (the tree's
/// top rows: the `src` directory, then more file rows than it shows).
fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir(&root).unwrap();
    git(&root, &["init", "-q"]);
    let a: Vec<String> = (1..=200).map(|i| format!("line {i}")).collect();
    std::fs::write(root.join("a.txt"), a.join("\n") + "\n").unwrap();
    std::fs::write(root.join("b.txt"), "b\n").unwrap();
    commit(&root, "base");
    for i in 1..=FILLERS {
        std::fs::write(root.join("f.txt"), format!("{i}\n")).unwrap();
        commit(&root, &format!("filler {i}"));
    }
    let a: Vec<String> = (1..=200)
        .map(|i| {
            if i % 5 == 0 {
                format!("line {i} changed")
            } else {
                format!("line {i}")
            }
        })
        .collect();
    std::fs::write(root.join("a.txt"), a.join("\n") + "\n").unwrap();
    std::fs::write(root.join("b.txt"), "b changed\n").unwrap();
    std::fs::create_dir(root.join("src")).unwrap();
    for i in 0..SRC_FILES {
        std::fs::write(root.join(format!("src/f{i:02}.rs")), "fn f() {}\n").unwrap();
    }
    commit(&root, "change");
    Fixture {
        root: root.canonicalize().unwrap(),
        cache: tmp.path().join("cache"),
        _tmp: tmp,
    }
}

impl Fixture {
    /// The log, `cols` x `rows`, drawn once.
    fn app_sized(&self, toml: &str, cols: u16, rows: u16) -> App {
        let mut opts = StartOptions::new(self.cache.clone());
        opts.config = Config::parse(toml).unwrap();
        let mut app = App::new(
            vcs::open(&self.root, vcs::OpenOptions::default()).unwrap(),
            opts,
        )
        .unwrap();
        app.event(AppEvent::Resize(cols, rows));
        draw(&mut app);
        app
    }

    fn app(&self, toml: &str) -> App {
        self.app_sized(toml, COLS, ROWS)
    }

    /// `change` open, `cols` wide.
    fn open_sized(&self, toml: &str, cols: u16) -> App {
        let mut app = self.app_sized(toml, cols, ROWS);
        keys(&mut app, "gg");
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.screen(), Screen::Commit(0));
        draw(&mut app);
        app
    }

    fn open(&self, toml: &str) -> App {
        self.open_sized(toml, COLS)
    }

    fn review(&self) -> Review {
        Review::open(&self.cache, &self.root).unwrap()
    }
}

fn draw(app: &mut App) {
    let (w, h) = app_size(app);
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| app.render(f)).unwrap();
}

/// The size the app was last resized to (the screen of its last draw).
fn app_size(app: &App) -> (u16, u16) {
    app.size()
}

/// The area above the status line.
fn main_area(app: &App) -> Rect {
    let (w, h) = app_size(app);
    Rect::new(0, 0, w, h - 1)
}

fn ev(kind: MouseEventKind, col: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind,
        column: col,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

/// A clock for presses: each [`Clock::later`] is a second on, so
/// presses never pair into a double-click by accident.
struct Clock(Instant);

impl Clock {
    fn new() -> Self {
        Clock(Instant::now())
    }

    fn later(&mut self) -> Instant {
        self.0 += Duration::from_secs(1);
        self.0
    }
}

/// A left press and release at (`col`, `row`) at `at`.
fn click_at(app: &mut App, col: u16, row: u16, at: Instant) {
    app.mouse(ev(MouseEventKind::Down(MouseButton::Left), col, row), at);
    app.mouse(ev(MouseEventKind::Up(MouseButton::Left), col, row), at);
}

/// A single click.
fn click(app: &mut App, clock: &mut Clock, col: u16, row: u16) {
    click_at(app, col, row, clock.later());
}

/// Two clicks on one cell 100 ms apart.
fn double_click(app: &mut App, clock: &mut Clock, col: u16, row: u16) {
    let t = clock.later();
    click_at(app, col, row, t);
    click_at(app, col, row, t + Duration::from_millis(100));
}

fn wheel(app: &mut App, col: u16, row: u16, down: bool) {
    let kind = if down {
        MouseEventKind::ScrollDown
    } else {
        MouseEventKind::ScrollUp
    };
    app.mouse(ev(kind, col, row), Instant::now());
}

/// The file list's tree inside its border.
fn tree_inner(app: &App) -> Rect {
    let l = app.commit_layout(main_area(app));
    let t = l.sidebar.expect("the file list is shown");
    let right = app.config().view.sidebar_side == debrief::app::config::SidebarSide::Right;
    Rect::new(t.x + u16::from(right), t.y, t.width - 1, t.height)
}

/// The commit list's rows (under its rule) inside its border.
fn commits_inner(app: &App) -> Rect {
    let c = app
        .commit_layout(main_area(app))
        .commits
        .expect("the commit list is shown");
    Rect::new(c.x, c.y + 1, c.width - 1, c.height - 1)
}

fn diff_rect(app: &App) -> Rect {
    app.commit_layout(main_area(app)).diff
}

/// The visible file-list row named `name`.
fn file_row(app: &App, name: &str) -> usize {
    let l = app.file_list().unwrap();
    (0..l.visible.rows.len())
        .find(|&r| l.entry(r).unwrap().name == name)
        .unwrap_or_else(|| panic!("no row {name}"))
}

/// The screen row of file-list row `row` (scrolled as drawn).
fn file_y(app: &App, row: usize) -> u16 {
    let t = tree_inner(app);
    t.y + (row - app.files_skip(t.height as usize)) as u16
}

/// The screen row of diff row `row`.
fn diff_y(app: &App, row: usize) -> u16 {
    diff_rect(app).y + (row - app.commit_scroll().0) as u16
}

// --- capture -----------------------------------------------------------

#[test]
fn capture_follows_mouse_enabled() {
    assert!(capture_wanted(&Config::defaults()), "on by default");
    assert!(capture_wanted(&Config::parse("").unwrap()));
    assert!(!capture_wanted(
        &Config::parse("[mouse]\nenabled = false\n").unwrap()
    ));
}

#[test]
fn disabled_the_mouse_does_nothing() {
    let f = fixture();
    let mut clock = Clock::new();
    let off = "[mouse]\nenabled = false\n";
    let mut app = f.app(off);
    double_click(&mut app, &mut clock, 70, 3);
    wheel(&mut app, 5, 3, true);
    assert_eq!((app.cursor(), app.scroll()), (0, 0));
    assert_eq!(app.screen(), Screen::Log);
    let mut app = f.open(off);
    let d = diff_rect(&app);
    click(&mut app, &mut clock, d.x + 10, d.y + 5);
    wheel(&mut app, d.x + 10, d.y + 5, true);
    app.mouse(
        ev(MouseEventKind::Drag(MouseButton::Left), d.x + 10, d.y + 9),
        Instant::now(),
    );
    assert_eq!(app.commit_cursor().row, 0);
    assert_eq!(app.commit_scroll().0, 0);
    assert_eq!(app.mode(), Mode::Normal);
    let t = tree_inner(&app);
    click(&mut app, &mut clock, t.x + 4, t.y + 2);
    assert_eq!(app.focus(), Focus::Diff);
}

// --- the log -----------------------------------------------------------

#[test]
fn a_click_on_a_log_row_moves_the_cursor_and_a_double_click_opens_it() {
    let f = fixture();
    let mut clock = Clock::new();
    let mut app = f.app("");
    let log = app.log_layout(main_area(&app)).log;
    // Scrolled: the click lands on the row drawn there.
    keys(&mut app, "40j");
    draw(&mut app);
    let top = app.scroll();
    assert!(top > 0);
    assert_eq!(app.hit(log.x + 5, log.y + 2), Hit::Log(top + 2));
    click(&mut app, &mut clock, log.x + 5, log.y + 2);
    assert_eq!(app.cursor(), top + 2);
    assert_eq!(app.screen(), Screen::Log);
    // Two presses on one cell within 400 ms open it.
    double_click(&mut app, &mut clock, log.x + 5, log.y + 4);
    assert_eq!(app.screen(), Screen::Commit(top + 4));
}

#[test]
fn presses_far_apart_or_on_another_cell_are_not_a_double_click() {
    let f = fixture();
    let mut app = f.app("");
    let log = app.log_layout(main_area(&app)).log;
    let t = Instant::now();
    click_at(&mut app, 5, log.y + 3, t);
    click_at(
        &mut app,
        5,
        log.y + 3,
        t + MULTI_CLICK + Duration::from_millis(1),
    );
    assert_eq!(app.screen(), Screen::Log, "over 400 ms");
    let t = t + Duration::from_secs(5);
    click_at(&mut app, 5, log.y + 3, t);
    click_at(&mut app, 6, log.y + 3, t + Duration::from_millis(50));
    assert_eq!(app.screen(), Screen::Log, "another column");
    click_at(&mut app, 6, log.y + 4, t + Duration::from_millis(100));
    assert_eq!(app.screen(), Screen::Log, "another row");
    assert_eq!(app.cursor(), 4);
    click_at(&mut app, 6, log.y + 4, t + Duration::from_millis(500));
    assert_eq!(app.screen(), Screen::Commit(4), "exactly 400 ms apart");
}

#[test]
fn the_wheel_scrolls_the_log_and_keeps_the_cursor_on_screen() {
    let f = fixture();
    let mut app = f.app("");
    let l = app.log_layout(main_area(&app));
    wheel(&mut app, l.log.x + 5, l.log.y + 5, true);
    assert_eq!(app.scroll(), 3);
    assert_eq!(app.cursor(), 3, "pulled onto the screen");
    wheel(&mut app, l.log.x + 5, l.log.y + 5, false);
    assert_eq!(app.scroll(), 0);
    assert_eq!(app.cursor(), 3, "still on screen: stays");
    // Over the stats pane the log stays put.
    let stats = l.stats.expect("the stats pane is on");
    assert_eq!(app.hit(stats.x + 2, stats.y + 2), Hit::Stats);
    wheel(&mut app, stats.x + 2, stats.y + 2, true);
    assert_eq!((app.scroll(), app.cursor()), (0, 3));
}

// --- the file list -----------------------------------------------------

#[test]
fn a_click_on_a_file_row_focuses_the_list_and_selects_it() {
    let f = fixture();
    let mut clock = Clock::new();
    let mut app = f.open("");
    assert_eq!(app.focus(), Focus::Diff);
    let t = tree_inner(&app);
    let a = file_row(&app, "f01.rs");
    let y = file_y(&app, a);
    assert_eq!(app.hit(t.x + 6, y), Hit::Files { row: a, x: 6 });
    click(&mut app, &mut clock, t.x + 6, y);
    assert_eq!(app.focus(), Focus::Files);
    assert_eq!(app.sidebar_selected(), a);
    let header = app.commit_rows()[app.commit_cursor().row].clone();
    assert_eq!(header.kind, RowKind::FileHeader);
    assert!(header.text.contains("src/f01.rs"), "{header:?}");
    assert_eq!(
        file_y(&app, a),
        y,
        "the list does not scroll under the pointer"
    );
    // A double-click acts as Enter: on a file, the diff gets the keys.
    let b = file_row(&app, "f02.rs");
    let y = file_y(&app, b);
    double_click(&mut app, &mut clock, t.x + 6, y);
    assert_eq!(app.focus(), Focus::Diff);
    let header = &app.commit_rows()[app.commit_cursor().row];
    assert!(header.text.contains("src/f02.rs"), "{header:?}");
}

#[test]
fn a_click_does_not_scroll_the_file_list_under_the_pointer() {
    let f = fixture();
    let mut clock = Clock::new();
    let mut app = f.open("");
    // The diff cursor on the last file: the list scrolls to show it.
    keys(&mut app, "G");
    draw(&mut app);
    let t = tree_inner(&app);
    let skip = app.files_skip(t.height as usize);
    assert!(skip > 0);
    // A click on the top row selects the row drawn there, and it stays.
    click(&mut app, &mut clock, t.x + 6, t.y);
    assert_eq!(app.focus(), Focus::Files);
    assert_eq!(app.sidebar_selected(), skip);
    assert_eq!(app.files_skip(t.height as usize), skip);
    // Keys from there scroll it as they always do.
    keys(&mut app, "k");
    assert_eq!(app.files_skip(t.height as usize), skip - 1);
}

/// The file list's selection is among the rows drawn.
fn selection_shown(app: &App) -> bool {
    let h = tree_inner(app).height as usize;
    let skip = app.files_skip(h);
    (skip..skip + h).contains(&app.sidebar_selected())
}

#[test]
fn gg_after_the_wheel_shows_the_selected_first_row() {
    let f = fixture();
    let mut app = f.open("");
    ctrl(&mut app, 'h');
    keys(&mut app, "gg");
    assert_eq!(app.sidebar_selected(), 0);
    let t = tree_inner(&app);
    wheel(&mut app, t.x + 3, t.y + 3, true);
    assert!(!selection_shown(&app), "the wheel scrolled it away");
    // gg selects the row already selected: the list still shows it.
    keys(&mut app, "gg");
    assert_eq!(app.files_skip(t.height as usize), 0);
    // So do k and C-u at the top, which leave the selection where it is.
    for key in ["k", "C-u"] {
        wheel(&mut app, t.x + 3, t.y + 3, true);
        assert!(!selection_shown(&app), "{key}: the wheel scrolled it away");
        match key {
            "C-u" => ctrl(&mut app, 'u'),
            k => keys(&mut app, k),
        }
        assert_eq!(app.sidebar_selected(), 0);
        assert_eq!(app.files_skip(t.height as usize), 0, "{key}");
    }
}

#[test]
fn g_after_the_wheel_up_shows_the_selected_last_row() {
    let f = fixture();
    let mut app = f.open("");
    ctrl(&mut app, 'h');
    keys(&mut app, "G");
    let t = tree_inner(&app);
    let h = t.height as usize;
    let last = app.file_list().unwrap().visible.rows.len() - 1;
    assert_eq!(app.sidebar_selected(), last);
    wheel(&mut app, t.x + 3, t.y + 3, false);
    assert!(!selection_shown(&app), "the wheel scrolled it away");
    keys(&mut app, "G");
    assert_eq!(app.files_skip(h), last + 1 - h);
}

#[test]
fn a_shorter_list_after_the_wheel_shows_the_selection() {
    let f = fixture();
    let mut clock = Clock::new();
    let mut app = f.open("");
    let t = tree_inner(&app);
    wheel(&mut app, t.x + 3, t.y + 3, true);
    // Select the bottom row drawn: shown, and pinned there.
    click(&mut app, &mut clock, t.x + 6, t.bottom() - 1);
    assert_eq!(app.focus(), Focus::Files);
    assert!(selection_shown(&app));
    app.event(AppEvent::Resize(COLS, ROWS - 6));
    draw(&mut app);
    assert!(selection_shown(&app), "the shorter list still shows it");
}

#[test]
fn a_click_on_a_directory_arrow_opens_and_closes_it() {
    let f = fixture();
    let mut clock = Clock::new();
    let mut app = f.open("");
    let t = tree_inner(&app);
    let src = file_row(&app, "src");
    let rows = app.file_list().unwrap().visible.rows.len();
    let y = file_y(&app, src);
    // The arrow sits after the one-column gutter (depth 0: no indent).
    click(&mut app, &mut clock, t.x + 1, y);
    assert_eq!(app.focus(), Focus::Files);
    assert_eq!(
        app.file_list().unwrap().visible.rows.len(),
        rows - SRC_FILES
    );
    click(&mut app, &mut clock, t.x + 1, y);
    assert_eq!(app.file_list().unwrap().visible.rows.len(), rows);
    // A click on its name only selects it.
    click(&mut app, &mut clock, t.x + 4, y);
    assert_eq!(app.file_list().unwrap().visible.rows.len(), rows);
    assert_eq!(app.sidebar_selected(), src);
    // A double-click on its name toggles it, as Enter does.
    double_click(&mut app, &mut clock, t.x + 4, y);
    assert_eq!(
        app.file_list().unwrap().visible.rows.len(),
        rows - SRC_FILES
    );
    assert_eq!(app.focus(), Focus::Files);
}

#[test]
fn the_wheel_scrolls_the_file_list_without_moving_the_diff() {
    let f = fixture();
    let mut app = f.open("");
    let t = tree_inner(&app);
    let h = t.height as usize;
    assert!(app.file_list().unwrap().visible.rows.len() > h + 3);
    let before = (
        app.commit_cursor(),
        app.commit_scroll(),
        app.sidebar_selected(),
    );
    let skip = app.files_skip(h);
    wheel(&mut app, t.x + 3, t.y + 3, true);
    assert_eq!(app.files_skip(h), skip + 3);
    assert_eq!(
        (
            app.commit_cursor(),
            app.commit_scroll(),
            app.sidebar_selected()
        ),
        before,
        "the diff and the selection stay"
    );
    assert_eq!(app.focus(), Focus::Diff, "the wheel does not focus");
    // A click there lands on the row drawn there.
    let mut clock = Clock::new();
    click(&mut app, &mut clock, t.x + 6, t.y + 2);
    assert_eq!(app.sidebar_selected(), skip + 5);
    assert_eq!(app.files_skip(h), skip + 3, "and the list stays");
    wheel(&mut app, t.x + 3, t.y + 3, false);
    assert_eq!(app.files_skip(h), skip);
}

// --- the commit list ---------------------------------------------------

const COMMITS: &str = "[view]\nsidebar_commits = true\n";

#[test]
fn a_click_on_a_commit_list_row_focuses_it_and_a_double_click_opens() {
    let f = fixture();
    let mut clock = Clock::new();
    let mut app = f.open(COMMITS);
    let c = commits_inner(&app);
    assert_eq!(app.hit(c.x + 3, c.y + 2), Hit::Commits(2));
    // The rule above the rows is the list, not a row.
    assert_eq!(app.hit(c.x + 3, c.y - 1), Hit::CommitsPane);
    click(&mut app, &mut clock, c.x + 3, c.y + 2);
    assert_eq!(app.focus(), Focus::Commits);
    assert_eq!(app.commit_list_cursor(), 2);
    assert_eq!(app.screen(), Screen::Commit(0), "a click opens nothing");
    double_click(&mut app, &mut clock, c.x + 3, c.y + 3);
    assert_eq!(app.screen(), Screen::Commit(3));
    assert_eq!(app.focus(), Focus::Commits, "the list keeps the keys");
}

#[test]
fn the_wheel_scrolls_the_commit_list_without_opening() {
    let f = fixture();
    let mut app = f.open(COMMITS);
    let c = commits_inner(&app);
    let diff = (app.commit_cursor(), app.commit_scroll());
    wheel(&mut app, c.x + 3, c.y + 1, true);
    assert_eq!(app.commit_list_offset(), 3);
    assert_eq!(app.commit_list_cursor(), 3, "kept on screen");
    assert_eq!(app.screen(), Screen::Commit(0));
    assert_eq!((app.commit_cursor(), app.commit_scroll()), diff);
    // A click after the scroll lands on the row drawn there.
    let mut clock = Clock::new();
    click(&mut app, &mut clock, c.x + 3, c.y + 1);
    assert_eq!(app.commit_list_cursor(), 4);
    wheel(&mut app, c.x + 3, c.y + 1, false);
    assert_eq!(app.commit_list_offset(), 0);
}

// --- the diff ----------------------------------------------------------

#[test]
fn a_click_in_the_diff_focuses_it_and_moves_the_cursor_there() {
    let f = fixture();
    let mut clock = Clock::new();
    let mut app = f.open("");
    ctrl(&mut app, 'h');
    assert_eq!(app.focus(), Focus::Files);
    keys(&mut app, "10");
    let d = diff_rect(&app);
    let row = 6;
    let y = diff_y(&app, row);
    assert!(matches!(
        app.hit(d.x + 20, y),
        Hit::Diff {
            row: 6,
            side: None,
            ..
        }
    ));
    click(&mut app, &mut clock, d.x + 20, y);
    assert_eq!(app.focus(), Focus::Diff);
    assert_eq!(app.commit_cursor().row, row);
    // The typed count is dropped by a click that acts.
    keys(&mut app, "j");
    assert_eq!(app.commit_cursor().row, row + 1);
}

#[test]
fn a_click_on_a_diff_line_puts_the_cursor_on_its_column() {
    let f = fixture();
    let mut clock = Clock::new();
    let mut app = f.open("");
    let d = diff_rect(&app);
    let row = app
        .commit_rows()
        .iter()
        .position(|r| r.text == "line 5 changed")
        .unwrap();
    keys(&mut app, &format!("{}G", row + 1));
    draw(&mut app);
    let Hit::Diff { col, .. } = app.hit(d.x + 30, diff_y(&app, row)) else {
        panic!()
    };
    // The text starts after the gutter: a click past it moves the column.
    let gutter = 30 - col as u16;
    let y = diff_y(&app, row);
    click(&mut app, &mut clock, d.x + gutter + 5, y);
    assert_eq!(app.commit_cursor().row, row);
    assert_eq!(app.commit_cursor().col, 5);
}

#[test]
fn the_wheel_scrolls_the_diff_and_keeps_the_cursor_on_screen() {
    let f = fixture();
    let mut app = f.open("");
    let d = diff_rect(&app);
    wheel(&mut app, d.x + 10, d.y + 5, true);
    assert_eq!(app.commit_scroll().0, 3);
    assert_eq!(app.commit_cursor().row, 3);
    wheel(&mut app, d.x + 10, d.y + 5, false);
    assert_eq!(app.commit_scroll().0, 0);
    assert_eq!(app.commit_cursor().row, 3);
}

#[test]
fn a_drag_in_the_diff_selects_lines_then_y_copies_them() {
    let f = fixture();
    let mut app = f.open("");
    let d = diff_rect(&app);
    let t = Instant::now();
    let (x, y) = (d.x + 20, d.y + 4);
    app.mouse(ev(MouseEventKind::Down(MouseButton::Left), x, y), t);
    assert_eq!(app.mode(), Mode::Normal, "a press alone selects nothing");
    app.mouse(ev(MouseEventKind::Drag(MouseButton::Left), x, y + 3), t);
    assert_eq!(app.mode(), Mode::Visual);
    assert_eq!(app.selection_rows(), Some((4, 7)));
    app.mouse(ev(MouseEventKind::Drag(MouseButton::Left), x, y - 2), t);
    assert_eq!(app.selection_rows(), Some((2, 4)), "back above the press");
    app.mouse(ev(MouseEventKind::Up(MouseButton::Left), x, y - 2), t);
    assert_eq!(app.mode(), Mode::Visual, "the selection stays for c / y");
    app.take_output();
    keys(&mut app, "y");
    let out = String::from_utf8(app.take_output()).unwrap();
    assert!(out.starts_with("\x1b]52;c;"), "{out:?}");
    assert_eq!(app.mode(), Mode::Normal);
}

#[test]
fn a_drag_past_the_diff_edges_scrolls_it() {
    let f = fixture();
    let mut app = f.open("");
    let d = diff_rect(&app);
    let t = Instant::now();
    let x = d.x + 20;
    let drag = |app: &mut App, y| app.mouse(ev(MouseEventKind::Drag(MouseButton::Left), x, y), t);
    app.mouse(ev(MouseEventKind::Down(MouseButton::Left), x, d.y + 10), t);
    // Onto the status line, below the diff: one row a move.
    drag(&mut app, d.bottom());
    assert_eq!(app.commit_scroll().0, 1);
    let last = app.commit_scroll().0 + d.height as usize - 1;
    assert_eq!(app.selection_rows(), Some((10, last)));
    drag(&mut app, d.bottom());
    assert_eq!(app.commit_scroll().0, 2);
    assert_eq!(app.selection_rows(), Some((10, last + 1)));
    // The top row is the top edge.
    drag(&mut app, d.y);
    assert_eq!(app.commit_scroll().0, 1);
    assert_eq!(app.selection_rows(), Some((1, 10)));
}

/// The text an OSC 52 sequence in `out` puts on the clipboard.
fn clipboard(out: &[u8]) -> String {
    let s = String::from_utf8(out.to_vec()).unwrap();
    let b = s
        .strip_prefix("\x1b]52;c;")
        .and_then(|s| s.strip_suffix('\x07'))
        .unwrap_or_else(|| panic!("not OSC 52: {s:?}"));
    let val = |c: u8| match c {
        b'A'..=b'Z' => c - b'A',
        b'a'..=b'z' => c - b'a' + 26,
        b'0'..=b'9' => c - b'0' + 52,
        b'+' => 62,
        _ => 63,
    };
    let mut out = Vec::new();
    for chunk in b.as_bytes().chunks(4) {
        let v: Vec<u8> = chunk
            .iter()
            .filter(|&&c| c != b'=')
            .map(|&c| val(c))
            .collect();
        let n = v.iter().fold(0u32, |n, &d| n << 6 | d as u32) << (6 * (4 - v.len()));
        out.extend(&[(n >> 16) as u8, (n >> 8) as u8, n as u8][..v.len() - 1]);
    }
    String::from_utf8(out).unwrap()
}

/// From side by side at 220 columns: press on `from`'s row, drag (once
/// to the row below when `early`, before the resize), resize to
/// `COLS` (unified: a change block's rows no longer pair), drag to
/// `to`'s row and copy. Returns the copied lines.
fn drag_across_a_relayout(f: &Fixture, early: bool, from: &str, to: &str) -> Vec<String> {
    let mut app = f.open_sized("[view]\nlayout = \"split\"\n", 220);
    assert!(app.commit_split());
    let at = |app: &App, text: &str| {
        app.commit_rows()
            .iter()
            .position(|r| r.text == text)
            .unwrap_or_else(|| panic!("no row {text}"))
    };
    let t = Instant::now();
    let (left, _) = app.split_panes(diff_rect(&app));
    let x = left.x + 12;
    let row = at(&app, from);
    keys(&mut app, &format!("{}Gzz", row + 1));
    draw(&mut app);
    let y = diff_y(&app, row);
    app.mouse(ev(MouseEventKind::Down(MouseButton::Left), x, y), t);
    if early {
        app.mouse(ev(MouseEventKind::Drag(MouseButton::Left), x, y + 1), t);
        assert_eq!(app.mode(), Mode::Visual);
    }
    app.event(AppEvent::Resize(COLS, ROWS));
    draw(&mut app);
    assert!(!app.commit_split(), "unified below split_min_width");
    let row = at(&app, to);
    let y = diff_y(&app, row);
    let d = diff_rect(&app);
    assert!(
        y > d.y && y < d.bottom(),
        "{to} inside the diff, off its edges"
    );
    app.mouse(ev(MouseEventKind::Drag(MouseButton::Left), 30, y), t);
    app.mouse(ev(MouseEventKind::Up(MouseButton::Left), 30, y), t);
    app.take_output();
    keys(&mut app, "y");
    clipboard(&app.take_output())
        .lines()
        .map(String::from)
        .collect()
}

#[test]
fn a_drag_keeps_its_pressed_line_across_a_relayout() {
    let f = fixture();
    // Old-side lines; unified, the change blocks above them add rows.
    for early in [true, false] {
        let lines = drag_across_a_relayout(&f, early, "line 61", "line 63");
        assert_eq!(
            lines,
            [" line 61", " line 62", " line 63"],
            "dragged before the resize: {early}"
        );
    }
}

#[test]
fn a_click_in_visual_mode_leaves_it() {
    let f = fixture();
    let mut clock = Clock::new();
    let mut app = f.open("");
    keys(&mut app, "Vjj");
    assert_eq!(app.mode(), Mode::Visual);
    let d = diff_rect(&app);
    click(&mut app, &mut clock, d.x + 10, d.y + 8);
    assert_eq!(app.mode(), Mode::Normal);
    assert_eq!(app.commit_cursor().row, 8);
}

// --- layouts -----------------------------------------------------------

#[test]
fn hits_follow_a_file_list_on_the_left() {
    let f = fixture();
    let app = f.open("");
    let side = app.commit_layout(main_area(&app)).sidebar.unwrap();
    let d = diff_rect(&app);
    assert_eq!(side.x, 0);
    assert!(matches!(app.hit(2, 3), Hit::Files { row: 3, x: 2 }));
    // Its border, toward the diff, is the list but no row.
    assert_eq!(app.hit(side.right() - 1, 3), Hit::FilesPane);
    assert!(matches!(app.hit(d.x, 3), Hit::Diff { row: 3, .. }));
    assert_eq!(app.hit(5, ROWS - 1), Hit::Status);
}

#[test]
fn hits_follow_a_file_list_on_the_right() {
    let f = fixture();
    let mut clock = Clock::new();
    let mut app = f.open("[view]\nsidebar_side = \"right\"\n");
    let side = app.commit_layout(main_area(&app)).sidebar.unwrap();
    let d = diff_rect(&app);
    assert_eq!((d.x, side.right()), (0, COLS));
    assert_eq!(app.hit(side.x, 3), Hit::FilesPane, "the border on its left");
    assert!(matches!(
        app.hit(side.x + 3, 3),
        Hit::Files { row: 3, x: 2 }
    ));
    assert!(matches!(app.hit(2, 3), Hit::Diff { row: 3, .. }));
    let a = file_row(&app, "f01.rs");
    let y = file_y(&app, a);
    click(&mut app, &mut clock, side.x + 6, y);
    assert_eq!((app.focus(), app.sidebar_selected()), (Focus::Files, a));
}

#[test]
fn side_by_side_a_click_picks_the_half() {
    let f = fixture();
    let mut clock = Clock::new();
    let mut app = f.open_sized("[view]\nlayout = \"split\"\n", 220);
    assert!(app.commit_split());
    let d = diff_rect(&app);
    let (left, right) = app.split_panes(d);
    let row = app
        .commit_rows()
        .iter()
        .position(|r| r.kind == RowKind::Line(vcs::LineKind::Added))
        .unwrap();
    let y = diff_y(&app, row);
    assert!(matches!(
        app.hit(left.x + 12, y),
        Hit::Diff {
            side: Some(Side::Old),
            ..
        }
    ));
    assert!(matches!(
        app.hit(right.x + 12, y),
        Hit::Diff {
            side: Some(Side::New),
            ..
        }
    ));
    click(&mut app, &mut clock, left.x + 12, y);
    assert_eq!(app.split_side(), Some(Side::Old));
    assert_eq!(app.commit_cursor().row, row);
    click(&mut app, &mut clock, right.x + 12, y);
    assert_eq!(app.split_side(), Some(Side::New));
    // A row drawn across both halves keeps the cursor's side.
    let header = app
        .commit_rows()
        .iter()
        .position(|r| r.kind == RowKind::FileHeader)
        .unwrap();
    assert!(matches!(
        app.hit(left.x + 3, diff_y(&app, header)),
        Hit::Diff {
            side: Some(Side::New),
            ..
        }
    ));
}

#[test]
fn an_auto_peek_over_the_diff_takes_its_clicks_and_a_click_beside_closes_it() {
    let f = fixture();
    let mut clock = Clock::new();
    // 90 columns: a peek beside would leave the diff under 80.
    let mut app = f.open_sized("[view]\nsidebar = \"auto\"\n", 90);
    assert!(!app.sidebar_visible());
    assert!(matches!(app.hit(2, 3), Hit::Diff { .. }), "no list yet");
    ctrl(&mut app, 'h');
    assert!(app.sidebar_visible() && app.sidebar_overlay());
    let side = app.commit_layout(main_area(&app)).sidebar.unwrap();
    assert!(matches!(app.hit(2, 3), Hit::Files { .. }), "over the diff");
    click(&mut app, &mut clock, side.right() + 5, 3);
    assert_eq!(app.focus(), Focus::Diff);
    assert!(!app.sidebar_visible(), "the peek closed");
    assert_eq!(app.commit_cursor().row, 3);
}

// --- overlays ----------------------------------------------------------

#[test]
fn help_scrolls_with_the_wheel_and_a_click_outside_closes_it() {
    let f = fixture();
    let mut clock = Clock::new();
    let mut app = f.open("");
    keys(&mut app, "g?");
    assert_eq!(app.mode(), Mode::Help);
    let r = debrief::app::help_rect(main_area(&app));
    assert_eq!(app.hit(r.x + 3, r.y + 3), Hit::Help);
    wheel(&mut app, r.x + 3, r.y + 3, true);
    assert_eq!(app.help_view().unwrap().scroll, 3);
    wheel(&mut app, r.x + 3, r.y + 3, false);
    assert_eq!(app.help_view().unwrap().scroll, 0);
    let row = app.commit_cursor().row;
    click(&mut app, &mut clock, r.x + 3, r.y + 3);
    assert_eq!(app.mode(), Mode::Help, "a click inside keeps it");
    click(&mut app, &mut clock, 0, 0);
    assert_eq!(app.mode(), Mode::Normal);
    assert_eq!(
        app.commit_cursor().row,
        row,
        "the closing click does nothing else"
    );
}

/// `bodies` as working-copy comments on x.rs lines 1, 2, ..., and the
/// comment picker open.
fn picker_of(f: &Fixture, bodies: &[&str]) -> App {
    let mut review = f.review();
    for (i, body) in bodies.iter().enumerate() {
        let line = i as u32 + 1;
        review
            .add(Comment::on_file("x.rs", line..=line, "x", *body))
            .unwrap();
    }
    let mut app = f.open("");
    keys(&mut app, " rl");
    assert_eq!(app.mode(), Mode::Picker);
    assert_eq!(app.picker().unwrap().items.len(), bodies.len());
    app
}

fn picker(f: &Fixture) -> App {
    picker_of(f, &["one", "two", "three"])
}

/// The screen row of picker item `i` (its list starts under the filter
/// line, inside the border).
fn picker_y(app: &App, i: usize) -> u16 {
    debrief::app::help_rect(main_area(app)).y + 2 + i as u16
}

#[test]
fn a_click_on_a_picker_item_selects_it_and_a_double_click_picks_it() {
    let f = fixture();
    let mut clock = Clock::new();
    let mut app = picker(&f);
    let r = debrief::app::help_rect(main_area(&app));
    assert_eq!(app.hit(r.x + 4, picker_y(&app, 1)), Hit::PickerItem(1));
    assert_eq!(app.hit(r.x + 4, r.y + 1), Hit::Picker, "the filter line");
    assert_eq!(
        app.hit(r.x + 4, picker_y(&app, 3)),
        Hit::Picker,
        "past the items"
    );
    let y = picker_y(&app, 1);
    click(&mut app, &mut clock, r.x + 4, y);
    assert_eq!(app.picker().unwrap().selected, 1);
    // With filter text typed the picker still takes the mouse.
    keys(&mut app, "t");
    assert_eq!(app.picker().unwrap().items.len(), 2, "two, three");
    let y = picker_y(&app, 1);
    double_click(&mut app, &mut clock, r.x + 4, y);
    assert_eq!(app.mode(), Mode::Normal);
    assert_eq!(app.status(), "Working-copy comment: x.rs:3");
}

#[test]
fn the_wheel_moves_the_picker_selection_one_a_notch_without_wrapping() {
    let f = fixture();
    let mut app = picker(&f);
    let r = debrief::app::help_rect(main_area(&app));
    let (x, y) = (r.x + 4, picker_y(&app, 0));
    wheel(&mut app, x, y, false);
    assert_eq!(app.picker().unwrap().selected, 0, "no wrap at the top");
    wheel(&mut app, x, y, true);
    assert_eq!(app.picker().unwrap().selected, 1);
    wheel(&mut app, x, y, true);
    wheel(&mut app, x, y, true);
    assert_eq!(app.picker().unwrap().selected, 2, "no wrap at the bottom");
    // Outside the box the wheel does nothing.
    wheel(&mut app, 0, 0, false);
    assert_eq!(app.picker().unwrap().selected, 2);
}

#[test]
fn a_click_in_a_scrolled_picker_lands_on_the_item_drawn_there() {
    let f = fixture();
    let mut clock = Clock::new();
    let bodies: Vec<String> = (0..40).map(|i| format!("c{i}")).collect();
    let bodies: Vec<&str> = bodies.iter().map(String::as_str).collect();
    let mut app = picker_of(&f, &bodies);
    let r = debrief::app::help_rect(main_area(&app));
    // Inside the border: the filter line, the list, the keys line.
    let h = (r.height - 4) as usize;
    let (x, y) = (r.x + 4, picker_y(&app, 0));
    for _ in 0..h + 2 {
        wheel(&mut app, x, y, true);
    }
    assert_eq!(app.picker().unwrap().selected, h + 2);
    // The list scrolled 3 to show it: its top row is item 3.
    assert_eq!(app.hit(x, y), Hit::PickerItem(3));
    click(&mut app, &mut clock, x, y);
    assert_eq!(app.picker().unwrap().selected, 3);
}

#[test]
fn a_click_outside_the_picker_closes_it() {
    let f = fixture();
    let mut clock = Clock::new();
    let mut app = picker(&f);
    let d = diff_rect(&app);
    click(&mut app, &mut clock, d.x + 1, d.bottom() - 1);
    assert_eq!(app.mode(), Mode::Normal);
    assert!(app.picker().is_none());
    assert_eq!(
        app.commit_cursor().row,
        0,
        "the closing click does nothing else"
    );
}

#[test]
fn a_click_outside_the_k_popup_closes_it() {
    let f = fixture();
    let mut clock = Clock::new();
    let mut app = f.open("");
    let row = app
        .commit_rows()
        .iter()
        .position(|r| r.kind == RowKind::Line(vcs::LineKind::Added))
        .unwrap();
    keys(&mut app, &format!("{}G", row + 1));
    keys(&mut app, "cc");
    keys(&mut app, "note");
    press(&mut app, KeyCode::Enter);
    keys(&mut app, "K");
    assert_eq!(app.mode(), Mode::Popup, "{}", app.status());
    let r = app.popup_view().unwrap().rect;
    assert_eq!(app.hit(r.x + 2, r.y + 1), Hit::Popup);
    click(&mut app, &mut clock, r.x + 2, r.y + 1);
    assert_eq!(app.mode(), Mode::Popup, "a click inside keeps it");
    let d = diff_rect(&app);
    click(&mut app, &mut clock, d.x + 1, d.bottom() - 1);
    assert_eq!(app.mode(), Mode::Normal);
    assert_eq!(
        app.commit_cursor().row,
        row,
        "the closing click does nothing else"
    );
}

// --- typing ------------------------------------------------------------

/// Click, double-click, wheel and drag over the diff and the file list.
fn poke(app: &mut App, clock: &mut Clock) {
    let d = diff_rect(app);
    click(app, clock, d.x + 10, d.y + 8);
    double_click(app, clock, d.x + 10, d.y + 9);
    wheel(app, d.x + 10, d.y + 8, true);
    app.mouse(
        ev(MouseEventKind::Drag(MouseButton::Left), d.x + 10, d.y + 12),
        Instant::now(),
    );
    click(app, clock, 3, 3);
    click(app, clock, 0, 0);
}

#[test]
fn mouse_events_are_ignored_while_typing() {
    let f = fixture();
    let mut clock = Clock::new();
    let mut app = f.open("");
    let row = app
        .commit_rows()
        .iter()
        .position(|r| r.kind == RowKind::Line(vcs::LineKind::Added))
        .unwrap();
    let opened = |app: &mut App, k: &str, mode: Mode| {
        keys(app, k);
        assert_eq!(app.mode(), mode, "{k}: {}", app.status());
    };
    // The comment box.
    keys(&mut app, &format!("{}G", row + 1));
    let at = (app.commit_cursor(), app.commit_scroll(), app.focus());
    opened(&mut app, "cc", Mode::Comment);
    poke(&mut app, &mut clock);
    assert_eq!(app.mode(), Mode::Comment);
    assert_eq!((app.commit_cursor(), app.commit_scroll(), app.focus()), at);
    press(&mut app, KeyCode::Esc);
    // The / search prompt.
    opened(&mut app, "/", Mode::Search);
    poke(&mut app, &mut clock);
    assert_eq!(app.mode(), Mode::Search);
    assert_eq!((app.commit_cursor(), app.commit_scroll(), app.focus()), at);
    press(&mut app, KeyCode::Esc);
    // The help filter: a click outside leaves it open.
    opened(&mut app, "g?/", Mode::Help);
    poke(&mut app, &mut clock);
    assert_eq!(app.mode(), Mode::Help);
    assert!(app.help_view().unwrap().filtering);
    assert_eq!(app.help_view().unwrap().scroll, 0);
}

#[test]
fn mouse_events_are_ignored_in_the_log_filter() {
    let f = fixture();
    let mut clock = Clock::new();
    let mut app = f.app("");
    keys(&mut app, "f");
    assert_eq!(app.mode(), Mode::Filter);
    let log = app.log_layout(main_area(&app)).log;
    double_click(&mut app, &mut clock, log.x + 5, log.y + 3);
    wheel(&mut app, log.x + 5, log.y + 3, true);
    assert_eq!(app.mode(), Mode::Filter);
    assert_eq!(
        (app.cursor(), app.scroll(), app.screen()),
        (0, 0, Screen::Log)
    );
}

#[test]
fn a_mouse_event_arrives_as_an_app_event() {
    let f = fixture();
    let mut app = f.app("");
    let log = app.log_layout(main_area(&app)).log;
    let at = |kind| ev(kind, log.x + 5, log.y + 6);
    app.event(AppEvent::Mouse(at(MouseEventKind::Down(MouseButton::Left))));
    app.event(AppEvent::Mouse(at(MouseEventKind::Up(MouseButton::Left))));
    assert_eq!(app.cursor(), 6);
}
