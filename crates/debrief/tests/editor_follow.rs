//! Following the editor (`--follow`, `<leader>O`): `App::editor_at`
//! moving the diff cursor, and the follow launcher's tmux split. Time is
//! injected (`Instant`s past the grace window), never slept.

mod common;

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use common::{git_repo, keys, press, rm, w};
use crossterm::event::KeyCode;
use debrief::app::config::Config;
use debrief::app::{
    App, AppEvent, Effect, Exit, FOLLOW_GRACE, Mode, RowKind, Screen, StartOptions,
};
use debrief::vcs::{self, LineKind};

struct Fixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    cache: PathBuf,
}

/// `base` then `change`: long.txt (200 lines) edits lines 10 and 150;
/// other.txt is added; untouched.txt is in base only; old.txt is renamed
/// to new.txt with line 2 edited; blob.bin is binary.
fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    let lines: Vec<String> = (1..=200).map(|i| format!("l{i}")).collect();
    let mut changed = lines.clone();
    changed[9] = "l10 edited".into();
    changed[149] = "l150 edited".into();
    git_repo(
        &root,
        &[
            (
                "base",
                vec![
                    w("long.txt", lines.join("\n") + "\n"),
                    w("untouched.txt", "u\n"),
                    w("old.txt", "r1\nr2\nr3\nr4\nr5\n"),
                    w("blob.bin", b"\0one"),
                ],
            ),
            (
                "change",
                vec![
                    w("long.txt", changed.join("\n") + "\n"),
                    w("other.txt", "o1\no2\n"),
                    rm("old.txt"),
                    w("new.txt", "r1\nR2\nr3\nr4\nr5\n"),
                    w("blob.bin", b"\0two"),
                ],
            ),
        ],
    );
    Fixture {
        cache: tmp.path().join("cache"),
        root: root.canonicalize().unwrap(),
        _tmp: tmp,
    }
}

impl Fixture {
    fn app_with(&self, config: Config, env: &'static [(&'static str, &'static str)]) -> App {
        let opts = StartOptions {
            config,
            ..StartOptions::new(self.cache.clone())
        };
        let mut app = App::new(
            vcs::open(&self.root, vcs::OpenOptions::default()).unwrap(),
            opts,
        )
        .unwrap()
        .with_env(move |k| {
            env.iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| v.to_string())
        });
        app.event(AppEvent::Resize(100, 30));
        app
    }

    /// `change` open, as a reader would get there.
    fn open(&self) -> App {
        let mut app = self.app_with(Config::defaults(), &[]);
        keys(&mut app, "gg");
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.screen(), Screen::Commit(0));
        app
    }

    fn path(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }
}

/// Well past the grace window the keys above started.
fn later() -> Instant {
    Instant::now() + FOLLOW_GRACE * 2
}

/// The new-side line number and text under the diff cursor.
fn at(app: &App) -> (Option<u32>, String) {
    let r = &app.commit_rows()[app.commit_cursor().row];
    let new = r
        .hunk
        .zip(r.line)
        .and_then(|(h, l)| app.commit_diff().unwrap().files[r.file?].hunks[h].lines[l].new);
    (new, r.text.clone())
}

#[test]
fn a_changed_line_puts_the_cursor_on_it() {
    let f = fixture();
    let mut app = f.open();
    app.editor_at(&f.path("long.txt"), 10, later());
    assert_eq!(at(&app), (Some(10), "l10 edited".into()));
    let r = &app.commit_rows()[app.commit_cursor().row];
    assert_eq!(r.kind, RowKind::Line(LineKind::Added));
}

#[test]
fn a_line_outside_the_hunks_lands_on_the_closest() {
    let f = fixture();
    let mut app = f.open();
    // Hunks hold 7..=13 and 147..=153 (3 lines of context).
    app.editor_at(&f.path("long.txt"), 60, later());
    assert_eq!(at(&app).0, Some(13));
    app.editor_at(&f.path("long.txt"), 120, later());
    assert_eq!(at(&app).0, Some(147));
    app.editor_at(&f.path("long.txt"), 10_000, later());
    assert_eq!(at(&app).0, Some(153));
    // 80 is 67 from both 13 and 147: the earlier wins.
    app.editor_at(&f.path("long.txt"), 80, later());
    assert_eq!(at(&app).0, Some(13));
}

#[test]
fn a_binary_file_lands_on_its_header() {
    let f = fixture();
    let mut app = f.open();
    keys(&mut app, "gg");
    app.editor_at(&f.path("blob.bin"), 1, later());
    let r = &app.commit_rows()[app.commit_cursor().row];
    assert_eq!(r.kind, RowKind::FileHeader);
    assert!(r.text.contains("blob.bin"), "{}", r.text);
}

#[test]
fn a_renamed_file_matches_on_its_new_path_only() {
    let f = fixture();
    let mut app = f.open();
    let renamed = app.commit_diff().unwrap().files.iter().any(|d| {
        d.old_path.as_deref() == Some("old.txt") && d.new_path.as_deref() == Some("new.txt")
    });
    assert!(renamed, "git saw the rename");
    keys(&mut app, "gg");
    let cur = app.commit_cursor();
    app.editor_at(&f.path("old.txt"), 2, later());
    assert_eq!(app.commit_cursor(), cur, "the old path is gone");
    app.editor_at(&f.path("new.txt"), 2, later());
    assert_eq!(at(&app), (Some(2), "R2".into()));
}

#[test]
fn updates_while_the_diff_reloads_are_dropped() {
    let f = fixture();
    let mut app = f.app_with(Config::defaults(), &[]);
    // A sender: diffs load on a worker. `T` reloads in place, keeping the
    // old diff on screen while the new one loads.
    let (tx, rx) = std::sync::mpsc::channel();
    app.set_sender(tx);
    let loaded = |app: &mut App| loop {
        let ev = rx.recv_timeout(Duration::from_secs(20)).expect("Loaded");
        let done = matches!(ev, AppEvent::Loaded { .. });
        app.event(ev);
        if done {
            break;
        }
    };
    keys(&mut app, "gg");
    press(&mut app, KeyCode::Enter);
    loaded(&mut app);
    keys(&mut app, "T");
    assert!(app.commit_loading());
    assert!(app.commit_diff().is_some(), "the old diff is still there");
    app.editor_at(&f.path("long.txt"), 150, later());
    assert_ne!(at(&app).0, Some(150), "dropped while loading");
    loaded(&mut app);
    assert!(!app.commit_loading());
    app.editor_at(&f.path("long.txt"), 150, later());
    assert_eq!(at(&app).0, Some(150), "followed once loaded");
}

#[test]
fn an_off_screen_row_is_centred_and_a_visible_one_only_moves_the_cursor() {
    let f = fixture();
    let mut app = f.open();
    // Shorter than the diff, so long.txt's second hunk is off screen.
    app.event(AppEvent::Resize(100, 12));
    let before = app.commit_scroll().0;
    app.editor_at(&f.path("long.txt"), 150, later());
    let row = app.commit_cursor().row;
    let scroll = app.commit_scroll().0;
    assert_ne!(scroll, before, "scrolled to it");
    assert!(row > scroll && row - scroll > 3, "centred, not at the edge");
    // A line near the top of that screen: centring would scroll back up.
    app.editor_at(&f.path("long.txt"), 147, later());
    let near_top = app.commit_cursor().row;
    assert!(
        near_top - scroll <= 2,
        "near the top: {near_top} vs {scroll}"
    );
    assert_eq!(app.commit_scroll().0, scroll, "visible: no scroll");
}

#[test]
fn files_not_in_the_diff_or_outside_the_repo_are_ignored_silently() {
    let f = fixture();
    let mut app = f.open();
    keys(&mut app, "gg");
    let cur = app.commit_cursor();
    for p in [
        f.path("untouched.txt"),
        PathBuf::from("/etc/hosts"),
        f.path("gone.txt"),
    ] {
        app.editor_at(&p, 1, later());
    }
    assert_eq!(app.commit_cursor(), cur);
    assert_eq!(app.status(), "");
}

#[test]
fn the_log_ignores_updates() {
    let f = fixture();
    let mut app = f.app_with(Config::defaults(), &[]);
    let cursor = app.cursor();
    app.editor_at(&f.path("long.txt"), 10, later());
    assert_eq!(app.screen(), Screen::Log);
    assert_eq!(app.cursor(), cursor);
}

#[test]
fn updates_in_the_grace_window_are_dropped() {
    let f = fixture();
    let mut app = f.open();
    keys(&mut app, "gg");
    let typed = Instant::now();
    let cur = app.commit_cursor();
    app.editor_at(&f.path("long.txt"), 10, typed + FOLLOW_GRACE / 2);
    assert_eq!(app.commit_cursor(), cur, "dropped inside the window");
    app.editor_at(
        &f.path("long.txt"),
        10,
        typed + FOLLOW_GRACE + Duration::from_millis(10),
    );
    assert_eq!(at(&app).0, Some(10), "followed after it");
}

#[test]
fn a_prompt_or_half_typed_key_pauses_following() {
    let f = fixture();
    for typed in ["/", "g", "5", "V"] {
        let mut app = f.open();
        keys(&mut app, "gg");
        keys(&mut app, typed);
        let cur = app.commit_cursor().row;
        app.editor_at(&f.path("long.txt"), 150, later());
        assert_eq!(app.commit_cursor().row, cur, "after {typed:?}");
    }
    let mut app = f.open();
    keys(&mut app, "/");
    assert_eq!(app.mode(), Mode::Search);
}

#[test]
fn side_by_side_moves_to_the_new_side() {
    let f = fixture();
    let mut app = f.open();
    app.event(AppEvent::Resize(200, 30));
    keys(&mut app, "t");
    assert_eq!(app.split_side(), Some(debrief_review::Side::New));
    press(&mut app, KeyCode::Tab);
    assert_eq!(app.split_side(), Some(debrief_review::Side::Old));
    app.editor_at(&f.path("long.txt"), 10, later());
    assert_eq!(app.split_side(), Some(debrief_review::Side::New));
    assert_eq!(at(&app), (Some(10), "l10 edited".into()));
}

const TMUX: &[(&str, &str)] = &[
    ("TMUX", "/tmp/tmux-1/default,42,0"),
    ("TMUX_PANE", "%7"),
    ("VISUAL", "nvim"),
];

#[test]
fn leader_shift_o_in_tmux_splits_without_suspending() {
    let f = fixture();
    let mut app = f.app_with(Config::defaults(), TMUX);
    keys(&mut app, "gg");
    press(&mut app, KeyCode::Enter);
    app.editor_at(&f.path("long.txt"), 10, later());
    keys(&mut app, " O");
    let Some(Effect::Split(cmd)) = app.pending_effect().cloned() else {
        panic!("not a split: {:?} / {}", app.pending_effect(), app.status());
    };
    assert!(!app.effect_suspends());
    let root = f.root.display().to_string();
    let file = f.path("long.txt").display().to_string();
    assert_eq!(
        cmd.argv,
        [
            "tmux",
            "split-window",
            "-h",
            "-t",
            "%7",
            "-c",
            &root,
            "--",
            "nvim",
            "+10",
            &file
        ]
    );
    // Run through the pane runner (never the terminal one).
    let seen = Rc::new(RefCell::new(Vec::new()));
    let log = seen.clone();
    let mut app = app
        .with_pane_runner(move |c| {
            log.borrow_mut().push(c.argv.clone());
            Ok(Exit::Code(0))
        })
        .with_runner(|_| panic!("a split must not suspend"));
    app.run_pending_effect();
    assert_eq!(seen.borrow().len(), 1);
    assert_eq!(app.pending_effect(), None);
    // The `O` keypress hands over: the editor's first position counts.
    app.editor_at(&f.path("long.txt"), 150, Instant::now());
    assert_eq!(at(&app).0, Some(150));
}

#[test]
fn a_failing_split_is_a_status() {
    let f = fixture();
    let mut app = f
        .app_with(Config::defaults(), TMUX)
        .with_pane_runner(|_| Ok(Exit::Code(1)));
    keys(&mut app, "gg");
    keys(&mut app, " O");
    app.run_pending_effect();
    assert_eq!(app.status(), "edit-follow: tmux exited with status 1");
}

#[test]
fn a_slow_tmux_is_not_an_error() {
    let f = fixture();
    let mut app = f.app_with(Config::defaults(), TMUX).with_pane_runner(|_| {
        Err(anyhow::Error::new(debrief::app::pane_nav::StillRunning)
            .context("tmux still running after 200ms"))
    });
    keys(&mut app, "gg");
    keys(&mut app, " O");
    app.run_pending_effect();
    assert_eq!(app.status(), "");
}

#[test]
fn leader_shift_o_outside_tmux_is_a_plain_launch() {
    let f = fixture();
    let mut app = f.app_with(Config::defaults(), &[("VISUAL", "nvim")]);
    keys(&mut app, "gg");
    keys(&mut app, " O");
    assert!(
        matches!(app.pending_effect(), Some(Effect::Launch(l)) if l.name == "edit-follow"),
        "{:?}",
        app.pending_effect()
    );
    assert!(app.effect_suspends());
}

#[test]
fn follow_is_a_launcher_option() {
    let f = fixture();
    let c = Config::parse(
        "[[launch]]\nname = \"lg\"\nkey = \"<leader>g\"\ncommand = [\"lazygit\"]\nfollow = true\n",
    )
    .unwrap();
    let mut app = f.app_with(c, TMUX);
    keys(&mut app, " g");
    let Some(Effect::Split(cmd)) = app.pending_effect() else {
        panic!("{:?}", app.pending_effect());
    };
    assert_eq!(cmd.argv.last().map(String::as_str), Some("lazygit"));
    assert!(Path::new(&cmd.argv[6]).is_absolute());
}

fn mouse(kind: crossterm::event::MouseEventKind) -> AppEvent {
    AppEvent::Mouse(crossterm::event::MouseEvent {
        kind,
        column: 50,
        row: 10,
        modifiers: crossterm::event::KeyModifiers::NONE,
    })
}

#[test]
fn a_click_or_paste_starts_the_grace_window_but_hovering_does_not() {
    use crossterm::event::MouseEventKind as K;
    let f = fixture();
    let cases = [
        (mouse(K::Moved), false),
        (mouse(K::ScrollDown), true),
        (AppEvent::Paste("x".into()), true),
    ];
    for (ev, pauses) in cases {
        let mut app = f.open();
        let before = app.editor_quiet_until().expect("the keys started one");
        let what = format!("{ev:?}");
        // Strictly later than the keys' window start.
        std::thread::sleep(Duration::from_millis(2));
        app.event(ev);
        let after = app.editor_quiet_until().unwrap();
        assert_eq!(after > before, pauses, "{what}");
    }
}

/// The listener: started once (a second start doesn't rebind), and a
/// bind failure is the "follow off" status. Needs a sender, as in `run`.
#[cfg(unix)]
#[test]
fn the_listener_starts_once_and_a_failure_is_a_status() {
    use std::os::unix::fs::PermissionsExt;
    let f = fixture();
    // A short base dir (socket paths are capped near 104 bytes); `listen`
    // makes the socket dir inside it, 0700.
    let base = tempfile::Builder::new()
        .prefix("f")
        .tempdir_in("/tmp")
        .unwrap();
    let dir = base.path().join("d");
    let mut app = f.app_with(Config::defaults(), &[]);
    let (tx, _rx) = std::sync::mpsc::channel();
    app.set_sender(tx);
    assert!(!app.editor_following());
    app.editor_follow_start_in(&dir);
    assert!(app.editor_following(), "{}", app.status());
    let socks = || std::fs::read_dir(&dir).unwrap().count();
    assert_eq!(socks(), 1);
    app.editor_follow_start_in(&dir);
    assert_eq!(socks(), 1, "no rebind");
    drop(app);
    assert_eq!(socks(), 0, "dropping the app removes the socket");

    // A directory others can open is refused.
    let mut app = f.app_with(Config::defaults(), &[]);
    let (tx, _rx) = std::sync::mpsc::channel();
    app.set_sender(tx);
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    app.editor_follow_start_in(&dir);
    assert!(!app.editor_following());
    assert!(app.status().starts_with("follow off: "), "{}", app.status());
}
