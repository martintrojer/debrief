//! `C-h/j/k/l` (Q36–Q39): file list <-> diff, file list -> old -> new
//! side by side, the tmux hand-off at an edge only inside tmux, the
//! comment box keeping its keys, and `[keys] pane_nav = false`.

mod common;

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use common::{commit, git};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use debrief::app::config::Config;
use debrief::app::sidebar::Pane;
use debrief::app::{App, AppEvent, Exit, Focus, HelpLine, Mode, Screen, StartOptions};
use debrief::vcs;
use debrief_review::Side;

struct Fixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    cache: PathBuf,
}

fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir(&root).unwrap();
    git(&root, &["init", "-q"]);
    std::fs::write(root.join("a.txt"), "c1\nr1\nc2\n").unwrap();
    commit(&root, "base");
    std::fs::write(root.join("a.txt"), "c1\na1\nc2\n").unwrap();
    std::fs::write(root.join("b.txt"), "b\n").unwrap();
    commit(&root, "change");
    Fixture {
        root: root.canonicalize().unwrap(),
        cache: tmp.path().join("cache"),
        _tmp: tmp,
    }
}

/// The tmux commands the app ran.
type Ran = Rc<RefCell<Vec<String>>>;

impl Fixture {
    /// The log, `$TMUX` set or not, tmux commands recorded.
    fn log(&self, toml: &str, tmux: bool) -> (App, Ran) {
        let mut opts = StartOptions::new(self.cache.clone());
        opts.config = Config::parse(toml).unwrap();
        let ran: Ran = Rc::default();
        let r = ran.clone();
        let mut app = App::new(
            vcs::open(&self.root, vcs::OpenOptions::default()).unwrap(),
            opts,
        )
        .unwrap()
        .with_env(move |k| (k == "TMUX" && tmux).then(|| "/tmp/tmux-1/default,1,0".into()))
        .with_pane_runner(move |c| {
            r.borrow_mut().push(c.argv.join(" "));
            Ok(Exit::Code(0))
        });
        app.event(AppEvent::Resize(200, 30));
        (app, ran)
    }

    /// `change` open.
    fn open(&self, toml: &str, tmux: bool) -> (App, Ran) {
        let (mut app, ran) = self.log(toml, tmux);
        keys(&mut app, "gg");
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.screen(), Screen::Commit(0));
        (app, ran)
    }
}

fn press(app: &mut App, code: KeyCode) {
    app.key(KeyEvent::new(code, KeyModifiers::NONE));
}

fn ctrl(app: &mut App, c: char) {
    app.key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL));
}

fn keys(app: &mut App, s: &str) {
    s.chars().for_each(|c| press(app, KeyCode::Char(c)));
}

fn ran(r: &Ran) -> Vec<String> {
    std::mem::take(&mut r.borrow_mut())
}

#[test]
fn c_h_and_c_l_move_between_the_file_list_and_the_diff() {
    let f = fixture();
    let (mut app, r) = f.open("", true);
    assert_eq!(app.focus(), Focus::Diff);
    ctrl(&mut app, 'h');
    assert_eq!(app.focus(), Focus::Files);
    ctrl(&mut app, 'l');
    assert_eq!(app.focus(), Focus::Diff);
    assert!(ran(&r).is_empty(), "moves inside debrief run no tmux");
}

#[test]
fn side_by_side_c_l_goes_from_the_list_to_old_then_new() {
    let f = fixture();
    let (mut app, r) = f.open("[view]\nlayout = \"split\"\n", true);
    assert!(app.commit_split());
    assert_eq!(app.current_pane(), Some(Pane::Half(Side::New)));
    ctrl(&mut app, 'h');
    assert_eq!(app.current_pane(), Some(Pane::Half(Side::Old)));
    assert_eq!(app.split_side(), Some(Side::Old));
    ctrl(&mut app, 'h');
    assert_eq!(app.current_pane(), Some(Pane::Files));
    ctrl(&mut app, 'l');
    assert_eq!(app.current_pane(), Some(Pane::Half(Side::Old)));
    ctrl(&mut app, 'l');
    assert_eq!(app.current_pane(), Some(Pane::Half(Side::New)));
    // Tab still switches sides.
    press(&mut app, KeyCode::Tab);
    assert_eq!(app.split_side(), Some(Side::Old));
    assert!(ran(&r).is_empty());
    // C-w h / C-w l walk the same panes.
    let (mut app, _) = f.open("[view]\nlayout = \"split\"\n", false);
    ctrl(&mut app, 'w');
    keys(&mut app, "h");
    assert_eq!(app.split_side(), Some(Side::Old));
    ctrl(&mut app, 'w');
    keys(&mut app, "h");
    assert_eq!(app.focus(), Focus::Files);
    ctrl(&mut app, 'w');
    keys(&mut app, "l");
    assert_eq!(app.current_pane(), Some(Pane::Half(Side::Old)));
}

#[test]
fn edges_hand_off_to_tmux_with_the_flag_of_the_move() {
    let f = fixture();
    let (mut app, r) = f.open("", true);
    ctrl(&mut app, 'l');
    assert_eq!(ran(&r), ["tmux select-pane -R"], "the diff is rightmost");
    ctrl(&mut app, 'j');
    assert_eq!(ran(&r), ["tmux select-pane -D"], "no pane below");
    ctrl(&mut app, 'k');
    assert_eq!(ran(&r), ["tmux select-pane -U"], "no pane above");
    ctrl(&mut app, 'h');
    assert!(ran(&r).is_empty(), "the list is a pane");
    ctrl(&mut app, 'h');
    assert_eq!(ran(&r), ["tmux select-pane -L"], "the list is leftmost");
    assert_eq!(app.focus(), Focus::Files, "focus stays");
}

#[test]
fn a_hidden_list_is_an_edge() {
    let f = fixture();
    let (mut app, r) = f.open("[view]\nsidebar = \"never\"\n", true);
    ctrl(&mut app, 'h');
    assert_eq!(ran(&r), ["tmux select-pane -L"]);
    assert_eq!(app.focus(), Focus::Diff);
    // An auto list peeks instead.
    let (mut app, r) = f.open("[view]\nsidebar = \"auto\"\n", true);
    ctrl(&mut app, 'h');
    assert!(ran(&r).is_empty());
    assert_eq!(app.focus(), Focus::Files);
}

#[test]
fn right_side_edges_follow_the_screen() {
    let f = fixture();
    let (mut app, r) = f.open("[view]\nsidebar_side = \"right\"\n", true);
    ctrl(&mut app, 'h');
    assert_eq!(ran(&r), ["tmux select-pane -L"], "the diff is leftmost");
    ctrl(&mut app, 'l');
    assert_eq!(app.focus(), Focus::Files);
    ctrl(&mut app, 'l');
    assert_eq!(ran(&r), ["tmux select-pane -R"]);
}

#[test]
fn the_log_hands_every_move_to_tmux() {
    let f = fixture();
    let (mut app, r) = f.log("", true);
    for (c, flag) in [('h', "-L"), ('j', "-D"), ('k', "-U"), ('l', "-R")] {
        ctrl(&mut app, c);
        assert_eq!(ran(&r), [format!("tmux select-pane {flag}")], "C-{c}");
    }
    assert_eq!(app.screen(), Screen::Log);
    assert!(app.stats_visible(), "the stats pane is no pane to move to");
}

#[test]
fn without_tmux_an_edge_does_nothing() {
    let f = fixture();
    let (mut app, r) = f.open("", false);
    for c in ['l', 'j', 'k'] {
        ctrl(&mut app, c);
    }
    assert_eq!(app.focus(), Focus::Diff);
    assert_eq!(app.status(), "");
    assert!(ran(&r).is_empty());
    let (mut app, r) = f.log("", false);
    for c in ['h', 'j', 'k', 'l'] {
        ctrl(&mut app, c);
    }
    assert!(ran(&r).is_empty());
    // An empty $TMUX is no tmux.
    let (app, r) = f.open("", false);
    let mut app = app.with_env(|k| (k == "TMUX").then(String::new));
    ctrl(&mut app, 'l');
    assert!(ran(&r).is_empty());
}

#[test]
fn a_failing_tmux_is_ignored() {
    let f = fixture();
    let (app, _) = f.open("", true);
    let mut app = app.with_pane_runner(|_| anyhow::bail!("no server running"));
    ctrl(&mut app, 'l');
    assert_eq!(app.status(), "");
    assert_eq!(app.focus(), Focus::Diff);
}

#[test]
fn the_comment_box_keeps_its_keys() {
    let f = fixture();
    let (mut app, r) = f.open("", true);
    keys(&mut app, "/a1");
    press(&mut app, KeyCode::Enter);
    keys(&mut app, "cc");
    assert_eq!(app.mode(), Mode::Comment);
    keys(&mut app, "ab");
    ctrl(&mut app, 'j');
    keys(&mut app, "c");
    for c in ['h', 'k', 'l'] {
        ctrl(&mut app, c);
    }
    assert_eq!(app.mode(), Mode::Comment);
    assert_eq!(
        app.comment_text(),
        Some("ab\nc"),
        "C-j a newline, C-h/k/l nothing"
    );
    assert_eq!(app.focus(), Focus::Diff);
    assert!(ran(&r).is_empty());
}

#[test]
fn visual_mode_leaves_them_unbound() {
    let f = fixture();
    let (mut app, r) = f.open("", true);
    keys(&mut app, "v");
    ctrl(&mut app, 'h');
    ctrl(&mut app, 'l');
    assert_eq!(app.mode(), Mode::Visual);
    assert_eq!(app.focus(), Focus::Diff);
    assert!(ran(&r).is_empty());
}

#[test]
fn pane_nav_false_unbinds_them_and_keeps_c_w() {
    let f = fixture();
    let toml = "[keys]\npane_nav = false\n";
    let (mut app, r) = f.open(toml, true);
    for c in ['h', 'j', 'k', 'l'] {
        ctrl(&mut app, c);
    }
    assert_eq!(app.focus(), Focus::Diff);
    assert!(ran(&r).is_empty());
    ctrl(&mut app, 'w');
    keys(&mut app, "h");
    assert_eq!(app.focus(), Focus::Files, "C-w h stays");
    let (mut app, r) = f.log(toml, true);
    ctrl(&mut app, 'l');
    assert!(ran(&r).is_empty());
}

fn help_has(app: &mut App, keys_: &str) -> bool {
    keys(app, "g?");
    let v = app.help_view().unwrap();
    let has = v
        .lines
        .iter()
        .any(|l| matches!(l, HelpLine::Item { keys, .. } if keys == keys_));
    press(app, KeyCode::Esc);
    has
}

#[test]
fn help_lists_the_keys_while_bound() {
    let f = fixture();
    let (mut app, _) = f.open("", false);
    assert!(help_has(&mut app, "C-h"));
    assert!(help_has(&mut app, "C-l"));
    assert!(help_has(&mut app, "C-j, C-k"));
    let (mut app, _) = f.log("", false);
    assert!(help_has(&mut app, "C-h, C-j, C-k, C-l"));
    let (mut app, _) = f.open("[keys]\npane_nav = false\n", false);
    assert!(!help_has(&mut app, "C-h"));
    assert!(help_has(&mut app, "C-w h"));
}
