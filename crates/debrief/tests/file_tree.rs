//! The commit view's file list as a tree: lazygit's layout, Enter / h / l
//! / - / = / ` in the list, the diff's file order, `▎` on a collapsed
//! directory, the auto width, `[view] files` and saved sessions.

mod common;

use std::path::PathBuf;

use common::{commit, git};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use debrief::app::config::Config;
use debrief::app::{App, AppEvent, Focus, RowKind, Screen, StartOptions};
use debrief::session::{Store, ViewFiles};
use debrief::vcs;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;

const ROWS: u16 = 30;

struct Fixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    cache: PathBuf,
}

/// `base`, then `change`: z.txt, src/main.rs, src/app/x.rs,
/// pkg/gui/helpers/a.go and b.txt changed or added (git lists them
/// alphabetically by path).
fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir(&root).unwrap();
    git(&root, &["init", "-q"]);
    std::fs::write(root.join("z.txt"), "z\n").unwrap();
    commit(&root, "base");
    std::fs::write(root.join("z.txt"), "z2\n").unwrap();
    for (p, body) in [
        ("src/main.rs", "fn main() {}\n"),
        ("src/app/x.rs", "a\nb\n"),
        ("pkg/gui/helpers/a.go", "package helpers\n"),
        ("b.txt", "b\n"),
    ] {
        let path = root.join(p);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }
    commit(&root, "change");
    Fixture {
        root: root.canonicalize().unwrap(),
        cache: tmp.path().join("cache"),
        _tmp: tmp,
    }
}

impl Fixture {
    fn open(&self, toml: &str) -> App {
        let mut opts = StartOptions::new(self.cache.clone());
        opts.config = Config::parse(toml).unwrap();
        let mut app = App::new(
            vcs::open(&self.root, vcs::OpenOptions::default()).unwrap(),
            opts,
        )
        .unwrap();
        app.event(AppEvent::Resize(120, ROWS));
        keys(&mut app, "gg");
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.screen(), Screen::Commit(0));
        app
    }

    fn store(&self) -> Store {
        Store::new(&self.cache.join("state"), &self.root)
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

fn to_list(app: &mut App) {
    ctrl(app, 'w');
    keys(app, "h");
    assert_eq!(app.focus(), Focus::Files);
}

/// The file list's rows as drawn (gutter included, trailing space cut).
fn list(app: &mut App) -> Vec<String> {
    let mut t = Terminal::new(TestBackend::new(120, ROWS)).unwrap();
    t.draw(|f| app.render(f)).unwrap();
    let buf = t.backend().buffer().clone();
    let w = app
        .commit_layout(Rect::new(0, 0, 120, ROWS - 1))
        .sidebar
        .unwrap()
        .width
        - 1;
    let n = app.file_list().unwrap().visible.rows.len();
    (0..n as u16)
        .map(|y| {
            let s: String = (0..w).map(|x| buf[(x, y)].symbol()).collect();
            s.trim_end().to_string()
        })
        .collect()
}

/// The paths of the diff's file headers, top to bottom.
fn diff_order(app: &App) -> Vec<String> {
    let d = app.commit_diff().unwrap();
    app.commit_rows()
        .iter()
        .filter(|r| r.kind == RowKind::FileHeader)
        .map(|r| debrief::app::commit::file_path(&d.files[r.file.unwrap()]).to_string())
        .collect()
}

/// `]f` from the top, visiting every file: the files in that order.
fn bracket_order(app: &mut App) -> Vec<String> {
    keys(app, "gg");
    let d = app.commit_diff().unwrap().clone();
    let mut out = Vec::new();
    for _ in 0..d.files.len() {
        keys(app, "]f");
        let f = app.current_file().unwrap();
        out.push(debrief::app::commit::file_path(&d.files[f]).to_string());
    }
    out
}

const TREE: [&str; 5] = [
    "pkg/gui/helpers/a.go",
    "src/app/x.rs",
    "src/main.rs",
    "b.txt",
    "z.txt",
];

#[test]
fn rows_are_a_tree_with_compressed_dirs_first() {
    let f = fixture();
    let mut app = f.open("");
    let rows = list(&mut app);
    let want = [
        " ▾ pkg/gui/helpers +1 −0",
        "   A a.go",
        " ▾ src +3 −0",
        "   ▾ app +2 −0",
        "     A x.rs",
        "   A main.rs",
        " A b.txt",
        " M z.txt",
    ];
    assert_eq!(rows.len(), want.len(), "{rows:#?}");
    for (r, w) in rows.iter().zip(want) {
        // Counts are right-aligned: compare the start and the end.
        let (name, counts) = w.split_once(" +").unwrap_or((w, ""));
        assert!(
            r.starts_with(&format!("{name} ")),
            "{r:?} vs {w:?}\n{rows:#?}"
        );
        if !counts.is_empty() {
            assert!(r.ends_with(&format!("+{counts}")), "{r:?} vs {w:?}");
        }
    }
    assert!(rows[1].ends_with("+1 −0"), "{}", rows[1]);
}

#[test]
fn the_diff_and_bracket_f_follow_the_tree_and_flat_keeps_git_order() {
    let f = fixture();
    let mut app = f.open("");
    assert_eq!(diff_order(&app), TREE);
    assert_eq!(bracket_order(&mut app), TREE);
    // ` in the list: flat, git's order, whole paths.
    to_list(&mut app);
    keys(&mut app, "`");
    assert!(!app.files_tree());
    let git_order = [
        "b.txt",
        "pkg/gui/helpers/a.go",
        "src/app/x.rs",
        "src/main.rs",
        "z.txt",
    ];
    assert_eq!(diff_order(&app), git_order);
    let rows = list(&mut app);
    assert!(rows[1].starts_with(" A pkg/gui/helpers/a.go"), "{rows:#?}");
    press(&mut app, KeyCode::Esc);
    assert_eq!(bracket_order(&mut app), git_order);
    // And back.
    to_list(&mut app);
    keys(&mut app, "`");
    assert!(app.files_tree());
    assert_eq!(diff_order(&app), TREE);
}

#[test]
fn backtick_keeps_the_cursor_on_its_file() {
    let f = fixture();
    let mut app = f.open("");
    keys(&mut app, "gg");
    let d = app.commit_diff().unwrap().clone();
    let path =
        |a: &App| debrief::app::commit::file_path(&d.files[a.current_file().unwrap()]).to_string();
    let mut seen = Vec::new();
    for _ in 0..3 {
        keys(&mut app, "]f");
        seen.push(path(&app));
    }
    assert_eq!(path(&app), "src/main.rs", "{seen:?}");
    to_list(&mut app);
    keys(&mut app, "`");
    assert_eq!(path(&app), "src/main.rs");
    assert_eq!(app.sidebar_selected(), 3, "the selection stays on main.rs");
}

#[test]
fn enter_h_l_collapse_and_expand() {
    let f = fixture();
    let mut app = f.open("");
    to_list(&mut app);
    // Row 2 is src: Enter closes it, its three rows go.
    keys(&mut app, "jj");
    assert_eq!(app.sidebar_selected(), 2);
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.focus(), Focus::Files, "Enter on a directory stays");
    assert_eq!(list(&mut app).len(), 5);
    assert!(list(&mut app)[2].contains("▸ src"), "{:#?}", list(&mut app));
    // l opens it; l on the dir again does nothing more; j to app, h closes app.
    keys(&mut app, "l");
    assert_eq!(list(&mut app).len(), 8);
    keys(&mut app, "jh");
    assert_eq!(list(&mut app).len(), 7);
    assert!(list(&mut app)[3].contains("▸ app"));
    // h on a closed dir goes to its parent; h on src closes it.
    keys(&mut app, "h");
    assert_eq!(app.sidebar_selected(), 2);
    keys(&mut app, "h");
    assert_eq!(list(&mut app).len(), 5);
    // h on a file goes to its parent.
    keys(&mut app, "=");
    assert_eq!(list(&mut app).len(), 8);
    keys(&mut app, "jj");
    assert_eq!(app.sidebar_selected(), 4, "x.rs");
    keys(&mut app, "h");
    assert_eq!(app.sidebar_selected(), 3, "app");
    // - closes everything; the top-level rows stay.
    keys(&mut app, "-");
    let rows = list(&mut app);
    assert_eq!(rows.len(), 4, "{rows:#?}");
    assert_eq!(app.sidebar_selected(), 1, "the selection moved to src");
    // l on a file jumps to it and focuses the diff.
    keys(&mut app, "G");
    keys(&mut app, "l");
    assert_eq!(app.focus(), Focus::Diff);
    let d = app.commit_diff().unwrap();
    let cur = app.current_file().unwrap();
    assert_eq!(debrief::app::commit::file_path(&d.files[cur]), "z.txt");
}

#[test]
fn the_marker_goes_on_the_collapsed_dir_hiding_the_current_file() {
    let f = fixture();
    let mut app = f.open("");
    // The diff cursor in src/app/x.rs.
    keys(&mut app, "gg]f]f");
    to_list(&mut app);
    keys(&mut app, "-");
    press(&mut app, KeyCode::Esc);
    let rows = list(&mut app);
    assert!(rows[1].starts_with("▎▸ src"), "{rows:#?}");
    assert_eq!(app.file_list().unwrap().current, Some(1));
}

#[test]
fn moves_cover_every_visible_row() {
    let f = fixture();
    let mut app = f.open("");
    to_list(&mut app);
    keys(&mut app, "G");
    assert_eq!(app.sidebar_selected(), 7);
    ctrl(&mut app, 'u');
    assert!(app.sidebar_selected() < 7);
    keys(&mut app, "gg");
    assert_eq!(app.sidebar_selected(), 0);
    ctrl(&mut app, 'd');
    assert!(app.sidebar_selected() > 0);
}

#[test]
fn the_auto_width_fits_names_not_paths() {
    let f = fixture();
    let tree = f.open("[view]\nsidebar_min_width = 1\n");
    let flat = f.open("[view]\nsidebar_min_width = 1\nfiles = \"flat\"\n");
    let w = |a: &App| {
        a.commit_layout(Rect::new(0, 0, 120, ROWS - 1))
            .sidebar
            .unwrap()
            .width
    };
    // The widest tree row: `▎    ▾ app +2 −0` / `▎▾ pkg/gui/helpers +1 −0`
    // (1 + 2 + 15 + 6 = 24), vs flat `▎A pkg/gui/helpers/a.go +1 −0` (29).
    assert_eq!(w(&tree), 24 + 1 + 1, "fit + padding + border");
    assert_eq!(w(&flat), 29 + 1 + 1);
}

#[test]
fn long_names_are_clipped_in_the_middle() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir(&root).unwrap();
    git(&root, &["init", "-q"]);
    std::fs::write(root.join("a"), "a\n").unwrap();
    commit(&root, "base");
    std::fs::create_dir(root.join("d")).unwrap();
    std::fs::write(
        root.join("d/an_extremely_long_module_name_for_test.rs"),
        "x\n",
    )
    .unwrap();
    commit(&root, "long");
    let mut opts = StartOptions::new(tmp.path().join("cache"));
    opts.config = Config::parse("[view]\nsidebar_width = 24\n").unwrap();
    let mut app = App::new(
        vcs::open(&root.canonicalize().unwrap(), vcs::OpenOptions::default()).unwrap(),
        opts,
    )
    .unwrap();
    app.event(AppEvent::Resize(120, ROWS));
    keys(&mut app, "gg");
    press(&mut app, KeyCode::Enter);
    let rows = list(&mut app);
    let file = &rows[1];
    assert!(
        file.contains('…') && file.contains("est.rs +1 −0"),
        "{rows:#?}"
    );
}

#[test]
fn view_files_flat_starts_flat() {
    let f = fixture();
    let app = f.open("[view]\nfiles = \"flat\"\n");
    assert!(!app.files_tree());
    assert_eq!(diff_order(&app)[0], "b.txt");
    assert!(Config::parse("[view]\nfiles = \"list\"\n").is_err());
}

#[test]
fn sessions_keep_tree_or_flat_and_the_collapsed_dirs() {
    let f = fixture();
    let mut app = f.open("");
    to_list(&mut app);
    keys(&mut app, "jj");
    press(&mut app, KeyCode::Enter);
    keys(&mut app, " ss");
    let top = app.commit_id().unwrap().to_string();
    let s = f.store().load(&top).unwrap().unwrap();
    assert_eq!(s.view.collapsed, ["src"]);
    assert_eq!(s.view.files, Some(ViewFiles::Tree));
    let mut app = f.open("");
    assert_eq!(list(&mut app).len(), 5, "src closed again");
    // Flat is restored over the config's tree.
    let mut s = f.store().load(&top).unwrap().unwrap();
    s.view.files = Some(ViewFiles::Flat);
    f.store().save(&s).unwrap();
    let app = f.open("");
    assert!(!app.files_tree());
}

#[test]
fn old_sessions_open_expanded_in_the_config_mode() {
    let f = fixture();
    let mut app = f.open("");
    keys(&mut app, " ss");
    let top = app.commit_id().unwrap().to_string();
    // A session saved before: no `files`, no `collapsed`.
    let path = f.store().path(&top).unwrap();
    let mut json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let view = json["view"].as_object_mut().unwrap();
    view.remove("files");
    view.remove("collapsed");
    std::fs::write(&path, json.to_string()).unwrap();
    let mut app = f.open("[view]\nfiles = \"flat\"\n");
    assert!(!app.files_tree());
    assert_eq!(list(&mut app).len(), 5);
    let mut app = f.open("");
    assert!(app.files_tree());
    assert_eq!(list(&mut app).len(), 8, "every directory open");
}
