//! `[theme]`: built-in schemes, scheme files, overrides, errors, syntax
//! colours through every highlighting path, and `fill`.

mod common;

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use common::{commit, git, keys, press};
use crossterm::event::KeyCode;
use debrief::app::config::Config;
use debrief::app::{App, AppEvent, Screen, StartOptions};
use debrief::theme::{self, MOCHA, Palette};
use debrief::vcs::{self, OpenOptions};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;

fn c(hex: u32) -> Color {
    Color::Rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

/// Load `src` as the config file in `dir`.
fn load_in(dir: &Path, src: &str) -> anyhow::Result<Config> {
    let path = dir.join("config.toml");
    std::fs::write(&path, src).unwrap();
    Config::load(&path)
}

fn load(src: &str) -> anyhow::Result<Config> {
    load_in(tempfile::tempdir().unwrap().path(), src)
}

fn palette_of(src: &str) -> Palette {
    load(src).unwrap().theme.resolve().unwrap().palette
}

fn err(src: &str) -> String {
    format!("{:#}", load(src).unwrap_err())
}

#[test]
fn mocha_is_todays_colours() {
    let want = [
        (MOCHA.text, 0xcdd6f4),
        (MOCHA.purple, 0xcba6f7),
        (MOCHA.blue, 0x89b4fa),
        (MOCHA.green, 0xa6e3a1),
        (MOCHA.muted, 0x6c7086),
        (MOCHA.red, 0xf38ba8),
        (MOCHA.yellow, 0xf9e2af),
        (MOCHA.orange, 0xfab387),
        (MOCHA.cyan, 0x94e2d5),
        (MOCHA.bg, 0x1e1e2e),
        (MOCHA.bg_dark, 0x181825),
        (MOCHA.surface0, 0x313244),
        (MOCHA.surface1, 0x45475a),
        (MOCHA.surface2, 0x585b70),
        (MOCHA.sky, 0x89dceb),
        (MOCHA.pink, 0xf5c2e7),
        (MOCHA.cursor_bg, 0x313244),
        (MOCHA.selection_bg, 0x585b70),
        (MOCHA.add_bg, 0x394545),
        (MOCHA.del_bg, 0x483346),
        (MOCHA.hunk_bg, 0x322e46),
    ];
    for (got, hex) in want {
        assert_eq!(got, c(hex), "{hex:06x}");
    }
    assert_eq!(MOCHA.hunk_bg, theme::blend(MOCHA.purple, MOCHA.bg, 12));
}

#[test]
fn every_builtin_loads_and_mocha_is_the_default() {
    assert_eq!(theme::BUILTINS.len(), 8);
    for name in theme::BUILTINS {
        let cfg = load(&format!("[theme]\nname = \"{name}\"\n")).unwrap();
        let r = cfg
            .theme
            .resolve()
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(Some(r.palette), theme::builtin(name), "{name}");
        // Building the syntect theme works for the built-in's syntax name.
        theme::Theme::resolve(&cfg.theme).unwrap();
    }
    assert_eq!(theme::builtin("catppuccin-mocha"), Some(MOCHA));
    assert_eq!(palette_of(""), MOCHA);
    assert_eq!(theme::builtin("nope"), None);
}

#[test]
fn gruvbox_light_is_pinned_to_the_spec_table() {
    let p = theme::builtin("gruvbox-light").unwrap();
    let want = [
        (p.bg, 0xfbf1c7),
        (p.bg_dark, 0xf2e5bc),
        (p.surface0, 0xe4dab5),
        (p.surface1, 0xd4ccaa),
        (p.surface2, 0xc5bd9e),
        (p.muted, 0xa59d85),
        (p.text, 0x3c3836),
        (p.red, 0x9d0006),
        (p.orange, 0xaf3a03),
        (p.yellow, 0xb57614),
        (p.green, 0x79740e),
        (p.cyan, 0x427b58),
        (p.sky, 0x427b58),
        (p.blue, 0x076678),
        (p.purple, 0x8f3f71),
        (p.pink, 0x8f3f71),
        // Roles follow the palette.
        (p.cursor_bg, 0xe4dab5),
        (p.selection_bg, 0xc5bd9e),
    ];
    for (got, hex) in want {
        assert_eq!(got, c(hex), "{hex:06x}");
    }
    assert_eq!(p.add_bg, theme::blend(p.green, p.bg, 20));
    assert_eq!(p.del_bg, theme::blend(p.red, p.bg, 20));
    assert_eq!(p.hunk_bg, theme::blend(p.purple, p.bg, 12));
    let r = load("[theme]\nname = \"gruvbox-light\"\n")
        .unwrap()
        .theme
        .resolve()
        .unwrap();
    assert_eq!(r.syntax, "gruvbox-light");
}

#[test]
fn blend_is_integer_arithmetic_per_channel() {
    assert_eq!(MOCHA.add_bg, theme::blend(MOCHA.green, MOCHA.bg, 20));
    assert_eq!(
        theme::blend(c(0xff0000), c(0x000000), 20),
        Color::Rgb(51, 0, 0)
    );
}

#[test]
fn an_override_replaces_one_colour() {
    let p = palette_of("[theme.colors]\nblue = \"#112233\"\n");
    assert_eq!(p.blue, c(0x112233));
    assert_eq!(
        Palette {
            blue: MOCHA.blue,
            ..p
        },
        MOCHA
    );
    // Over another scheme, the rest stays that scheme's.
    let p = palette_of("[theme]\nname = \"catppuccin-latte\"\n[theme.colors]\nred = \"AABBCC\"\n");
    assert_eq!(p.red, c(0xaabbcc));
    assert_eq!(p.blue, theme::builtin("catppuccin-latte").unwrap().blue);
}

#[test]
fn cursor_bg_follows_a_surface0_override_when_unset() {
    let p = palette_of("[theme.colors]\nsurface0 = \"102030\"\n");
    assert_eq!(p.surface0, c(0x102030));
    assert_eq!(p.cursor_bg, c(0x102030));
    assert_eq!(p.selection_bg, MOCHA.surface2, "selection_bg: surface2");
    let p = palette_of("[theme.colors]\nbg = \"#000000\"\ngreen = \"#00ff00\"\n");
    assert_eq!(p.add_bg, theme::blend(c(0x00ff00), c(0x000000), 20));
}

#[test]
fn a_role_override_wins_even_when_it_equals_mocha() {
    let p = palette_of("[theme.colors]\ncursor_bg = \"#585b70\"\n");
    assert_eq!(p.cursor_bg, c(0x585b70));
    assert_eq!(p.surface0, MOCHA.surface0);
    // An explicit Mocha value is kept, so it wins after a palette change.
    let p = palette_of(
        "[theme.colors]\nsurface0 = \"#000000\"\ncursor_bg = \"#313244\"\nselection_bg = \"#ABCDEF\"\n",
    );
    assert_eq!(p.cursor_bg, MOCHA.cursor_bg);
    assert_eq!(p.selection_bg, c(0xabcdef));
    let cfg = load("[theme.colors]\ncursor_bg = \"#313244\"\n").unwrap();
    assert_eq!(cfg.theme.colors.cursor_bg, Some([0x31, 0x32, 0x44]));
}

const BASE16: &str = r##"system: "base16"
name: "Test"
author: "me"
variant: "dark"
palette:
  base00: "#101010"
  base01: "202020"
  base02: "#303030"   # not used
  base03: "#404040"
  base04: "#505050"
  base05: "#e0e0e0"
  base06: "#606060"
  base07: "#707070"
  base08: "#aa0000"
  base09: "#bb5500"
  base0A: "#cccc00"
  base0B: "#00dd00"
  base0C: "#00eeee"
  base0D: "#0000ff"
  base0E: "#ee00ee"
  base0F: "#808080"
"##;

fn base24() -> String {
    format!("{BASE16}  base10: \"#000000\"\n  base15: \"#11aaff\"\n  base17: \"#ff66aa\"\n")
}

fn file_palette(name: &str, text: &str) -> Palette {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(name), text).unwrap();
    let cfg = load_in(dir.path(), &format!("[theme]\nname = \"{name}\"\n")).unwrap();
    cfg.theme.resolve().unwrap().palette
}

#[test]
fn a_base16_file_loads_with_the_spec_mapping_and_the_sky_pink_fallback() {
    let p = file_palette("t.yaml", BASE16);
    assert_eq!(p.bg, c(0x101010));
    assert_eq!(p.bg_dark, c(0x202020));
    assert_eq!(p.text, c(0xe0e0e0));
    assert_eq!(p.red, c(0xaa0000));
    assert_eq!(p.orange, c(0xbb5500));
    assert_eq!(p.yellow, c(0xcccc00));
    assert_eq!(p.green, c(0x00dd00));
    assert_eq!(p.cyan, c(0x00eeee));
    assert_eq!(p.blue, c(0x0000ff));
    assert_eq!(p.purple, c(0xee00ee));
    assert_eq!(p.sky, p.cyan, "no base15: cyan");
    assert_eq!(p.pink, p.purple, "no base17: purple");
    assert_eq!(p.surface0, theme::blend(p.text, p.bg, 12));
    assert_eq!(p.surface1, theme::blend(p.text, p.bg, 20));
    assert_eq!(p.surface2, theme::blend(p.text, p.bg, 28));
    assert_eq!(p.muted, theme::blend(p.text, p.bg, 45));
    assert_eq!(p.cursor_bg, p.surface0);
}

#[test]
fn a_base24_file_uses_base15_and_base17() {
    let p = file_palette("t.yml", &base24());
    assert_eq!(p.sky, c(0x11aaff));
    assert_eq!(p.pink, c(0xff66aa));
    assert_eq!(p.cyan, c(0x00eeee));
}

#[test]
fn a_scheme_file_s_syntax_follows_its_variant() {
    let syntax = |text: &str| {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("t.yaml"), text).unwrap();
        let cfg = load_in(dir.path(), "[theme]\nname = \"t.yaml\"\n").unwrap();
        cfg.theme.resolve().unwrap().syntax
    };
    assert_eq!(syntax(BASE16), "Catppuccin Mocha");
    assert_eq!(
        syntax(&BASE16.replace("variant: \"dark\"\n", "")),
        "Catppuccin Mocha"
    );
    assert_eq!(
        syntax(&BASE16.replace("\"dark\"", "\"light\"")),
        "Catppuccin Latte"
    );
}

#[test]
fn scheme_paths_resolve_against_the_config_dir() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("schemes")).unwrap();
    std::fs::write(dir.path().join("schemes/t.yaml"), BASE16).unwrap();
    let cfg = load_in(dir.path(), "[theme]\nname = \"schemes/t.yaml\"\n").unwrap();
    assert_eq!(cfg.theme.resolve().unwrap().palette.bg, c(0x101010));
    // An absolute path is used as it is.
    let abs = dir.path().join("schemes/t.yaml");
    let cfg = load_in(
        tempfile::tempdir().unwrap().path(),
        &format!("[theme]\nname = \"{}\"\n", abs.display()),
    )
    .unwrap();
    assert_eq!(cfg.theme.resolve().unwrap().palette.bg, c(0x101010));
    // A missing file is an error naming it.
    let e = err("[theme]\nname = \"nope/x.yaml\"\n");
    assert!(e.contains("nope/x.yaml") && e.contains("line 2"), "{e}");
}

#[test]
fn a_scheme_file_missing_a_slot_or_with_a_bad_colour_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("t.yaml"),
        BASE16.replace("  base0D: \"#0000ff\"\n", ""),
    )
    .unwrap();
    let e = format!(
        "{:#}",
        load_in(dir.path(), "[theme]\nname = \"t.yaml\"\n").unwrap_err()
    );
    assert!(e.contains("missing base0D") && e.contains("t.yaml"), "{e}");
    assert!(theme::parse_scheme(&BASE16.replace("#0000ff", "#00zzff"), "f").is_err());
}

#[test]
fn bad_values_give_errors_naming_the_key_or_value() {
    let e = err("[theme.colors]\nblue = \"#12345\"\n");
    assert!(
        e.contains("theme.colors.blue") && e.contains("line 2"),
        "{e}"
    );
    assert!(e.contains("#12345"), "{e}");
    let e = err("[theme.colors]\nbrown = \"#123456\"\n");
    assert!(
        e.contains("brown") && e.contains("line 2") && e.contains("cursor_bg"),
        "{e}"
    );
    let e = err("[theme]\nname = \"gruvbox\"\n");
    assert!(e.contains("\"gruvbox\"") && e.contains("line 2"), "{e}");
    assert!(
        e.contains("catppuccin-mocha") && e.contains("gruvbox-light"),
        "{e}"
    );
    let e = err("[theme]\nsyntax = \"Nope\"\n");
    assert!(e.contains("\"Nope\"") && e.contains("line 2"), "{e}");
    assert!(e.contains("Dracula") && e.contains("Nord"), "{e}");
    let e = err("[theme]\ncolour = 1\n");
    assert!(e.contains("colour"), "{e}");
}

#[test]
fn syntax_is_matched_without_regard_to_case_and_overrides_the_default() {
    let cfg = load("[theme]\nname = \"gruvbox-dark\"\nsyntax = \"dRaCuLa\"\n").unwrap();
    assert_eq!(cfg.theme.resolve().unwrap().syntax, "Dracula");
    assert_eq!(
        theme::Theme::resolve(&cfg.theme)
            .unwrap()
            .syntax
            .name
            .as_deref(),
        Some("Dracula")
    );
}

// ---- syntax colours and fill, through the app ----

const SIZE: (u16, u16) = (100, 30);

struct Repo {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    cache: PathBuf,
}

/// Two commits touching `a.rs`, then an uncommitted edit of it.
fn repo() -> Repo {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir(&root).unwrap();
    git(&root, &["init", "-q"]);
    std::fs::write(root.join("a.rs"), "fn one() {}\n").unwrap();
    commit(&root, "base");
    std::fs::write(root.join("a.rs"), "fn one() { let x = \"s\"; }\n").unwrap();
    commit(&root, "second");
    std::fs::write(root.join("a.rs"), "fn one() { let y = 2; }\n").unwrap();
    Repo {
        root: root.canonicalize().unwrap(),
        cache: tmp.path().join("cache"),
        _tmp: tmp,
    }
}

impl Repo {
    fn app(&self, toml: &str) -> App {
        let mut opts = StartOptions::new(self.cache.clone());
        opts.config = Config::parse(toml).unwrap();
        opts.config.view.header = false;
        let mut app =
            App::new(vcs::open(&self.root, OpenOptions::default()).unwrap(), opts).unwrap();
        app.event(AppEvent::Resize(SIZE.0, SIZE.1));
        app
    }
}

fn draw(app: &mut App) -> Buffer {
    let mut t = Terminal::new(TestBackend::new(SIZE.0, SIZE.1)).unwrap();
    t.draw(|f| app.render(f)).unwrap();
    t.backend().buffer().clone()
}

fn row_text(buf: &Buffer, y: u16) -> String {
    (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect()
}

/// The colour of the `fn` keyword on the screen row containing `needle`.
fn keyword_fg(app: &mut App, needle: &str) -> Option<Color> {
    let buf = draw(app);
    let y = (0..buf.area.height).find(|&y| row_text(&buf, y).contains(needle))?;
    let s = row_text(&buf, y);
    let x = s[..s.find("fn one").unwrap()].chars().count() as u16;
    buf[(x, y)].style().fg
}

/// Log row `i` open (the working copy is row 0).
fn open_row(app: &mut App, i: usize) {
    keys(app, "gg");
    if i > 0 {
        keys(app, &format!("{i}j"));
    }
    press(app, KeyCode::Enter);
    assert!(matches!(app.screen(), Screen::Commit(_)));
}

#[test]
fn syntax_changes_a_commit_diff_s_token_colours() {
    let r = repo();
    let fg = |toml: &str| {
        let mut app = r.app(toml);
        open_row(&mut app, 1);
        keyword_fg(&mut app, "let x").expect("a coloured keyword")
    };
    let mocha = fg("");
    let gruvbox = fg("[theme]\nsyntax = \"gruvbox-dark\"\n");
    assert_ne!(mocha, gruvbox);
    // The built-in's own syntax follows `name`.
    assert_eq!(fg("[theme]\nname = \"gruvbox-dark\"\n"), gruvbox);
}

#[test]
fn syntax_changes_the_working_copy_diff_s_token_colours() {
    let r = repo();
    let fg = |toml: &str| {
        let mut app = r.app(toml);
        open_row(&mut app, 0);
        assert!(app.viewing_working_copy());
        keyword_fg(&mut app, "let y").expect("a coloured keyword")
    };
    assert_ne!(fg(""), fg("[theme]\nsyntax = \"gruvbox-dark\"\n"));
}

/// Feed the events the worker threads send until they go quiet.
fn settle(app: &mut App, rx: &mpsc::Receiver<AppEvent>) {
    while let Ok(ev) = rx.recv_timeout(Duration::from_secs(3)) {
        app.event(ev);
    }
}

#[test]
fn syntax_reaches_the_worker_threads_too() {
    let r = repo();
    for (row, needle) in [(1, "let x"), (0, "let y")] {
        let fg = |toml: &str| {
            let mut app = r.app(toml);
            let (tx, rx) = mpsc::channel();
            app.set_sender(tx);
            open_row(&mut app, row);
            settle(&mut app, &rx);
            keyword_fg(&mut app, needle).expect("a coloured keyword")
        };
        assert_ne!(
            fg(""),
            fg("[theme]\nsyntax = \"gruvbox-dark\"\n"),
            "{needle}"
        );
    }
}

fn layout(app: &App) -> (Rect, Rect, Option<Rect>) {
    let l = app.commit_layout(Rect::new(0, 0, SIZE.0, SIZE.1 - 1));
    (l.diff, l.sidebar.expect("the file list shows"), l.commits)
}

#[test]
fn fill_paints_bg_under_the_diff_the_file_list_and_the_commit_list() {
    let r = repo();
    let toml = "[theme]\nname = \"solarized-light\"\nfill = true\n[view]\nsidebar_commits = true\n";
    let p = theme::builtin("solarized-light").unwrap();
    let mut app = r.app(toml);
    open_row(&mut app, 1);
    let buf = draw(&mut app);
    let (diff, side, commits) = layout(&app);
    let commits = commits.expect("the commit list shows");
    // A blank cell at the bottom right of each area.
    for (what, rect) in [("diff", diff), ("files", side), ("commits", commits)] {
        let (x, y) = (rect.right() - 2, rect.bottom() - 1);
        assert_eq!(buf[(x, y)].symbol(), " ", "{what}");
        assert_eq!(buf[(x, y)].bg, p.bg, "{what}");
        assert_eq!(buf[(x, y)].fg, p.text, "{what}");
    }
}

#[test]
fn without_fill_those_cells_have_no_background() {
    let r = repo();
    let toml = "[theme]\nname = \"solarized-light\"\n[view]\nsidebar_commits = true\n";
    let mut app = r.app(toml);
    open_row(&mut app, 1);
    let buf = draw(&mut app);
    let (diff, side, commits) = layout(&app);
    for (what, rect) in [
        ("diff", diff),
        ("files", side),
        ("commits", commits.unwrap()),
    ] {
        let (x, y) = (rect.right() - 2, rect.bottom() - 1);
        assert_eq!(buf[(x, y)].bg, Color::Reset, "{what}");
    }
}

#[test]
fn the_palette_changes_what_the_diff_is_drawn_in() {
    let r = repo();
    let mut app = r.app("[theme]\nname = \"gruvbox-dark\"\n");
    open_row(&mut app, 1);
    let buf = draw(&mut app);
    let p = theme::builtin("gruvbox-dark").unwrap();
    let y = (0..SIZE.1)
        .find(|&y| row_text(&buf, y).contains("let x"))
        .unwrap();
    let x = row_text(&buf, y).find("let x").unwrap() as u16;
    assert_eq!(buf[(x, y)].bg, p.add_bg);
    assert_eq!(buf[(0, SIZE.1 - 1)].bg, p.bg_dark, "the status line");
}

#[test]
fn the_default_file_has_a_theme_section_with_every_key() {
    let src = debrief::app::config::DEFAULT_TOML;
    for key in theme::KEYS {
        let line = format!("# {key} = ");
        assert!(src.lines().any(|l| l.starts_with(&line)), "{key}");
    }
    for key in ["name", "syntax", "fill"] {
        assert!(src.contains(&format!("# {key} = ")), "{key}");
    }
}

#[test]
fn a_bad_theme_in_an_app_config_is_an_error_not_a_panic() {
    let mut cfg = Config::default();
    cfg.theme.name = Some("nope".into());
    let mut opts = StartOptions::new(tempfile::tempdir().unwrap().path().join("c"));
    opts.config = cfg;
    let r = repo();
    let e = App::new(vcs::open(&r.root, OpenOptions::default()).unwrap(), opts).err();
    assert!(e.is_some_and(|e| e.to_string().contains("nope")));
}

#[test]
fn non_string_colour_values_name_the_key_and_line() {
    for key in ["blue", "brown"] {
        let e = err(&format!("[theme.colors]\n{key} = 123\n"));
        assert!(
            e.contains(&format!("theme.colors.{key}")) && e.contains("line 2"),
            "{e}"
        );
    }
}

#[test]
fn a_loaded_scheme_file_is_not_read_again_by_the_app() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("t.yaml");
    std::fs::write(&file, BASE16).unwrap();
    let toml = "[theme]\nname = \"t.yaml\"\n[theme.colors]\ncursor_bg = \"#585b70\"\n";
    let cfg = load_in(dir.path(), toml).unwrap();
    std::fs::remove_file(&file).unwrap();

    let r = repo();
    let start = |config: Config| {
        let mut opts = StartOptions::new(r.cache.clone());
        opts.config = config;
        App::new(vcs::open(&r.root, OpenOptions::default()).unwrap(), opts)
    };
    // Validated at load: the deleted file is not needed, and the explicit
    // role override still wins.
    let app = start(cfg.clone()).unwrap();
    assert_eq!(app.theme().palette.bg, c(0x101010));
    assert_eq!(app.theme().palette.cursor_bg, c(0x585b70));

    // A programmatic change to the theme resolves afresh.
    let mut changed = cfg.clone();
    changed.theme.colors.bg = Some([1, 2, 3]);
    let e = start(changed).err().expect("re-resolves, file is gone");
    assert!(e.to_string().contains("t.yaml"), "{e}");
    let mut changed = cfg.clone();
    changed.theme.name = Some("gruvbox-dark".into());
    let app = start(changed).unwrap();
    assert_eq!(
        app.theme().palette.bg,
        theme::builtin("gruvbox-dark").unwrap().bg
    );
}
