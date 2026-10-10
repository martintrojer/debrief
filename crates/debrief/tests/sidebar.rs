//! The commit view's file list: `[view] sidebar` always /
//! never / auto (peek), `sidebar_side`, `sidebar_auto_hide_below`,
//! `sidebar_width` and its bounds, the peek's triggers, the overlay when
//! there is no room beside the diff, and saved sessions' old `true` /
//! `false`.

mod common;

use std::path::PathBuf;

use common::{commit, git};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use debrief::app::config::{Config, SidebarShow};
use debrief::app::sidebar::Pane;
use debrief::app::{App, AppEvent, Focus, HelpLine, Screen, StartOptions};
use debrief::session::View;
use debrief::vcs;
use debrief_review::Side;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;

const ROWS: u16 = 30;
/// The long file's path: its row needs 1 + 2 + 28 + 6 = 37 columns.
const LONG: &str = "src/deeply/nested/path/x.rs";

struct Fixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    cache: PathBuf,
}

/// `base`, then `change`: a.txt changed, `LONG` added.
fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir(&root).unwrap();
    git(&root, &["init", "-q"]);
    let body: String = (1..=40).map(|i| format!("line {i}\n")).collect();
    std::fs::write(root.join("a.txt"), &body).unwrap();
    commit(&root, "base");
    std::fs::write(root.join("a.txt"), body.replace("line 20\n", "changed\n")).unwrap();
    std::fs::create_dir_all(root.join("src/deeply/nested/path")).unwrap();
    std::fs::write(root.join(LONG), "fn x() {}\n").unwrap();
    commit(&root, "change");
    Fixture {
        root: root.canonicalize().unwrap(),
        cache: tmp.path().join("cache"),
        _tmp: tmp,
    }
}

impl Fixture {
    fn app(&self, toml: &str, cols: u16) -> App {
        let mut opts = StartOptions::new(self.cache.clone());
        opts.config = Config::parse(toml).unwrap();
        let mut app = App::new(
            vcs::open(&self.root, vcs::OpenOptions::default()).unwrap(),
            opts,
        )
        .unwrap();
        app.event(AppEvent::Resize(cols, ROWS));
        app
    }

    /// `change` open, `cols` wide.
    fn open(&self, toml: &str, cols: u16) -> App {
        let mut app = self.app(toml, cols);
        keys(&mut app, "gg");
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.screen(), Screen::Commit(0));
        app
    }
}

fn press(app: &mut App, code: KeyCode) {
    app.key(KeyEvent::new(code, KeyModifiers::NONE));
}

fn ctrl(app: &mut App, c: char) {
    app.key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL));
}

/// `C-w` then `c`.
fn win(app: &mut App, c: char) {
    ctrl(app, 'w');
    keys(app, &c.to_string());
}

fn keys(app: &mut App, s: &str) {
    s.chars().for_each(|c| press(app, KeyCode::Char(c)));
}

/// The layout above the status line of a terminal `cols` wide.
fn layout(app: &App, cols: u16) -> debrief::app::sidebar::CommitLayout {
    app.commit_layout(Rect::new(0, 0, cols, ROWS - 1))
}

fn screen(app: &mut App, cols: u16) -> Vec<String> {
    let mut t = Terminal::new(TestBackend::new(cols, ROWS)).unwrap();
    t.draw(|f| app.render(f)).unwrap();
    let buf = t.backend().buffer().clone();
    (0..buf.area.height)
        .map(|y| (0..cols).map(|x| buf[(x, y)].symbol()).collect())
        .collect()
}

fn hidden(app: &App, cols: u16) -> bool {
    !app.sidebar_visible() && layout(app, cols).sidebar.is_none()
}

// --- always / never / auto ---------------------------------------------

#[test]
fn always_shows_and_leader_e_hides() {
    let f = fixture();
    let mut app = f.open("", 120);
    assert!(app.sidebar_visible());
    assert!(!app.sidebar_overlay());
    keys(&mut app, " e");
    assert!(hidden(&app, 120));
    assert_eq!(app.sidebar_show(), SidebarShow::Never);
    keys(&mut app, " e");
    assert!(app.sidebar_visible());
}

#[test]
fn never_hides_and_leader_e_shows() {
    let f = fixture();
    let mut app = f.open("[view]\nsidebar = \"never\"\n", 120);
    assert!(hidden(&app, 120));
    win(&mut app, 'h');
    assert!(hidden(&app, 120), "never: C-w h opens no peek");
    assert_eq!(app.focus(), Focus::Diff);
    assert_eq!(app.status(), "no file list");
    keys(&mut app, " e");
    assert!(app.sidebar_visible());
    assert_eq!(app.sidebar_show(), SidebarShow::Always);
}

#[test]
fn auto_starts_hidden_with_the_diff_at_full_width() {
    let f = fixture();
    let app = f.open("[view]\nsidebar = \"auto\"\n", 120);
    assert!(hidden(&app, 120));
    assert_eq!(layout(&app, 120).diff.width, 120);
    assert_eq!(app.focus(), Focus::Diff);
}

/// A key press, named.
type Key = fn(&mut App);

/// Every way into the list peeks; every way back hides it.
#[test]
fn auto_peeks_on_each_trigger_and_hides_on_each_way_back() {
    let f = fixture();
    let into: [(&str, Key); 2] = [("C-h", |a| ctrl(a, 'h')), ("C-w h", |a| win(a, 'h'))];
    let back: [(&str, Key); 4] = [
        ("Enter", |a| press(a, KeyCode::Enter)),
        ("Esc", |a| press(a, KeyCode::Esc)),
        ("C-l", |a| ctrl(a, 'l')),
        ("C-w l", |a| win(a, 'l')),
    ];
    for (iname, go) in into {
        for (bname, leave) in back {
            let mut app = f.open("[view]\nsidebar = \"auto\"\n", 160);
            go(&mut app);
            assert_eq!(app.focus(), Focus::Files, "{iname}");
            assert!(app.sidebar_visible(), "{iname} peeks");
            assert!(!app.sidebar_overlay(), "{iname}: room beside at 160");
            leave(&mut app);
            assert_eq!(app.focus(), Focus::Diff, "{iname} then {bname}");
            assert!(hidden(&app, 160), "{iname} then {bname} hides");
        }
    }
}

#[test]
fn enter_in_a_peek_jumps_to_the_file_and_hides() {
    let f = fixture();
    let mut app = f.open("[view]\nsidebar = \"auto\"\n", 160);
    ctrl(&mut app, 'h');
    keys(&mut app, "j");
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.current_file(), Some(1));
    assert!(hidden(&app, 160));
}

#[test]
fn leader_e_pins_an_auto_list_and_unpins_it() {
    let f = fixture();
    let mut app = f.open("[view]\nsidebar = \"auto\"\n", 160);
    keys(&mut app, " e");
    assert!(app.sidebar_visible(), "pinned");
    assert_eq!(app.sidebar_show(), SidebarShow::Always);
    ctrl(&mut app, 'h');
    ctrl(&mut app, 'l');
    assert!(app.sidebar_visible(), "a pin stays when focus returns");
    keys(&mut app, " e");
    assert!(hidden(&app, 160), "unpinned");
    assert_eq!(app.sidebar_show(), SidebarShow::Auto);
    ctrl(&mut app, 'h');
    ctrl(&mut app, 'l');
    assert!(hidden(&app, 160), "back to peeking");
}

#[test]
fn leader_e_on_an_open_peek_hides_it() {
    let f = fixture();
    let mut app = f.open("[view]\nsidebar = \"auto\"\n", 160);
    ctrl(&mut app, 'h');
    keys(&mut app, " e");
    assert!(hidden(&app, 160));
    assert_eq!(app.focus(), Focus::Diff);
    assert_eq!(app.sidebar_show(), SidebarShow::Auto, "still unpinned");
}

#[test]
fn leaving_the_commit_closes_a_peek() {
    let f = fixture();
    let mut app = f.open("[view]\nsidebar = \"auto\"\n", 160);
    ctrl(&mut app, 'h');
    keys(&mut app, "q");
    assert_eq!(app.screen(), Screen::Log);
    press(&mut app, KeyCode::Enter);
    assert!(!app.sidebar_visible());
}

// --- overlay -------------------------------------------------------------

#[test]
fn a_peek_without_room_beside_draws_over_the_diff_without_reflow() {
    let f = fixture();
    // 100 columns: the 36-column list would leave the diff 64 < 80.
    let mut app = f.open("[view]\nsidebar = \"auto\"\n", 100);
    let before = screen(&mut app, 100);
    ctrl(&mut app, 'h');
    assert!(app.sidebar_overlay());
    let l = layout(&app, 100);
    assert!(l.overlay);
    assert_eq!(l.diff.width, 100, "the diff keeps its width");
    let side = l.sidebar.unwrap();
    assert_eq!((side.x, side.width), (0, 36), "35% of 100, and the border");
    let s = screen(&mut app, 100);
    // The list covers the diff's left columns, the rest is unchanged.
    for (y, (row, was)) in s.iter().zip(&before).enumerate().take(3) {
        let tail: String = row.chars().skip(36).collect();
        let was_tail: String = was.chars().skip(36).collect();
        assert_eq!(tail, was_tail, "row {y} right of the list");
    }
    assert!(s[0].contains('│'), "{s:?}");
    assert!(s[1].contains("…c/deeply/nested/path/x.rs"), "{s:?}");
    // Rows under the list don't show the diff through.
    let under: String = s[ROWS as usize - 3].chars().take(35).collect();
    assert_eq!(under.trim(), "", "{s:?}");
    ctrl(&mut app, 'l');
    assert!(hidden(&app, 100));
    assert_eq!(screen(&mut app, 100), before);
}

#[test]
fn a_peek_beside_needs_the_diff_minimum_next_to_it() {
    let f = fixture();
    // The list is 37 + 1 border; beside iff cols - 38 >= 80.
    let mut app = f.open("[view]\nsidebar = \"auto\"\n", 118);
    ctrl(&mut app, 'h');
    assert!(!app.sidebar_overlay(), "118 fits exactly");
    let mut app = f.open("[view]\nsidebar = \"auto\"\n", 117);
    ctrl(&mut app, 'h');
    assert!(app.sidebar_overlay(), "117 is one short");
}

#[test]
fn side_by_side_a_peek_needs_split_min_width_beside_it() {
    let f = fixture();
    let toml = "[view]\nsidebar = \"auto\"\nlayout = \"split\"\n";
    let mut app = f.open(toml, 170);
    assert!(app.commit_split());
    // New side, old side, then the list.
    ctrl(&mut app, 'h');
    ctrl(&mut app, 'h');
    assert_eq!(app.focus(), Focus::Files);
    assert!(app.sidebar_overlay(), "170 - 38 < 140");
    assert!(app.commit_split(), "no reflow back to unified");
    let mut app = f.open(toml, 178);
    ctrl(&mut app, 'h');
    ctrl(&mut app, 'h');
    assert!(!app.sidebar_overlay(), "178 - 38 = 140");
    assert!(app.commit_split());
}

#[test]
fn an_open_split_peek_stays_over_the_diff_through_a_resize_on_both_sides() {
    for side in ["left", "right"] {
        let f = fixture();
        let toml =
            format!("[view]\nsidebar = \"auto\"\nlayout = \"split\"\nsidebar_side = \"{side}\"\n");
        let mut app = f.open(&toml, 170);
        if side == "left" {
            ctrl(&mut app, 'h');
            ctrl(&mut app, 'h');
        } else {
            ctrl(&mut app, 'l');
        }
        assert_eq!(app.focus(), Focus::Files, "{side}");
        assert!(app.sidebar_overlay() && app.commit_split(), "{side}");
        app.event(AppEvent::Resize(139, ROWS));
        assert!(!app.commit_split(), "{side}: too narrow for side by side");
        app.event(AppEvent::Resize(170, ROWS));
        assert!(app.sidebar_overlay(), "{side}: over the diff again");
        assert!(app.commit_split(), "{side}: side by side again");
        assert_eq!(layout(&app, 170).diff.width, 170, "{side}: no reflow");
    }
}

#[test]
fn a_narrow_terminal_overlays_an_auto_list_even_pinned() {
    let f = fixture();
    let mut app = f.open("[view]\nsidebar = \"auto\"\n", 70);
    keys(&mut app, " e");
    assert!(app.sidebar_visible());
    assert!(app.sidebar_overlay(), "below auto_hide_below");
    app.event(AppEvent::Resize(200, ROWS));
    assert!(!app.sidebar_overlay(), "the pin lays out beside when wide");
}

#[test]
fn a_resize_decides_beside_or_over_again() {
    let f = fixture();
    let mut app = f.open("[view]\nsidebar = \"auto\"\n", 200);
    ctrl(&mut app, 'h');
    assert!(!app.sidebar_overlay());
    app.event(AppEvent::Resize(100, ROWS));
    assert!(app.sidebar_overlay());
    app.event(AppEvent::Resize(200, ROWS));
    assert!(!app.sidebar_overlay());
}

#[test]
fn a_right_overlay_sits_at_the_right_edge() {
    let f = fixture();
    let mut app = f.open(
        "[view]\nsidebar = \"auto\"\nsidebar_side = \"right\"\n",
        100,
    );
    ctrl(&mut app, 'h');
    assert!(hidden(&app, 100), "C-h points away from a right list");
    ctrl(&mut app, 'l');
    assert_eq!(app.focus(), Focus::Files);
    let side = layout(&app, 100).sidebar.unwrap();
    assert_eq!((side.x, side.width), (64, 36));
    let s = screen(&mut app, 100);
    assert_eq!(s[0].chars().nth(64), Some('│'), "{s:?}");
    ctrl(&mut app, 'h');
    assert!(hidden(&app, 100));
}

// --- side ------------------------------------------------------------------

#[test]
fn a_right_list_is_drawn_after_the_diff() {
    let f = fixture();
    let mut app = f.open("[view]\nsidebar_side = \"right\"\n", 120);
    let l = layout(&app, 120);
    let side = l.sidebar.unwrap();
    assert_eq!((l.diff.x, l.diff.width), (0, 82));
    assert_eq!((side.x, side.width), (82, 38));
    let s = screen(&mut app, 120);
    assert_eq!(s[0].chars().nth(82), Some('│'), "border toward the diff");
    assert!(s[0].contains("M a.txt"), "{}", s[0]);
    assert!(!s[0].starts_with('▎'), "{}", s[0]);
}

#[test]
fn left_is_the_default_side() {
    let f = fixture();
    let mut app = f.open("", 120);
    let l = layout(&app, 120);
    assert_eq!(l.sidebar.unwrap().x, 0);
    assert_eq!(l.diff.x, 38);
    assert_eq!(screen(&mut app, 120)[0].chars().nth(37), Some('│'));
}

#[test]
fn pane_order_follows_the_side() {
    let f = fixture();
    use Pane::*;
    let app = f.open("", 200);
    assert_eq!(app.panes(), [Files, Diff]);
    let app = f.open("[view]\nsidebar_side = \"right\"\n", 200);
    assert_eq!(app.panes(), [Diff, Files]);
    let app = f.open("[view]\nlayout = \"split\"\n", 200);
    assert_eq!(app.panes(), [Files, Half(Side::Old), Half(Side::New)]);
    let app = f.open(
        "[view]\nlayout = \"split\"\nsidebar_side = \"right\"\n",
        200,
    );
    assert_eq!(app.panes(), [Half(Side::Old), Half(Side::New), Files]);
    let app = f.open("[view]\nsidebar = \"never\"\n", 200);
    assert_eq!(app.panes(), [Diff], "a hidden list is no pane");
    let app = f.open("[view]\nsidebar = \"auto\"\n", 200);
    assert_eq!(app.panes(), [Files, Diff], "an auto list peeks: a pane");
}

#[test]
fn c_h_and_c_l_walk_the_panes_on_the_right_unified_and_split() {
    let f = fixture();
    let mut app = f.open("[view]\nsidebar_side = \"right\"\n", 200);
    assert_eq!(app.current_pane(), Some(Pane::Diff));
    ctrl(&mut app, 'l');
    assert_eq!(app.current_pane(), Some(Pane::Files));
    ctrl(&mut app, 'h');
    assert_eq!(app.current_pane(), Some(Pane::Diff));

    let mut app = f.open(
        "[view]\nlayout = \"split\"\nsidebar_side = \"right\"\n",
        200,
    );
    assert_eq!(app.current_pane(), Some(Pane::Half(Side::New)));
    ctrl(&mut app, 'h');
    assert_eq!(app.current_pane(), Some(Pane::Half(Side::Old)));
    ctrl(&mut app, 'l');
    assert_eq!(app.current_pane(), Some(Pane::Half(Side::New)));
    ctrl(&mut app, 'l');
    assert_eq!(app.current_pane(), Some(Pane::Files));
    ctrl(&mut app, 'h');
    assert_eq!(app.current_pane(), Some(Pane::Half(Side::New)));
    // C-w h / l walk the same order.
    win(&mut app, 'h');
    assert_eq!(app.current_pane(), Some(Pane::Half(Side::Old)));
    win(&mut app, 'l');
    win(&mut app, 'l');
    assert_eq!(app.current_pane(), Some(Pane::Files));
}

// --- auto-hide and width ---------------------------------------------------

#[test]
fn auto_hide_below_hides_an_always_list_and_zero_never_does() {
    let f = fixture();
    let mut app = f.open("", 79);
    assert!(hidden(&app, 79), "default 80");
    app.event(AppEvent::Resize(80, ROWS));
    assert!(app.sidebar_visible(), "comes back at 80");
    let app = f.open("[view]\nsidebar_auto_hide_below = 100\n", 99);
    assert!(hidden(&app, 99));
    let app = f.open("[view]\nsidebar_auto_hide_below = 0\n", 40);
    assert!(app.sidebar_visible(), "0 never hides");
}

#[test]
fn a_list_hidden_with_leader_e_stays_hidden_when_the_terminal_grows() {
    let f = fixture();
    let mut app = f.open("", 120);
    keys(&mut app, " e");
    app.event(AppEvent::Resize(70, ROWS));
    app.event(AppEvent::Resize(120, ROWS));
    assert!(!app.sidebar_visible(), "<leader>e hid it");
    // Shown while narrow holds until the width changes.
    let mut app = f.open("", 70);
    keys(&mut app, " e");
    assert!(app.sidebar_visible(), "shown while narrow");
    app.event(AppEvent::Resize(70, ROWS + 1));
    assert!(app.sidebar_visible(), "a height change keeps it");
    app.event(AppEvent::Resize(72, ROWS));
    assert!(!app.sidebar_visible(), "a width change ends it");
}

fn side_width(app: &App, cols: u16) -> u16 {
    layout(app, cols).sidebar.unwrap().width - 1
}

#[test]
fn widths_auto_number_min_and_max() {
    let f = fixture();
    assert_eq!(side_width(&f.open("", 200), 200), 37, "auto fits the rows");
    assert_eq!(side_width(&f.open("", 100), 100), 35, "35% of 100");
    let app = f.open("[view]\nsidebar_width = 20\n", 200);
    assert_eq!(side_width(&app, 200), 20, "a number as given");
    let app = f.open("[view]\nsidebar_width = 90\n", 200);
    assert_eq!(side_width(&app, 200), 90, "not capped at 35%");
    let app = f.open("[view]\nsidebar_width = 300\n", 120);
    assert_eq!(side_width(&app, 120), 119, "but at the terminal");
    let app = f.open(
        "[view]\nsidebar_min_width = 50\nsidebar_max_width = 60\n",
        200,
    );
    assert_eq!(side_width(&app, 200), 50, "min");
    let app = f.open("[view]\nsidebar_max_width = 24\n", 200);
    assert_eq!(side_width(&app, 200), 24, "max");
}

// --- sessions ----------------------------------------------------------------

#[test]
fn saved_true_and_false_load_as_always_and_never() {
    let view = |json: &str| -> View { serde_json::from_str(json).unwrap() };
    assert_eq!(view(r#"{"sidebar": true}"#).sidebar, SidebarShow::Always);
    assert_eq!(view(r#"{"sidebar": false}"#).sidebar, SidebarShow::Never);
    assert_eq!(view(r#"{"sidebar": "auto"}"#).sidebar, SidebarShow::Auto);
    assert_eq!(view("{}").sidebar, SidebarShow::Always);
    assert!(serde_json::from_str::<View>(r#"{"sidebar": "on"}"#).is_err());
    let json = serde_json::to_value(view(r#"{"sidebar": false}"#)).unwrap();
    assert_eq!(json["sidebar"], "never", "saved in the new form");
}

#[test]
fn a_session_saved_with_the_list_hidden_reopens_hidden() {
    let f = fixture();
    let mut app = f.open("", 120);
    keys(&mut app, " e ss");
    drop(app);
    let app = f.open("", 120);
    assert!(!app.sidebar_visible());
    assert_eq!(app.sidebar_show(), SidebarShow::Never);
}

#[test]
fn an_auto_session_keeps_peeking() {
    let f = fixture();
    let toml = "[view]\nsidebar = \"auto\"\n";
    let mut app = f.open(toml, 160);
    keys(&mut app, " e");
    keys(&mut app, " ss");
    drop(app);
    let mut app = f.open(toml, 160);
    assert!(app.sidebar_visible(), "saved pinned");
    keys(&mut app, " e");
    ctrl(&mut app, 'h');
    assert!(app.sidebar_visible(), "still a peek");
    ctrl(&mut app, 'l');
    assert!(!app.sidebar_visible());
}

#[test]
fn a_saved_file_list_focus_peeks_an_auto_list() {
    let f = fixture();
    let toml = "[view]\nsidebar = \"auto\"\n";
    let mut app = f.open(toml, 160);
    ctrl(&mut app, 'h');
    keys(&mut app, " ss");
    drop(app);
    let app = f.open(toml, 160);
    assert_eq!(app.focus(), Focus::Files);
    assert!(app.sidebar_visible());
}

#[test]
fn a_saved_never_or_old_false_wins_over_an_auto_config() {
    for legacy in [false, true] {
        let f = fixture();
        let mut app = f.open("[view]\nsidebar = \"never\"\n", 170);
        keys(&mut app, " ss");
        drop(app);
        let id = git(&f.root, &["rev-parse", "HEAD"]).trim().to_string();
        if legacy {
            let store = debrief::session::Store::new(&f.cache.join("state"), &f.root);
            let p = store.dir().join(format!("{id}.json"));
            let json = std::fs::read_to_string(&p).unwrap();
            let old = json.replace("\"sidebar\": \"never\"", "\"sidebar\": false");
            assert_ne!(json, old, "legacy={legacy}");
            std::fs::write(p, old).unwrap();
        }
        let mut app = f.open("[view]\nsidebar = \"auto\"\n", 170);
        assert_eq!(app.sidebar_show(), SidebarShow::Never, "legacy={legacy}");
        ctrl(&mut app, 'h');
        assert!(
            !app.sidebar_visible(),
            "never opens no peek, legacy={legacy}"
        );
        keys(&mut app, " ss");
        let saved = app.saved_session(&id).unwrap().view.sidebar;
        assert_eq!(saved, SidebarShow::Never, "saved back unchanged");
    }
}

#[test]
fn a_saved_auto_wins_over_an_always_config() {
    let f = fixture();
    let mut app = f.open("[view]\nsidebar = \"auto\"\n", 170);
    keys(&mut app, " ss");
    drop(app);
    let mut app = f.open("", 170);
    assert_eq!(app.sidebar_show(), SidebarShow::Auto);
    assert!(!app.sidebar_visible());
    ctrl(&mut app, 'h');
    assert!(app.sidebar_visible(), "a peek");
    keys(&mut app, " ss");
    let id = git(&f.root, &["rev-parse", "HEAD"]).trim().to_string();
    assert_eq!(
        app.saved_session(&id).unwrap().view.sidebar,
        SidebarShow::Auto
    );
}

/// The `<leader>e` rows of `g?`, and the leader clue's `e` row.
fn leader_e_texts(app: &mut App) -> (Vec<String>, Vec<String>) {
    keys(app, "g?");
    let help = app
        .help_view()
        .unwrap()
        .lines
        .iter()
        .filter_map(|l| match l {
            HelpLine::Item { keys, desc } if keys == "<leader>e" => Some(desc.clone()),
            _ => None,
        })
        .collect();
    press(app, KeyCode::Esc);
    keys(app, " ");
    let clue = app
        .clue_rows()
        .into_iter()
        .filter(|r| r.key == "e")
        .map(|r| r.desc)
        .collect();
    press(app, KeyCode::Esc);
    (help, clue)
}

#[test]
fn in_auto_the_leader_e_help_says_it_pins() {
    let f = fixture();
    let pin = "show or hide the file list (pin in auto)".to_string();
    let plain = "show or hide the file list".to_string();
    let mut app = f.open("[view]\nsidebar = \"auto\"\n", 170);
    assert_eq!(leader_e_texts(&mut app), (vec![pin.clone()], vec![pin]));
    let mut app = f.open("", 170);
    assert_eq!(leader_e_texts(&mut app), (vec![plain.clone()], vec![plain]));
}
