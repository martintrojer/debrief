//! The commit list under the file tree (`[view] sidebar_commits`,
//! `<leader>E`): the 60/40 split and its hiding under 12 rows, the rows
//! and their highlights, the list's keys and focus moves, Enter opening
//! through the log's path while the list keeps the keys (async loads,
//! saved sessions, a commit without files), following a filter and a
//! reload, and sessions that never store the setting.

mod common;

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use common::{commit, ctrl, git, keys, press};
use crossterm::event::KeyCode;
use debrief::app::config::{Config, SidebarShow};
use debrief::app::{App, AppEvent, Exit, Focus, HelpLine, Screen, StartOptions, short_id};
use debrief::seen::Status;
use debrief::session::{Store, ViewFocus};
use debrief::ui::palette;
use debrief::vcs;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier};

const COLS: u16 = 120;
const ROWS: u16 = 30;
const ON: &str = "[view]\nsidebar_commits = true\n";
const AUTO: &str = "[view]\nsidebar_commits = true\nsidebar = \"auto\"\n";

struct Fixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    cache: PathBuf,
}

/// `base`, then `commit 1` .. `commit {n}`, each changing a.txt (row 0
/// is `commit {n}`).
fn fixture_n(n: usize) -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir(&root).unwrap();
    git(&root, &["init", "-q"]);
    std::fs::write(root.join("a.txt"), "base\n").unwrap();
    std::fs::write(root.join("b.txt"), "b\n").unwrap();
    commit(&root, "base");
    for i in 1..=n {
        std::fs::write(root.join("a.txt"), format!("{i}\n")).unwrap();
        commit(&root, &format!("commit {i}"));
    }
    Fixture {
        root: root.canonicalize().unwrap(),
        cache: tmp.path().join("cache"),
        _tmp: tmp,
    }
}

fn fixture() -> Fixture {
    fixture_n(5)
}

/// The tmux commands the app ran.
type Ran = Rc<RefCell<Vec<String>>>;

fn ran(r: &Ran) -> Vec<String> {
    std::mem::take(&mut r.borrow_mut())
}

impl Fixture {
    fn app_sized(&self, toml: &str, cols: u16, rows: u16) -> (App, Ran) {
        let mut opts = StartOptions::new(self.cache.clone());
        opts.config = Config::parse(toml).unwrap();
        let ran: Ran = Rc::default();
        let r = ran.clone();
        let mut app = App::new(
            vcs::open(&self.root, vcs::OpenOptions::default()).unwrap(),
            opts,
        )
        .unwrap()
        .with_env(|k| (k == "TMUX").then(|| "/tmp/tmux-1/default,1,0".into()))
        .with_pane_runner(move |c| {
            r.borrow_mut().push(c.argv.join(" "));
            Ok(Exit::Code(0))
        });
        app.event(AppEvent::Resize(cols, rows));
        (app, ran)
    }

    fn app(&self, toml: &str) -> App {
        self.app_sized(toml, COLS, ROWS).0
    }

    /// Log row `i` open.
    fn open(&self, toml: &str, i: usize) -> App {
        let mut app = self.app(toml);
        open_row(&mut app, i);
        app
    }

    fn store(&self) -> Store {
        Store::new(&self.cache.join("state"), &self.root)
    }
}

/// From the log, open row `i`.
fn open_row(app: &mut App, i: usize) {
    keys(app, &format!("{}G", i + 1));
    press(app, KeyCode::Enter);
    assert_eq!(app.screen(), Screen::Commit(i));
}

/// The diff, then the tree (`C-h`), then the list (`C-j`).
fn to_list(app: &mut App) {
    ctrl(app, 'h');
    ctrl(app, 'j');
    assert_eq!(app.focus(), Focus::Commits, "{}", app.status());
}

fn area(rows: u16) -> Rect {
    Rect::new(0, 0, COLS, rows - 1)
}

fn draw(app: &mut App, cols: u16, rows: u16) -> Buffer {
    let mut t = Terminal::new(TestBackend::new(cols, rows)).unwrap();
    t.draw(|f| app.render(f)).unwrap();
    t.backend().buffer().clone()
}

fn line(buf: &Buffer, r: Rect, y: u16) -> String {
    (r.x..r.x + r.width).map(|x| buf[(x, y)].symbol()).collect()
}

/// The commit list's rows on screen, below its rule (border excluded).
fn list_lines(app: &mut App) -> Vec<String> {
    let l = app.commit_layout(area(ROWS));
    let c = l.commits.expect("the commit list is shown");
    let buf = draw(app, COLS, ROWS);
    let inner = Rect::new(c.x, c.y + 1, c.width - 1, c.height - 1);
    (inner.y..inner.y + inner.height)
        .map(|y| line(&buf, inner, y))
        .collect()
}

/// A list row after its state marker and space.
fn body(line: &str) -> String {
    line.chars().skip(2).collect()
}

/// The screen row of log row `i` in the list (scrolled as drawn).
fn list_y(app: &App, i: usize) -> u16 {
    let c = app.commit_layout(area(ROWS)).commits.unwrap();
    c.y + 1 + (i - app.commit_list_offset()) as u16
}

/// Feed the loader's events until the open commit has loaded.
fn loaded(app: &mut App, rx: &mpsc::Receiver<AppEvent>) {
    while app.commit_loading() {
        app.event(rx.recv_timeout(Duration::from_secs(20)).unwrap());
    }
}

// --- layout ------------------------------------------------------------

#[test]
fn the_file_list_splits_into_the_tree_a_rule_and_the_commits() {
    let f = fixture();
    let app = f.open(ON, 0);
    let l = app.commit_layout(area(ROWS));
    let tree = l.sidebar.unwrap();
    let list = l.commits.unwrap();
    let h = ROWS - 1;
    assert_eq!(tree.height, (h - 1) * 3 / 5, "60% for the tree");
    assert_eq!((list.x, list.width), (tree.x, tree.width));
    assert_eq!(
        list.y,
        tree.y + tree.height,
        "the rule starts the list rect"
    );
    assert_eq!(tree.height + list.height, h);
    assert!(app.commit_list_visible());
    // Off (the default): the tree takes it all.
    let app = f.open("", 0);
    let l = app.commit_layout(area(ROWS));
    assert_eq!(l.commits, None);
    assert_eq!(l.sidebar.unwrap().height, h);
    assert!(!app.commit_list_visible());
}

#[test]
fn under_12_rows_the_list_hides_and_the_tree_takes_it_all() {
    let f = fixture();
    let (mut app, _) = f.app_sized(ON, COLS, 13);
    open_row(&mut app, 0);
    assert!(app.commit_layout(area(13)).commits.is_some(), "12 rows");
    app.event(AppEvent::Resize(COLS, 12));
    let l = app.commit_layout(area(12));
    assert_eq!(l.commits, None, "11 rows");
    assert_eq!(l.sidebar.unwrap().height, 11);
    assert!(!app.commit_list_visible());
}

#[test]
fn a_right_list_sits_under_a_right_tree() {
    let f = fixture();
    let app = f.open(
        "[view]\nsidebar_commits = true\nsidebar_side = \"right\"\n",
        0,
    );
    let l = app.commit_layout(area(ROWS));
    let (tree, list) = (l.sidebar.unwrap(), l.commits.unwrap());
    assert_eq!(tree.x + tree.width, COLS);
    assert_eq!((list.x, list.width), (tree.x, tree.width));
    assert_eq!(l.diff.x, 0);
}

// --- rows --------------------------------------------------------------

#[test]
fn rows_show_the_markers_the_short_id_and_the_summary_clipped() {
    let f = fixture();
    // The first run marks everything seen; two later commits are new.
    drop(f.app(""));
    std::fs::write(f.root.join("a.txt"), "x\n").unwrap();
    commit(&f.root, "new one");
    let long = "a summary far too long for the list ".repeat(4);
    std::fs::write(f.root.join("a.txt"), "y\n").unwrap();
    commit(&f.root, &long);
    let mut app = f.open(&format!("{ON}sidebar_width = 30\n"), 2);
    let rows = app.rows().to_vec();
    let lines = list_lines(&mut app);
    assert_eq!(lines.len(), 12);
    assert_eq!(lines[0].chars().count(), 30);
    for (i, r) in rows.iter().enumerate().take(lines.len()) {
        let id = short_id(&r.info);
        assert!(lines[i].contains(id), "{i}: {:?}", lines[i]);
    }
    assert_eq!(rows[0].status, Status::New);
    assert!(lines[0].starts_with("● "), "new: {:?}", lines[0]);
    assert!(lines[1].starts_with("● "), "new: {:?}", lines[1]);
    assert!(lines[2].starts_with("  "), "seen: {:?}", lines[2]);
    let row = |i: usize| format!("{} {}", short_id(&rows[i].info), rows[i].info.summary);
    assert_eq!(body(&lines[1]).trim_end(), row(1), "{:?}", lines[1]);
    // Clipped at the list's edge: the border column stays.
    let shown = body(&lines[0]);
    assert!(row(0).starts_with(shown.trim_end()), "{:?}", lines[0]);
    assert!(shown.chars().count() > 10, "{:?}", lines[0]);
    assert!(!lines[0].contains("list"), "{:?}", lines[0]);
    let buf = draw(&mut app, COLS, ROWS);
    let c = app.commit_layout(area(ROWS)).commits.unwrap();
    assert_eq!(buf[(c.x + c.width - 1, c.y + 1)].symbol(), "│");
    // The rule.
    assert!(
        line(&buf, c, c.y).starts_with("──────"),
        "{:?}",
        line(&buf, c, c.y)
    );
}

#[test]
fn a_saved_session_shows_its_marker() {
    let f = fixture();
    let mut app = f.open(ON, 1);
    keys(&mut app, " ss");
    assert!(
        app.status().starts_with("Session saved"),
        "{}",
        app.status()
    );
    let lines = list_lines(&mut app);
    let marker = debrief::app::session::SESSION_MARKER;
    assert!(lines[1].contains(marker), "{:?}", lines[1]);
    assert!(!lines[0].contains(marker), "{:?}", lines[0]);
}

#[test]
fn the_git_working_copy_row_is_listed() {
    let f = fixture();
    std::fs::write(f.root.join("a.txt"), "dirty\n").unwrap();
    let mut app = f.open(ON, 1);
    assert!(app.rows()[0].working);
    let lines = list_lines(&mut app);
    assert!(lines[0].starts_with("✎ Working copy"), "{:?}", lines[0]);
    // It opens from the list like the log's row.
    to_list(&mut app);
    keys(&mut app, "gg");
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.screen(), Screen::Commit(0));
    assert!(app.viewing_working_copy());
    assert_eq!(app.focus(), Focus::Commits);
}

// --- highlights --------------------------------------------------------

#[test]
fn the_open_commit_row_has_the_surface_background() {
    let f = fixture();
    let mut app = f.open(ON, 2);
    assert_eq!(app.commit_list_open_row(), Some(2));
    let buf = draw(&mut app, COLS, ROWS);
    let c = app.commit_layout(area(ROWS)).commits.unwrap();
    for i in 0..5 {
        let cell = &buf[(c.x + 3, list_y(&app, i))];
        let want = if i == 2 {
            palette::SURFACE
        } else {
            Color::Reset
        };
        assert_eq!(cell.bg, want, "row {i}");
        assert!(
            !cell.modifier.contains(Modifier::BOLD),
            "no cursor unfocused"
        );
    }
}

#[test]
fn the_focused_cursor_is_a_separate_highlight() {
    let f = fixture();
    let mut app = f.open(ON, 2);
    to_list(&mut app);
    assert_eq!(
        app.commit_list_cursor(),
        2,
        "the cursor starts on the open row"
    );
    keys(&mut app, "j");
    assert_eq!(app.commit_list_cursor(), 3);
    let buf = draw(&mut app, COLS, ROWS);
    let c = app.commit_layout(area(ROWS)).commits.unwrap();
    let open = &buf[(c.x + 3, list_y(&app, 2))];
    let cur = &buf[(c.x + 3, list_y(&app, 3))];
    let other = &buf[(c.x + 3, list_y(&app, 4))];
    assert_eq!(open.bg, palette::SURFACE);
    assert!(!open.modifier.contains(Modifier::BOLD));
    assert!(
        cur.modifier.contains(Modifier::BOLD),
        "the tree's selected style"
    );
    assert_eq!(cur.bg, palette::SURFACE);
    assert_ne!(open.style(), cur.style(), "distinct");
    assert_eq!(other.bg, Color::Reset);
}

// --- keys --------------------------------------------------------------

#[test]
fn moving_the_cursor_neither_opens_nor_marks_seen() {
    let f = fixture();
    drop(f.app(""));
    for name in ["n1", "n2"] {
        std::fs::write(f.root.join("a.txt"), format!("{name}\n")).unwrap();
        commit(&f.root, name);
    }
    let mut app = f.open(ON, 3);
    to_list(&mut app);
    keys(&mut app, "gg");
    assert_eq!(app.commit_list_cursor(), 0);
    keys(&mut app, "j");
    assert_eq!(app.screen(), Screen::Commit(3), "still the open commit");
    assert_eq!(app.rows()[0].status, Status::New);
    assert_eq!(app.rows()[1].status, Status::New);
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.screen(), Screen::Commit(1));
    assert_eq!(app.rows()[1].status, Status::Seen, "Enter marks it seen");
    assert_eq!(app.rows()[0].status, Status::New);
    assert_eq!(app.focus(), Focus::Commits, "the list keeps the keys");
    assert_eq!(app.commit_list_open_row(), Some(1));
}

#[test]
fn list_motions_counts_and_esc() {
    let f = fixture_n(30);
    let mut app = f.open(ON, 0);
    to_list(&mut app);
    keys(&mut app, "G");
    assert_eq!(app.commit_list_cursor(), app.rows().len() - 1);
    keys(&mut app, "gg");
    assert_eq!(app.commit_list_cursor(), 0);
    keys(&mut app, "3j");
    assert_eq!(app.commit_list_cursor(), 3);
    keys(&mut app, "2k");
    assert_eq!(app.commit_list_cursor(), 1);
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Up);
    assert_eq!(app.commit_list_cursor(), 1);
    // C-d / C-u: half the list's 12 rows.
    ctrl(&mut app, 'd');
    assert_eq!(app.commit_list_cursor(), 7);
    ctrl(&mut app, 'u');
    assert_eq!(app.commit_list_cursor(), 1);
    assert_eq!(app.screen(), Screen::Commit(0));
    press(&mut app, KeyCode::Esc);
    assert_eq!(app.focus(), Focus::Diff);
    assert_eq!(app.screen(), Screen::Commit(0));
}

#[test]
fn the_cursor_stays_visible_and_recentres_on_the_open_commit() {
    let f = fixture_n(30);
    let mut app = f.open(ON, 0);
    to_list(&mut app);
    keys(&mut app, "G");
    let last = app.rows().len() - 1;
    let top = app.commit_list_offset();
    assert!(top <= last && last < top + 12, "{top} {last}");
    // Enter re-centres on the opened commit.
    keys(&mut app, "20gg");
    assert_eq!(app.commit_list_cursor(), 19);
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.screen(), Screen::Commit(19));
    assert_eq!(app.commit_list_offset(), 19 - 5, "centred in 12 rows");
    // ]] from the diff moves the cursor with the open commit, centred.
    press(&mut app, KeyCode::Esc);
    keys(&mut app, "[[[[");
    assert_eq!(app.screen(), Screen::Commit(17));
    assert_eq!(app.commit_list_cursor(), 17);
    assert_eq!(app.commit_list_offset(), 17 - 5);
}

#[test]
fn select_and_scroll_move_the_cursor_and_the_view() {
    let f = fixture_n(30);
    let mut app = f.open(ON, 0);
    app.commit_list_select(4);
    assert_eq!(app.focus(), Focus::Commits);
    assert_eq!(app.commit_list_cursor(), 4);
    assert_eq!(app.screen(), Screen::Commit(0), "select does not open");
    app.commit_list_scroll(10);
    assert_eq!(app.commit_list_offset(), 10);
    assert_eq!(app.commit_list_cursor(), 10, "pulled into view");
    app.commit_list_scroll(-3);
    assert_eq!(app.commit_list_offset(), 7);
    assert_eq!(app.commit_list_cursor(), 10);
    app.commit_list_scroll(1000);
    assert_eq!(app.commit_list_offset(), app.rows().len() - 12);
    app.commit_list_open();
    assert_eq!(app.screen(), Screen::Commit(app.commit_list_cursor()));
}

// --- focus moves ---------------------------------------------------------

#[test]
fn c_j_and_c_k_move_between_the_tree_and_the_list() {
    let f = fixture();
    for side in ["left", "right"] {
        let toml = format!("{ON}sidebar_side = \"{side}\"\n");
        let (mut app, r) = f.app_sized(&toml, COLS, ROWS);
        open_row(&mut app, 0);
        let into = if side == "left" { 'h' } else { 'l' };
        ctrl(&mut app, into);
        assert_eq!(app.focus(), Focus::Files, "{side}");
        ctrl(&mut app, 'j');
        assert_eq!(app.focus(), Focus::Commits, "{side}");
        assert!(ran(&r).is_empty(), "{side}: moves inside debrief");
        ctrl(&mut app, 'j');
        assert_eq!(
            ran(&r),
            ["tmux select-pane -D"],
            "{side}: the list is bottom"
        );
        ctrl(&mut app, 'k');
        assert_eq!(app.focus(), Focus::Files);
        ctrl(&mut app, 'k');
        assert_eq!(ran(&r), ["tmux select-pane -U"], "{side}: the tree is top");
        // Back to the list, then out to the diff sideways.
        ctrl(&mut app, 'j');
        let out = if side == "left" { 'l' } else { 'h' };
        ctrl(&mut app, out);
        assert_eq!(app.focus(), Focus::Diff, "{side}");
        assert!(ran(&r).is_empty());
        // Toward the screen edge from the list: tmux, the list keeps focus.
        ctrl(&mut app, into);
        ctrl(&mut app, 'j');
        ctrl(&mut app, into);
        let flag = if side == "left" { "-L" } else { "-R" };
        assert_eq!(ran(&r), [format!("tmux select-pane {flag}")]);
        assert_eq!(app.focus(), Focus::Commits);
        assert_eq!(app.status(), "", "no \"no file list\"");
    }
}

#[test]
fn with_the_list_hidden_c_j_and_c_k_hand_off_to_tmux() {
    let f = fixture();
    let (mut app, r) = f.app_sized("", COLS, ROWS);
    open_row(&mut app, 0);
    ctrl(&mut app, 'h');
    ctrl(&mut app, 'j');
    assert_eq!(ran(&r), ["tmux select-pane -D"]);
    assert_eq!(app.focus(), Focus::Files);
    // Too short a terminal hides it too.
    let (mut app, r) = f.app_sized(ON, COLS, 12);
    open_row(&mut app, 0);
    ctrl(&mut app, 'h');
    ctrl(&mut app, 'j');
    assert_eq!(ran(&r), ["tmux select-pane -D"]);
    // pane_nav = false: unbound.
    let (mut app, r) = f.app_sized(&format!("{ON}[keys]\npane_nav = false\n"), COLS, ROWS);
    open_row(&mut app, 0);
    ctrl(&mut app, 'w');
    keys(&mut app, "h");
    ctrl(&mut app, 'j');
    assert_eq!(app.focus(), Focus::Files);
    assert!(ran(&r).is_empty());
}

#[test]
fn a_list_hidden_while_focused_gives_the_keys_back() {
    let f = fixture();
    let mut app = f.open(ON, 0);
    to_list(&mut app);
    app.event(AppEvent::Resize(COLS, 12));
    assert_eq!(app.focus(), Focus::Files, "the tree takes over");
    let mut app = f.open(ON, 0);
    to_list(&mut app);
    keys(&mut app, " E");
    assert!(!app.commit_list_visible());
    assert_eq!(app.focus(), Focus::Files);
}

// --- <leader>E ---------------------------------------------------------

#[test]
fn leader_shift_e_toggles_the_list_for_the_run() {
    let f = fixture();
    let mut app = f.open("", 0);
    assert!(!app.sidebar_commits());
    keys(&mut app, " E");
    assert!(app.sidebar_commits() && app.commit_list_visible());
    keys(&mut app, "q");
    open_row(&mut app, 1);
    assert!(app.commit_list_visible(), "for the run");
    keys(&mut app, " E");
    assert!(!app.commit_list_visible());
    // On the log it flips the setting for the next commit.
    keys(&mut app, "q");
    keys(&mut app, " E");
    open_row(&mut app, 0);
    assert!(app.commit_list_visible());
    let app = f.open(ON, 0);
    assert!(app.sidebar_commits(), "the config sets the start");
}

#[test]
fn a_launcher_on_leader_shift_e_wins() {
    let f = fixture();
    let toml = format!("{ON}[[launch]]\nname = \"x\"\nkey = \"<leader>E\"\ncommand = [\"true\"]\n");
    let mut app = f.open(&toml, 0);
    keys(&mut app, " E");
    assert!(app.pending_effect().is_some(), "{}", app.status());
    assert!(app.commit_list_visible(), "not toggled");
}

// --- auto peek -----------------------------------------------------------

#[test]
fn an_auto_peek_opens_on_moving_into_the_list_and_stays_open_on_enter() {
    let f = fixture();
    let mut app = f.open(AUTO, 0);
    assert!(!app.sidebar_visible());
    to_list(&mut app);
    assert!(app.sidebar_visible() && app.commit_list_visible());
    keys(&mut app, "j");
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.screen(), Screen::Commit(1));
    assert!(app.sidebar_visible(), "the peek stays while browsing");
    assert_eq!(app.focus(), Focus::Commits);
    keys(&mut app, "j");
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.screen(), Screen::Commit(2));
    assert!(app.sidebar_visible());
    press(&mut app, KeyCode::Esc);
    assert!(!app.sidebar_visible(), "Esc to the diff closes it");
    // Selecting a row from the diff peeks too.
    app.commit_list_select(3);
    assert!(app.sidebar_visible());
    assert_eq!(app.focus(), Focus::Commits);
}

// --- Enter while loading, onto sessions, onto an empty commit ----------

#[test]
fn enter_keeps_the_focus_through_an_async_load() {
    let f = fixture();
    for toml in [ON, AUTO] {
        let mut app = f.app(toml);
        let (tx, rx) = mpsc::channel();
        app.set_sender(tx);
        keys(&mut app, "gg");
        press(&mut app, KeyCode::Enter);
        app.event(rx.recv_timeout(Duration::from_secs(20)).unwrap());
        to_list(&mut app);
        keys(&mut app, "j");
        press(&mut app, KeyCode::Enter);
        assert!(app.commit_loading());
        assert!(app.sidebar_visible(), "{toml}: shown while loading");
        assert!(app.commit_list_visible());
        assert_eq!(app.focus(), Focus::Commits);
        // Browsing goes on while it loads.
        keys(&mut app, "j");
        assert_eq!(app.commit_list_cursor(), 2);
        keys(&mut app, "k");
        loaded(&mut app, &rx);
        assert!(!app.commit_loading());
        assert_eq!(app.screen(), Screen::Commit(1));
        assert_eq!(app.focus(), Focus::Commits, "{toml}");
        assert!(app.sidebar_visible());
    }
}

#[test]
fn the_row_backgrounds_fill_the_whole_row() {
    let f = fixture();
    let mut app = f.open(&format!("{ON}sidebar_width = 40\n"), 2);
    to_list(&mut app);
    keys(&mut app, "j");
    let buf = draw(&mut app, COLS, ROWS);
    let c = app.commit_layout(area(ROWS)).commits.unwrap();
    // The last column inside the border, right of a short summary.
    let x = c.x + c.width - 2;
    let open = &buf[(x, list_y(&app, 2))];
    let cur = &buf[(x, list_y(&app, 3))];
    assert_eq!(open.symbol(), " ");
    assert_eq!(open.bg, palette::SURFACE, "the open row");
    assert_eq!(cur.bg, palette::SURFACE, "the cursor row");
    assert_eq!(buf[(x, list_y(&app, 4))].bg, Color::Reset);
}

/// Saved sessions for log row 1: the file list hidden, the diff focused.
fn save_hidden_session(f: &Fixture) {
    let mut app = f.open(ON, 1);
    keys(&mut app, " e ss");
    assert!(
        app.status().starts_with("Session saved"),
        "{}",
        app.status()
    );
}

#[test]
fn saving_from_the_list_while_a_restored_commit_loads_saves_files_focus() {
    let f = fixture();
    let mut app = f.open(ON, 1);
    keys(&mut app, " ss");
    let id = app.rows()[1].info.commit.clone();
    assert_eq!(
        f.store().load(&id).unwrap().unwrap().view.focus,
        ViewFocus::Diff
    );
    drop(app);
    let mut app = f.app(ON);
    let (tx, rx) = mpsc::channel();
    app.set_sender(tx);
    keys(&mut app, "gg");
    press(&mut app, KeyCode::Enter);
    loaded(&mut app, &rx);
    to_list(&mut app);
    keys(&mut app, "j");
    press(&mut app, KeyCode::Enter);
    assert!(app.commit_loading());
    assert_eq!(app.focus(), Focus::Commits);
    keys(&mut app, " ss");
    assert!(
        app.status().starts_with("Session saved"),
        "{}",
        app.status()
    );
    assert_eq!(
        f.store().load(&id).unwrap().unwrap().view.focus,
        ViewFocus::Files
    );
}

#[test]
fn enter_onto_a_session_with_the_file_list_hidden_keeps_browsing() {
    let f = fixture();
    save_hidden_session(&f);
    let mut app = f.open(ON, 0);
    to_list(&mut app);
    keys(&mut app, "j");
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.screen(), Screen::Commit(1));
    assert_eq!(
        app.sidebar_show(),
        SidebarShow::Never,
        "the session's setting"
    );
    assert!(app.commit_list_visible(), "the list browse wins");
    assert_eq!(app.focus(), Focus::Commits);
    keys(&mut app, "k");
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.screen(), Screen::Commit(0));
    assert_eq!(app.focus(), Focus::Commits);
    keys(&mut app, "j");
    press(&mut app, KeyCode::Enter);
    assert!(app.commit_list_visible());
    // Leaving the sidebar lets the session's setting apply.
    press(&mut app, KeyCode::Esc);
    assert_eq!(app.focus(), Focus::Diff);
    assert!(!app.sidebar_visible());
    // Off, the session hides the list as before.
    let mut app = f.open("", 0);
    keys(&mut app, "q");
    open_row(&mut app, 1);
    assert!(!app.sidebar_visible());
}

/// A hold ends when the commit list goes away, even with the tree
/// focused: the hidden file list doesn't come back with the list.
#[test]
fn a_hold_ends_when_the_list_goes_away() {
    let f = fixture();
    save_hidden_session(&f);
    let mut app = f.open(ON, 0);
    to_list(&mut app);
    keys(&mut app, "j");
    press(&mut app, KeyCode::Enter);
    assert!(app.commit_list_visible(), "held");
    ctrl(&mut app, 'k');
    assert_eq!(app.focus(), Focus::Files);
    keys(&mut app, " E");
    assert!(!app.sidebar_visible(), "the session's never applies");
    assert_eq!(app.focus(), Focus::Diff);
    keys(&mut app, " E");
    assert!(!app.sidebar_visible(), "no hold comes back with the list");
}

#[test]
fn enter_onto_a_session_in_a_narrow_terminal_keeps_browsing() {
    let f = fixture();
    let mut app = f.open(ON, 1);
    keys(&mut app, " ss");
    drop(app);
    // Below sidebar_auto_hide_below an always list shows only after
    // <leader>e; a restored session drops that choice.
    let (mut app, _) = f.app_sized(ON, 70, ROWS);
    open_row(&mut app, 0);
    assert!(!app.sidebar_visible());
    keys(&mut app, " e");
    to_list(&mut app);
    keys(&mut app, "j");
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.screen(), Screen::Commit(1));
    assert!(app.commit_list_visible(), "the list browse wins");
    assert_eq!(app.focus(), Focus::Commits);
    ctrl(&mut app, 'l');
    assert_eq!(app.focus(), Focus::Diff);
    assert!(!app.sidebar_visible());
}

#[test]
fn enter_onto_a_commit_with_a_saved_session_keeps_the_list_focused() {
    let f = fixture();
    // Saved with the tree focused.
    let mut app = f.open(ON, 1);
    ctrl(&mut app, 'h');
    keys(&mut app, " ss");
    drop(app);
    for toml in [ON, AUTO] {
        let mut app = f.app(toml);
        let (tx, rx) = mpsc::channel();
        app.set_sender(tx);
        keys(&mut app, "gg");
        press(&mut app, KeyCode::Enter);
        app.event(rx.recv_timeout(Duration::from_secs(20)).unwrap());
        to_list(&mut app);
        keys(&mut app, "j");
        press(&mut app, KeyCode::Enter);
        loaded(&mut app, &rx);
        assert!(
            app.status().starts_with("Restored session"),
            "{}",
            app.status()
        );
        assert_eq!(app.focus(), Focus::Commits, "{toml}");
        assert!(app.sidebar_visible(), "{toml}");
    }
    // Sync loads too.
    let mut app = f.open(AUTO, 0);
    to_list(&mut app);
    keys(&mut app, "j");
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.focus(), Focus::Commits);
    assert!(app.sidebar_visible());
}

#[test]
fn enter_onto_a_commit_without_files_keeps_browsing() {
    let f = fixture();
    git(&f.root, &["commit", "-q", "--allow-empty", "-m", "empty"]);
    let mut app = f.open(ON, 1);
    to_list(&mut app);
    keys(&mut app, "gg");
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.screen(), Screen::Commit(0));
    assert_eq!(app.commit_diff().map(|d| d.files.len()), Some(0));
    assert!(app.sidebar_visible(), "the list keeps the sidebar");
    assert!(app.commit_list_visible());
    assert_eq!(app.focus(), Focus::Commits);
    // C-k: no tree rows to go to; the edge.
    ctrl(&mut app, 'k');
    assert_eq!(app.focus(), Focus::Commits);
    keys(&mut app, "j");
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.screen(), Screen::Commit(1));
    // From the diff, C-h lands in the list when the tree is empty.
    keys(&mut app, "k");
    press(&mut app, KeyCode::Enter);
    press(&mut app, KeyCode::Esc);
    ctrl(&mut app, 'h');
    assert_eq!(app.focus(), Focus::Commits);
    // Off, an empty commit has no sidebar, as before.
    let app = f.open("", 0);
    assert!(!app.sidebar_visible());
}

// --- following the log ---------------------------------------------------

#[test]
fn the_list_follows_a_log_filter() {
    let f = fixture_n(12);
    let mut app = f.app(ON);
    // No hex ID holds a space.
    keys(&mut app, "ft 1");
    press(&mut app, KeyCode::Enter);
    let shown: Vec<String> = app.rows().iter().map(|r| r.info.summary.clone()).collect();
    assert_eq!(shown, ["commit 12", "commit 11", "commit 10", "commit 1"]);
    let ids: Vec<String> = app
        .rows()
        .iter()
        .map(|r| short_id(&r.info).into())
        .collect();
    open_row(&mut app, 1);
    let lines = list_lines(&mut app);
    for (i, id) in ids.iter().enumerate() {
        assert!(
            body(&lines[i]).starts_with(&format!("{id} ")),
            "{:?}",
            lines[i]
        );
    }
    assert!(lines[4].trim().is_empty(), "nothing past the filter");
    to_list(&mut app);
    keys(&mut app, "G");
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.screen(), Screen::Commit(3));
    let msg = app.commit_diff().unwrap().message.clone();
    assert_eq!(msg.lines().next(), Some("commit 1"));
}

#[test]
fn the_list_follows_a_reload() {
    let f = fixture();
    let mut app = f.open(ON, 2);
    to_list(&mut app);
    keys(&mut app, "j");
    let id = app.rows()[3].info.commit.clone();
    std::fs::write(f.root.join("a.txt"), "later\n").unwrap();
    commit(&f.root, "later");
    app.event(AppEvent::Reload);
    assert_eq!(
        app.screen(),
        Screen::Commit(3),
        "the open commit moved down"
    );
    assert_eq!(app.commit_list_open_row(), Some(3));
    assert_eq!(app.rows()[app.commit_list_cursor()].info.commit, id);
    assert_eq!(app.commit_list_cursor(), 4);
}

#[test]
fn an_open_commit_gone_from_the_log_highlights_no_row() {
    let f = fixture();
    let mut app = f.open(ON, 0);
    git(&f.root, &["commit", "-q", "--amend", "-m", "rewritten"]);
    app.event(AppEvent::Reload);
    assert_eq!(app.commit_list_open_row(), None);
    let buf = draw(&mut app, COLS, ROWS);
    let c = app.commit_layout(area(ROWS)).commits.unwrap();
    for i in 0..5 {
        let bg = buf[(c.x + 3, list_y(&app, i))].bg;
        assert_eq!(bg, Color::Reset, "row {i}");
    }
}

#[test]
fn a_remembered_width_keeps_the_cap_of_the_current_terminal() {
    let f = fixture();
    let long = "src/a/really/quite/long/directory/name/for/the/list/x.rs";
    std::fs::create_dir_all(f.root.join(long).parent().unwrap()).unwrap();
    std::fs::write(f.root.join(long), "x\n").unwrap();
    commit(&f.root, "long");
    git(&f.root, &["commit", "-q", "--allow-empty", "-m", "empty"]);
    let toml = format!("{ON}files = \"flat\"\n");
    let (mut app, _) = f.app_sized(&toml, 160, ROWS);
    open_row(&mut app, 1);
    let wide = Rect::new(0, 0, 160, ROWS - 1);
    assert_eq!(app.commit_layout(wide).sidebar.unwrap().width, 49, "max 48");
    to_list(&mut app);
    keys(&mut app, "k");
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.commit_diff().map(|d| d.files.len()), Some(0));
    assert_eq!(app.commit_layout(wide).sidebar.unwrap().width, 49, "kept");
    app.event(AppEvent::Resize(100, ROWS));
    let narrow = Rect::new(0, 0, 100, ROWS - 1);
    assert_eq!(app.commit_layout(narrow).sidebar.unwrap().width, 36, "35%");
}

// --- the tree half ---------------------------------------------------------

#[test]
fn the_tree_half_page_uses_the_tree_height() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir(&root).unwrap();
    git(&root, &["init", "-q"]);
    std::fs::write(root.join("seed"), "s\n").unwrap();
    commit(&root, "base");
    for i in 0..40 {
        std::fs::write(root.join(format!("f{i:02}.txt")), "x\n").unwrap();
    }
    commit(&root, "many");
    let f = Fixture {
        root: root.canonicalize().unwrap(),
        cache: tmp.path().join("cache"),
        _tmp: tmp,
    };
    let mut app = f.open(ON, 0);
    let tree = app.commit_layout(area(ROWS)).sidebar.unwrap().height as usize;
    ctrl(&mut app, 'h');
    ctrl(&mut app, 'd');
    assert_eq!(app.sidebar_selected(), tree / 2);
    ctrl(&mut app, 'u');
    assert_eq!(app.sidebar_selected(), 0);
    let mut app = f.open("", 0);
    ctrl(&mut app, 'h');
    ctrl(&mut app, 'd');
    assert_eq!(
        app.sidebar_selected(),
        (ROWS as usize - 1) / 2,
        "off: the whole height"
    );
}

// --- sessions and config -------------------------------------------------

#[test]
fn a_session_saved_from_the_list_saves_the_file_list_focus() {
    let f = fixture();
    let mut app = f.open("", 0);
    keys(&mut app, " E");
    to_list(&mut app);
    keys(&mut app, " ss");
    assert!(
        app.status().starts_with("Session saved"),
        "{}",
        app.status()
    );
    let id = app.rows()[0].info.commit.clone();
    let s = f.store().load(&id).unwrap().unwrap();
    assert_eq!(s.view.focus, ViewFocus::Files);
    let json = std::fs::read_to_string(f.store().path(&id).unwrap()).unwrap();
    assert!(!json.contains("sidebar_commits"), "not stored: {json}");
    drop(app);
    let mut app = f.open("", 0);
    assert!(!app.sidebar_commits(), "a session never turns it on");
    assert_eq!(app.focus(), Focus::Files, "restored to the tree");
    keys(&mut app, "j");
    let mut app = f.open(ON, 0);
    assert!(app.sidebar_commits(), "nor off");
    assert_eq!(app.focus(), Focus::Files);
    ctrl(&mut app, 'j');
    assert_eq!(app.focus(), Focus::Commits);
}

#[test]
fn the_config_key_round_trips_and_rejects_bad_values() {
    let c = |s: &str| Config::parse(s).map(|c| c.view.sidebar_commits);
    assert_eq!(c(""), Ok(false));
    assert_eq!(c(ON), Ok(true));
    assert_eq!(c("[view]\nsidebar_commits = false\n"), Ok(false));
    for bad in ["\"yes\"", "1"] {
        let e = c(&format!("[view]\nsidebar_commits = {bad}\n")).unwrap_err();
        assert!(e.starts_with("view.sidebar_commits: invalid type"), "{e}");
    }
}

#[test]
fn help_lists_the_list_keys_while_it_has_them() {
    let f = fixture();
    let mut app = f.open(ON, 0);
    to_list(&mut app);
    keys(&mut app, "g?");
    let v = app.help_view().unwrap();
    assert!(v.lines.contains(&HelpLine::Group("Commit list")));
    let has = |k: &str| {
        v.lines
            .iter()
            .any(|l| matches!(l, HelpLine::Item { keys, .. } if keys == k))
    };
    assert!(has("<leader>E") && has("Enter") && has("C-k"));
    press(&mut app, KeyCode::Esc);
    let mut app = f.open("", 0);
    keys(&mut app, "g?");
    let v = app.help_view().unwrap();
    assert!(
        !v.lines.contains(&HelpLine::Group("Commit list")),
        "not shown"
    );
    assert!(
        v.lines
            .iter()
            .any(|l| matches!(l, HelpLine::Item { keys, .. } if keys == "<leader>E"))
    );
}
