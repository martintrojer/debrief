//! The config file: the commented default, `--init-config`, errors, and
//! each toggle's starting state from `[view]` / `[render]` / `[keys]`.

mod common;

use std::path::PathBuf;
use std::process::Command;
use std::time::Instant;

use common::{commit, git};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use debrief::app::config::{self, Config, Layout};
use debrief::app::{App, AppEvent, Screen, StartOptions, TOGGLES};
use debrief::vcs::{self, Context};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

fn manifest() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn default_file() -> String {
    std::fs::read_to_string(manifest().join("config.default.toml")).unwrap()
}

fn err_at(src: &str) -> String {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("c.toml");
    std::fs::write(&p, src).unwrap();
    format!("{:#}", Config::load(&p).unwrap_err())
}

#[test]
fn commented_default_file_is_the_defaults() {
    assert_eq!(config::DEFAULT_TOML, default_file());
    assert_eq!(Config::parse(&default_file()).unwrap(), Config::default());
}

/// The key of a value line (`# <key> = <value>`), if `l` is one.
fn value_key(l: &str) -> Option<&str> {
    let (k, _) = l.strip_prefix("# ")?.split_once(" = ")?;
    let ident = !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    ident.then_some(k)
}

/// The table of a `# [table]` / `# [[table]]` line, if `l` is one.
fn table_header(l: &str) -> Option<&str> {
    let t = l.strip_prefix("# ")?;
    let t = t.strip_prefix("[[").and_then(|t| t.strip_suffix("]]"));
    t.or_else(|| l.strip_prefix("# [")?.strip_suffix(']'))
        .filter(|t| {
            !t.is_empty()
                && t.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
        })
}

/// How many more `[`/`{` than `]`/`}` `s` has, ignoring any inside a quoted
/// string (`"..."` with `\` escapes, or `'...'`).
fn bracket_depth(s: &str) -> i32 {
    let (mut depth, mut quote, mut escaped) = (0, None, false);
    for c in s.chars() {
        match quote {
            Some('"') if escaped => escaped = false,
            Some('"') if c == '\\' => escaped = true,
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None => match c {
                '"' | '\'' => quote = Some(c),
                '[' | '{' => depth += 1,
                ']' | '}' => depth -= 1,
                _ => {}
            },
        }
    }
    depth
}

/// `src` with the `# ` taken off value lines only: `# <key> = `, `# [table]`,
/// `# [[table]]`, and, for a value that opens a `[`/`{` it does not close on
/// its own line (with or without content after the opener), the lines through
/// the one that balances it. Prose (including indented examples) stays
/// commented.
fn uncomment_values(src: &str) -> String {
    let mut out = Vec::new();
    let mut depth = 0;
    for l in src.lines() {
        let rest = l.strip_prefix("# ").unwrap_or(l);
        if depth > 0 {
            depth += bracket_depth(rest);
            out.push(rest);
        } else if value_key(l).is_some() || table_header(l).is_some() {
            depth = rest
                .split_once(" = ")
                .map_or(0, |(_, v)| bracket_depth(v))
                .max(0);
            out.push(rest);
        } else {
            out.push(l);
        }
    }
    out.join("\n")
}

#[test]
fn uncommented_default_file_is_the_defaults() {
    let uncommented = uncomment_values(&default_file());
    assert!(
        !default_file().lines().any(|l| l.starts_with("##")),
        "one comment style: no `##` lines"
    );
    // Every section is really there (no vacuous pass).
    for header in [
        "[view]",
        "[render]",
        "[keys]",
        "[mouse]",
        "[log]",
        "[theme]",
        "[theme.colors]",
        "[review]",
        "[send]",
        "[[launch]]",
    ] {
        assert!(uncommented.lines().any(|l| l == header), "missing {header}");
    }
    assert_eq!(
        uncommented.lines().filter(|l| *l == "[[launch]]").count(),
        2,
        "md and edit"
    );
    // The theme stays unresolved, and the file sets every key (explicitly,
    // even at the Mocha value): everything else equals the defaults, and the
    // theme resolves to the same palette, syntax and fill.
    let mut parsed = Config::parse(&uncommented).unwrap();
    let theme = std::mem::take(&mut parsed.theme);
    assert_eq!(parsed, Config::default());
    assert_eq!(
        theme.resolve().unwrap(),
        Config::default().theme.resolve().unwrap()
    );
}

/// `table.key` is set (commented out) under `# [table]` in the default file.
fn documented(key: &str) -> bool {
    default_keys(&[key.split_once('.').unwrap().0]).contains(&key.to_string())
}

/// The guide's toggle table (docs/guide.md): rows of `| what | key | config key | default |`.
fn guide_rows() -> Vec<Vec<String>> {
    let guide = std::fs::read_to_string(manifest().join("../../docs/guide.md")).unwrap();
    let at = guide
        .find("| Toggle | Key | Config key | Default |")
        .expect("docs/guide.md has the toggle table");
    guide[at..]
        .lines()
        .skip(2)
        .take_while(|l| l.starts_with('|'))
        .map(|l| {
            l.trim_matches('|')
                .split('|')
                // `` ` `` (a backtick key) leaves ` ` ` once its fences go.
                .map(|c| c.trim().trim_matches('`').trim().to_string())
                .collect()
        })
        .collect()
}

/// The `Toggle*` variants of `keys::Action`, read from the source.
fn toggle_actions() -> Vec<String> {
    let src = std::fs::read_to_string(manifest().join("src/app/keys.rs")).unwrap();
    let start = src.find("pub enum Action {").unwrap();
    let end = start + src[start..].find("\n}").unwrap();
    src[start..end]
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with("Toggle"))
        .map(|l| {
            l.trim_end_matches(',')
                .split(['(', ' '])
                .next()
                .unwrap()
                .to_string()
        })
        .collect()
}

#[test]
fn every_toggle_has_a_config_key_and_a_guide_row() {
    let actions = toggle_actions();
    assert!(actions.len() >= 3, "{actions:?}");
    for a in &actions {
        assert!(
            TOGGLES.iter().any(|(t, _, _)| format!("{t:?}") == *a),
            "{a} is not in keys::TOGGLES"
        );
    }
    let rows = guide_rows();
    for (action, key, cfg) in TOGGLES {
        assert!(
            documented(cfg),
            "{cfg} ({action:?}) not in config.default.toml"
        );
        assert!(
            rows.iter().any(|r| r[1] == *key && r[2] == *cfg),
            "no docs/guide.md row for {key} / {cfg}"
        );
    }
    // The config-only rows are documented too.
    for r in &rows {
        assert!(documented(&r[2]), "{} not in config.default.toml", r[2]);
    }
    // And every [view] / [keys] / [mouse] key has a row, key-bound or
    // config-only (`keys.pane_nav` binds C-h/j/k/l but no key flips it, so
    // it is a config-only row rather than a `keys::TOGGLES` entry).
    let keys = default_keys(&["view", "keys", "mouse"]);
    assert!(keys.contains(&"keys.pane_nav".to_string()), "{keys:?}");
    assert!(keys.contains(&"mouse.enabled".to_string()), "{keys:?}");
    for k in &keys {
        assert!(
            rows.iter().any(|r| r[2] == *k),
            "no docs/guide.md row for {k}"
        );
    }
}

/// `table.key` for every value line (`# <key> = ...`) under `# [table]` in the
/// default file, for each of `tables`. Prose is never taken for a key.
fn default_keys(tables: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    let mut table = None;
    for l in config::DEFAULT_TOML.lines() {
        if let Some(t) = table_header(l) {
            table = tables.contains(&t).then_some(t);
        } else if let (Some(t), Some(k)) = (table, value_key(l)) {
            out.push(format!("{t}.{k}"));
        }
    }
    out
}

#[test]
fn section_banners_are_78_columns_with_blank_lines_around() {
    let lines: Vec<&str> = config::DEFAULT_TOML.lines().collect();
    let banners: Vec<usize> = (0..lines.len())
        .filter(|&i| lines[i].starts_with("# ── "))
        .collect();
    assert!(banners.len() >= 8, "sections are really there: {banners:?}");
    for i in banners {
        let l = lines[i];
        assert_eq!(l.chars().count(), 78, "line {}: {l}", i + 1);
        assert!(l.ends_with('─'), "line {}: padded with ─", i + 1);
        assert!(i >= 3, "line {}", i + 1);
        assert!(
            lines[i - 1].is_empty() && lines[i - 2].is_empty() && !lines[i - 3].is_empty(),
            "line {}: two blank lines before",
            i + 1
        );
        assert!(
            lines[i + 1].is_empty() && !lines[i + 2].is_empty(),
            "line {}: exactly one blank line after",
            i + 1
        );
    }
}

#[test]
fn init_config_writes_once_then_refuses() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("a").join("config.toml");
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_debrief"))
            .args(["--init-config", "--config"])
            .arg(&p)
            .output()
            .unwrap()
    };
    let out = run();
    assert!(out.status.success(), "{out:?}");
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        p.display().to_string()
    );
    assert_eq!(std::fs::read_to_string(&p).unwrap(), config::DEFAULT_TOML);
    std::fs::write(&p, "keep").unwrap();
    let out = run();
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&out.stderr).trim(),
        format!("debrief: {} already exists", p.display())
    );
    assert_eq!(std::fs::read_to_string(&p).unwrap(), "keep");
}

#[test]
fn init_config_uses_xdg_config_home() {
    let d = tempfile::tempdir().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_debrief"))
        .arg("--init-config")
        .env("XDG_CONFIG_HOME", d.path())
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let p = d.path().join("debrief/config.toml");
    assert_eq!(std::fs::read_to_string(p).unwrap(), config::DEFAULT_TOML);
}

#[test]
fn unknown_keys_name_the_key_and_the_allowed_ones() {
    let e = err_at("[view]\nsidebr = true\n");
    assert!(
        e.contains(" line 2: view.sidebr: unknown field `sidebr`"),
        "{e}"
    );
    assert!(e.contains("`sidebar`, `sidebar_side`"), "{e}");
    let e = err_at("[keys]\nleadr = \",\"\n");
    assert!(e.contains("keys.leadr: unknown field"), "{e}");
    assert!(
        e.contains("expected one of `leader`, `clue`, `pane_nav`"),
        "{e}"
    );
    let e = err_at("[viw]\n");
    assert!(e.contains("line 1: viw: unknown field `viw`"), "{e}");
    assert!(e.contains("`view`, `keys`"), "{e}");
}

#[test]
fn bad_values_name_the_key_and_the_allowed_values() {
    for (src, want) in [
        (
            "[view]\nlayout = \"side\"\n",
            "line 2: view.layout: unknown variant `side`, expected `unified` or `split`",
        ),
        (
            "[view]\ncontext = \"all\"\n",
            "line 2: view.context: unknown variant `all`, expected `hunks` or `full`",
        ),
        (
            "[view]\nsidebar = 1\n",
            "line 2: view.sidebar: expected \"always\", \"never\" or \"auto\", got 1",
        ),
        (
            "[view]\nsidebar = true\n",
            "line 2: view.sidebar: expected \"always\", \"never\" or \"auto\", got true",
        ),
        (
            "[view]\nsidebar = false\n",
            "line 2: view.sidebar: expected \"always\", \"never\" or \"auto\", got false",
        ),
        (
            "[view]\nsidebar_width = \"wide\"\n",
            "line 2: view.sidebar_width: expected \"auto\" or a number of columns, got \"wide\"",
        ),
        (
            "[view]\nsidebar_side = \"top\"\n",
            "line 2: view.sidebar_side: unknown variant `top`, expected `left` or `right`",
        ),
        (
            "[keys]\npane_nav = 1\n",
            "line 2: keys.pane_nav: invalid type",
        ),
        ("[keys]\nclue = \"no\"\n", "line 2: keys.clue: invalid type"),
        ("[keys]\nleader = \"ab\"\n", "keys.leader: invalid value"),
        ("\n[render]\nhighlight = 0\n", "line 3: render.highlight"),
        // The key comes from the parser, not the text around the error.
        (
            "[view] # preferences\nlayout = \"side\"\n",
            "line 2: view.layout: unknown variant `side`",
        ),
        (
            "view = { layout = \"side\" }\n",
            "line 1: view.layout: unknown variant `side`",
        ),
        (
            "[[launch]]\nname = \"edit\"\ncommand = [\n  1\n]\n",
            "line 4: launch.command: invalid type: integer `1`",
        ),
        // A decoded newline in a value stays escaped: still one line.
        (
            "[view]\nlayout = \"side\\nby side\"\n",
            "line 2: view.layout: unknown variant `side\\nby side`, expected",
        ),
        (
            "[[launch]]\nname = \"a\\r\\nb\"\n",
            "launcher `a\\r\\nb` is missing `command`",
        ),
    ] {
        let e = err_at(src);
        assert!(e.starts_with("config error at "), "{e}");
        assert!(e.contains(want), "{src:?}: {e}");
        assert!(!e.contains('\n'), "one line: {e}");
    }
}

#[test]
fn a_bad_config_exits_2_with_the_message_on_stderr() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("c.toml");
    std::fs::write(&p, "[view]\nlayout = \"side\"\n").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_debrief"))
        .arg("--config")
        .arg(&p)
        .arg(d.path())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let e = String::from_utf8_lossy(&out.stderr);
    assert!(e.starts_with("debrief: config error at "), "{e}");
    assert!(e.contains("view.layout"), "{e}");

    std::fs::write(&p, "[view]\nlayout = \"side\\nby side\"\n").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_debrief"))
        .arg("--config")
        .arg(&p)
        .arg(d.path())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let e = String::from_utf8_lossy(&out.stderr);
    assert_eq!(e.lines().count(), 1, "one line: {e:?}");
    assert!(
        e.contains("view.layout: unknown variant `side\\nby side`"),
        "{e}"
    );
}

// --- starting states, on a real git repo ------------------------------

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
    let body: String = (1..=30).map(|i| format!("line {i}\n")).collect();
    std::fs::write(root.join("x.rs"), &body).unwrap();
    commit(&root, "base");
    std::fs::write(root.join("x.rs"), body.replace("line 15\n", "fn x() {}\n")).unwrap();
    commit(&root, "change");
    Fixture {
        root: root.canonicalize().unwrap(),
        cache: tmp.path().join("cache"),
        _tmp: tmp,
    }
}

impl Fixture {
    fn app(&self, toml: &str, width: u16) -> App {
        let mut opts = StartOptions::new(self.cache.clone());
        opts.config = Config::parse(toml).unwrap();
        // These tests place panes from row 0: no header row.
        opts.config.view.header = false;
        let mut app = App::new(
            vcs::open(&self.root, vcs::OpenOptions::default()).unwrap(),
            opts,
        )
        .unwrap();
        app.event(AppEvent::Resize(width, 30));
        app
    }

    /// `change` open at `width` columns.
    fn open(&self, toml: &str, width: u16) -> App {
        let mut app = self.app(toml, width);
        keys(&mut app, "gg");
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.screen(), Screen::Commit(0));
        app
    }
}

fn press(app: &mut App, code: KeyCode) {
    app.key(KeyEvent::new(code, KeyModifiers::NONE));
}

fn keys(app: &mut App, s: &str) {
    for c in s.chars() {
        press(app, KeyCode::Char(c));
    }
}

fn screen(app: &mut App, w: u16) -> Vec<String> {
    let mut t = Terminal::new(TestBackend::new(w, 30)).unwrap();
    t.draw(|f| app.render(f)).unwrap();
    let buf = t.backend().buffer().clone();
    (0..buf.area.height)
        .map(|y| (0..w).map(|x| buf[(x, y)].symbol()).collect())
        .collect()
}

#[test]
fn view_sidebar_sets_the_file_list_at_start() {
    let f = fixture();
    let mut app = f.open("", 120);
    assert!(app.sidebar_visible());
    let s = screen(&mut app, 120);
    assert!(
        s[0].contains("x.rs") && s[0].contains('│'),
        "file list: {s:#?}"
    );
    let mut app = f.open("[view]\nsidebar = \"never\"\n", 120);
    assert!(!app.sidebar_visible());
    let s = screen(&mut app, 120);
    assert!(!s.iter().any(|l| l.contains('│')), "no file list: {s:#?}");
    keys(&mut app, " e");
    assert!(app.sidebar_visible(), "<leader>e shows it");
}

#[test]
fn view_stats_sets_the_stats_pane_at_start() {
    let f = fixture();
    let mut app = f.app("", 120);
    assert!(app.stats_visible());
    assert!(
        screen(&mut app, 120)[1].contains('│'),
        "pane beside the log (row 0 is the branch chip)"
    );
    let mut app = f.app("[view]\nstats = false\n", 120);
    assert!(!app.stats_visible());
    assert!(!screen(&mut app, 120).iter().any(|l| l.contains('│')));
    keys(&mut app, " e");
    assert!(app.stats_visible(), "<leader>e shows it");
}

#[test]
fn view_context_sets_hunks_or_the_full_file_at_start() {
    let f = fixture();
    let shows_line_1 = |app: &mut App| screen(app, 120).iter().any(|l| l.contains("line 1 "));
    let mut app = f.open("", 120);
    assert_eq!(app.context(), Context::Default);
    assert!(!shows_line_1(&mut app), "hunks only");
    let mut app = f.open("[view]\ncontext = \"full\"\n", 120);
    assert_eq!(app.context(), Context::Full);
    assert!(shows_line_1(&mut app), "the whole file");
}

#[test]
fn view_layout_split_starts_side_by_side_when_wide_enough() {
    let f = fixture();
    let split = "[view]\nlayout = \"split\"\n";
    let mut app = f.open(split, 200);
    assert!(app.commit_split());
    let s = screen(&mut app, 200);
    let row = s.iter().find(|l| l.contains("fn x()")).unwrap();
    assert!(row.contains("line 15"), "both sides on one row: {row}");
    // 140 is the narrowest diff pane; 120 columns less the list is below it.
    let app = f.open(split, 120);
    assert!(!app.commit_split() && app.split_wanted(), "unified below");
    let app = f.open(&format!("{split}split_min_width = 90\n"), 120);
    assert!(app.commit_split(), "split_min_width lowers the bar");
    let app = f.open("", 200);
    assert!(!app.commit_split(), "unified by default");
    assert_eq!(Config::default().view.layout, Layout::Unified);
}

#[test]
fn render_highlight_sets_syntax_colours_at_start() {
    let f = fixture();
    let fg_of = |app: &mut App, word: &str| {
        let mut t = Terminal::new(TestBackend::new(120, 30)).unwrap();
        t.draw(|fr| app.render(fr)).unwrap();
        let buf = t.backend().buffer().clone();
        let y = (0..30)
            .find(|&y| {
                (0..120)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
                    .contains(word)
            })
            .unwrap();
        let line: Vec<String> = (0..120).map(|x| buf[(x, y)].symbol().to_string()).collect();
        let x = (0..120)
            .find(|&x| line[x..].concat().starts_with(word))
            .unwrap();
        buf[(x as u16, y)].style().fg
    };
    let mut on = f.open("", 120);
    let mut off = f.open("[render]\nhighlight = false\n", 120);
    assert_ne!(fg_of(&mut on, "fn"), fg_of(&mut off, "fn"));
    assert_eq!(
        fg_of(&mut off, "fn"),
        Some(debrief::theme::MOCHA.green),
        "plain added line"
    );
}

#[test]
fn keys_clue_false_never_opens_the_clue() {
    let f = fixture();
    let t0 = Instant::now();
    for (toml, want) in [("", true), ("[keys]\nclue = false\n", false)] {
        let mut app = f.app(toml, 120);
        keys(&mut app, "g");
        app.clue_started_at(t0);
        app.tick(t0 + debrief::app::CLUE_DELAY);
        assert_eq!(app.clue_visible(), want, "{toml:?}");
    }
}

#[test]
fn keys_leader_replaces_space() {
    let f = fixture();
    let mut app = f.app("[keys]\nleader = \",\"\n", 120);
    assert!(app.stats_visible());
    keys(&mut app, " e");
    assert!(app.stats_visible(), "Space is no longer the leader");
    keys(&mut app, ",e");
    assert!(!app.stats_visible(), ",e toggles the stats pane");
    // In a prompt the leader is typed as itself.
    keys(&mut app, "/,");
    press(&mut app, KeyCode::Esc);
    assert!(!app.stats_visible());
}
