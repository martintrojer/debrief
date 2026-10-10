//! Commit-view rendering (spec Q25): tabs, CR and control characters,
//! full-width row backgrounds, sidebar status letters, syntax colours. A fake `Vcs`
//! serves one commit with a hand-built diff.

use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use debrief::app::{App, Screen, StartOptions};
use debrief::ui::palette;
use debrief::vcs::{
    self, CommitDiff, CommitInfo, Context, DiffLine, FileDiff, Hunk, Kind, LineKind, OpenOptions,
    Vcs,
};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::style::Modifier;

struct Fake {
    root: PathBuf,
    diff: CommitDiff,
}

impl Vcs for Fake {
    fn kind(&self) -> Kind {
        Kind::Git
    }
    fn root(&self) -> &Path {
        &self.root
    }
    fn open_options(&self) -> OpenOptions {
        OpenOptions::default()
    }
    fn log(&self, _: Option<&str>, _: usize) -> anyhow::Result<Vec<CommitInfo>> {
        Ok(vec![CommitInfo {
            commit: "c0ffee0000000000000000000000000000000000".into(),
            change: None,
            author: "t".into(),
            time: 0,
            summary: "s".into(),
            parents: vec![],
        }])
    }
    fn show(&self, _: &str, _: Context) -> anyhow::Result<CommitDiff> {
        Ok(self.diff.clone())
    }
    fn stat(&self, _: &str) -> anyhow::Result<vcs::CommitStat> {
        anyhow::bail!("no stat")
    }
}

fn file(old: Option<&str>, new: Option<&str>, lines: Vec<DiffLine>) -> FileDiff {
    let hunks = if lines.is_empty() {
        vec![]
    } else {
        vec![Hunk {
            header: "@@ -1 +1 @@".into(),
            old_start: 1,
            new_start: 1,
            lines,
        }]
    };
    FileDiff {
        old_path: old.map(str::to_string),
        new_path: new.map(str::to_string),
        binary: false,
        hunks,
        note: None,
        mode_change: None,
        copied: false,
    }
}

fn line(kind: LineKind, n: u32, text: &str) -> DiffLine {
    let (old, new) = match kind {
        LineKind::Added => (None, Some(n)),
        LineKind::Removed => (Some(n), None),
        LineKind::Context => (Some(n), Some(n)),
    };
    DiffLine {
        kind,
        old,
        new,
        text: text.into(),
        no_newline: false,
    }
}

/// The app with the fake commit open (no message, so row 0 is the first
/// file header).
fn open(files: Vec<FileDiff>) -> (tempfile::TempDir, App) {
    open_with(files, true)
}

/// [`open`] with `[render] highlight` set to `highlight`.
fn open_with(files: Vec<FileDiff>, highlight: bool) -> (tempfile::TempDir, App) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir(&root).unwrap();
    let fake = Fake {
        root,
        diff: CommitDiff {
            message: String::new(),
            files,
        },
    };
    let mut opts = StartOptions::new(tmp.path().join("cache"));
    opts.config.render.highlight = highlight;
    let mut app = App::new(Box::new(fake), opts).unwrap();
    app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(matches!(app.screen(), Screen::Commit(_)));
    (tmp, app)
}

fn keys(app: &mut App, s: &str) {
    for c in s.chars() {
        app.key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
}

fn draw(app: &mut App, w: u16, h: u16) -> Buffer {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| app.render(f)).unwrap();
    t.backend().buffer().clone()
}

fn text(buf: &Buffer, y: u16) -> String {
    (0..buf.area.width)
        .map(|x| buf[(x, y)].symbol())
        .collect::<String>()
        .trim_end()
        .to_string()
}

/// The screen row whose text contains `needle`.
fn row_of(buf: &Buffer, needle: &str) -> u16 {
    (0..buf.area.height)
        .find(|&y| text(buf, y).contains(needle))
        .unwrap_or_else(|| panic!("{needle:?} not on screen"))
}

/// The first text column of the diff pane (after sidebar and gutter),
/// found from a line whose text starts with `first`.
fn text_x(buf: &Buffer, y: u16, first: &str) -> u16 {
    let s = text(buf, y);
    let byte = s.rfind(first).unwrap();
    s[..byte].chars().count() as u16
}

/// Index of the diff row whose text is `t`.
fn diff_row(app: &App, t: &str) -> usize {
    app.commit_rows().iter().position(|r| r.text == t).unwrap()
}

/// Put the cursor on diff row `r` at column 0.
fn goto(app: &mut App, r: usize) {
    keys(app, "gg");
    if r > 0 {
        keys(app, &format!("{r}j"));
    }
    keys(app, "0");
}

#[test]
fn tabs_render_at_tab_stops_with_cursor_and_search_on_the_right_cells() {
    let (_tmp, mut app) = open(vec![file(
        Some("a.go"),
        Some("a.go"),
        vec![
            line(LineKind::Context, 1, "|x"),
            line(LineKind::Added, 2, "\tfoo\tbar"),
        ],
    )]);
    let r = diff_row(&app, "\tfoo\tbar");
    assert_eq!(
        app.commit_rows()[r].text,
        "\tfoo\tbar",
        "Row.text stays raw"
    );
    let buf = draw(&mut app, 100, 12);
    // Column 0 of the text, from the context line above.
    let x0 = text_x(&buf, row_of(&buf, "|x"), "|x");
    let y = row_of(&buf, "foo");
    let at = |x: u16| buf[(x0 + x, y)].symbol().to_string();
    assert_eq!(
        (0..11).map(at).collect::<String>(),
        "    foo bar",
        "tab to column 4, then from 7 to 8"
    );
    // The cursor walks the cells: l from the tab lands on f (column 4).
    goto(&mut app, r);
    keys(&mut app, "l");
    assert_eq!(app.commit_cursor().col, 4);
    let buf = draw(&mut app, 100, 12);
    assert_eq!(buf[(x0 + 4, y)].symbol(), "f");
    assert!(
        buf[(x0 + 4, y)]
            .style()
            .add_modifier
            .contains(Modifier::REVERSED)
    );
    // w then w: foo -> tab -> bar at column 8.
    keys(&mut app, "w");
    assert_eq!(app.commit_cursor().col, 8);
    // A search for "bar" highlights columns 8..11 and jumps there.
    keys(&mut app, "0/bar");
    app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.search_hits(), vec![(r, 8, 11)]);
    assert_eq!(app.commit_cursor(), debrief::app::Cursor { row: r, col: 8 });
    keys(&mut app, "k");
    let buf = draw(&mut app, 100, 12);
    for x in 8..11 {
        assert_eq!(buf[(x0 + x, y)].style().bg, Some(palette::YELLOW), "{x}");
    }
    assert_ne!(buf[(x0 + 7, y)].style().bg, Some(palette::YELLOW));
}

#[test]
fn a_tab_cut_by_the_viewport_edge_keeps_its_search_hit_and_cursor() {
    // Width 79 leaves 67 text columns; `$` scrolls to 5, inside the tab
    // at 4..8, whose three visible columns stay highlighted.
    let raw = format!("aaaa\t{}", "Z".repeat(64));
    let (_tmp, mut app) = open(vec![file(
        Some("a"),
        Some("a"),
        vec![line(LineKind::Context, 1, &raw)],
    )]);
    draw(&mut app, 79, 12);
    let r = diff_row(&app, &raw);
    goto(&mut app, r);
    keys(&mut app, "/\t");
    app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.search_hits(), vec![(r, 4, 8)]);
    keys(&mut app, "$");
    let buf = draw(&mut app, 79, 12);
    assert_eq!(app.commit_scroll().1, 5);
    let y = row_of(&buf, "ZZZ");
    let first_z = (0..79).find(|&x| buf[(x, y)].symbol() == "Z").unwrap();
    for x in first_z - 3..first_z {
        assert_eq!(buf[(x, y)].style().bg, Some(palette::YELLOW), "{x}");
    }
    assert_ne!(buf[(first_z - 4, y)].style().bg, Some(palette::YELLOW));

    // A pane narrower than the tab: the cursor on it stays visible.
    for width in 13..20 {
        let (_tmp, mut app) = open(vec![file(
            Some("a"),
            Some("a"),
            vec![line(LineKind::Added, 1, "\tX")],
        )]);
        let r = diff_row(&app, "\tX");
        goto(&mut app, r);
        let buf = draw(&mut app, width, 12);
        let reversed = (0..buf.area.height)
            .flat_map(|y| (0..width).map(move |x| (x, y)))
            .filter(|&p| buf[p].style().add_modifier.contains(Modifier::REVERSED))
            .count();
        assert_eq!(reversed, 1, "width {width}");
    }
}

#[test]
fn cr_and_control_characters() {
    let (_tmp, mut app) = open(vec![file(
        Some("a"),
        Some("a"),
        vec![
            line(LineKind::Context, 1, "|x"),
            line(LineKind::Context, 2, "a\tb\rc"),
            line(LineKind::Context, 3, "esc\x1b!"),
            line(LineKind::Context, 4, "crlf\r"),
        ],
    )]);
    let buf = draw(&mut app, 100, 12);
    let x0 = text_x(&buf, row_of(&buf, "|x"), "|x");
    let cells = |y: u16, n: u16| {
        (0..n)
            .map(|x| buf[(x0 + x, y)].symbol())
            .collect::<String>()
    };
    let y = row_of(&buf, "a   b");
    assert_eq!(cells(y, 8), "a   b?c ");
    let q = buf[(x0 + 5, y)].style();
    assert_eq!(q.fg, Some(palette::OVERLAY));
    assert!(q.add_modifier.contains(Modifier::DIM));
    let y = row_of(&buf, "esc");
    assert_eq!(cells(y, 6), "esc?! ");
    assert!(
        buf[(x0 + 3, y)]
            .style()
            .add_modifier
            .contains(Modifier::DIM)
    );
    let y = row_of(&buf, "crlf");
    assert_eq!(cells(y, 6), "crlf  ", "a trailing CR is not drawn");
    // `$` stops on the f, not on an invisible CR cell.
    let r = diff_row(&app, "crlf\r");
    goto(&mut app, r);
    keys(&mut app, "$");
    assert_eq!(app.commit_cursor().col, 3);
    // Row text stays raw for yank and comments.
    assert_eq!(app.commit_rows()[r].text, "crlf\r");
}

#[test]
fn the_cursor_shows_on_a_blank_crlf_line() {
    let (_tmp, mut app) = open(vec![file(
        Some("a"),
        Some("a"),
        vec![line(LineKind::Added, 1, "\r")],
    )]);
    let r = diff_row(&app, "\r");
    goto(&mut app, r);
    let buf = draw(&mut app, 79, 12);
    let reversed = (0..buf.area.height)
        .flat_map(|y| (0..79).map(move |x| (x, y)))
        .filter(|&p| buf[p].style().add_modifier.contains(Modifier::REVERSED))
        .count();
    assert_eq!(reversed, 1);
}

#[test]
fn backgrounds_fill_the_pane_and_the_sign_is_coloured() {
    let (_tmp, mut app) = open(vec![file(
        Some("a"),
        Some("a"),
        vec![
            line(LineKind::Removed, 1, "x"),
            line(LineKind::Added, 1, "y"),
            line(LineKind::Context, 2, "z"),
        ],
    )]);
    let w = 100;
    let buf = draw(&mut app, w, 12);
    let last = w - 1;
    let del = row_of(&buf, "- x");
    let add = row_of(&buf, "+ y");
    let hunk = row_of(&buf, "@@ -1 +1 @@");
    let ctx = row_of(&buf, "  z");
    let header = row_of(&buf, "── a ──");
    assert_eq!(buf[(last, del)].style().bg, Some(palette::DEL_BG));
    assert_eq!(buf[(last, add)].style().bg, Some(palette::ADD_BG));
    assert_eq!(buf[(last, hunk)].style().bg, Some(palette::HUNK_BG));
    assert_ne!(buf[(last, ctx)].style().bg, Some(palette::ADD_BG));
    // The cursor row (the header at first) is surface to the edge.
    assert_eq!(app.commit_cursor().row, 0);
    assert_eq!(buf[(last, header)].style().bg, Some(palette::SURFACE));
    assert_eq!(
        buf[(last, header)].symbol(),
        "─",
        "the rule runs to the edge"
    );
    // The sign is coloured.
    let sx = |y| {
        text(&buf, y)
            .chars()
            .position(|c| c == '+' || c == '-')
            .unwrap() as u16
    };
    assert_eq!(buf[(sx(add), add)].style().fg, Some(palette::GREEN));
    assert_eq!(buf[(sx(del), del)].style().fg, Some(palette::RED));
    // Moving the cursor onto the short added line paints it surface to
    // the edge.
    let r = diff_row(&app, "y");
    goto(&mut app, r);
    let buf = draw(&mut app, w, 12);
    assert_eq!(buf[(last, add)].style().bg, Some(palette::SURFACE));
    assert_eq!(buf[(last, del)].style().bg, Some(palette::DEL_BG));
}

#[test]
fn sidebar_letters_mode_changes_and_empty_files() {
    let ctx = || vec![line(LineKind::Context, 1, "x")];
    let mut copy = file(Some("orig.rs"), Some("copy.rs"), ctx());
    copy.copied = true;
    let mut mode = file(Some("run.sh"), Some("run.sh"), vec![]);
    mode.mode_change = Some(("100644".into(), "100755".into()));
    let files = vec![
        file(None, Some("added.rs"), ctx()),
        file(Some("mod.rs"), Some("mod.rs"), ctx()),
        file(Some("gone.rs"), None, ctx()),
        file(Some("old.rs"), Some("new.rs"), vec![]),
        copy,
        mode,
    ];
    let (_tmp, mut app) = open(files);
    let buf = draw(&mut app, 100, 40);
    let want = [
        ('A', "added.rs", palette::GREEN),
        ('M', "mod.rs", palette::YELLOW),
        ('D', "gone.rs", palette::RED),
        ('R', "new.rs", palette::PINK),
        ('C', "copy.rs", palette::TEAL),
        ('M', "run.sh", palette::YELLOW),
    ];
    for (y, (letter, path, color)) in want.into_iter().enumerate() {
        let y = y as u16;
        let s = text(&buf, y);
        let after_mark: String = s.chars().skip(1).collect();
        assert!(after_mark.starts_with(&format!("{letter} {path} ")), "{s}");
        assert_eq!(buf[(1, y)].style().fg, Some(color), "{s}");
    }
    let rows: Vec<_> = app.commit_rows().iter().map(|r| r.text.as_str()).collect();
    let i = rows
        .iter()
        .position(|t| t.contains("old.rs → new.rs"))
        .unwrap();
    assert_eq!(rows[i + 1], "(no content changes)");
    let i = rows
        .iter()
        .position(|t| *t == "── run.sh (mode 100644 → 100755) ──")
        .unwrap();
    assert_eq!(rows[i + 1], "(no content changes)");
}

#[test]
fn a_real_mode_change_is_labelled() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir(&root).unwrap();
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .current_dir(&root)
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "maintenance.auto=false",
                "-c",
                "gc.auto=0",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
    };
    git(&["init", "-q"]);
    std::fs::write(root.join("run.sh"), "echo\n").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "a"]);
    git(&["update-index", "--chmod=+x", "run.sh"]);
    git(&["commit", "-q", "-m", "b"]);
    let v = vcs::open(&root, OpenOptions::default()).unwrap();
    let id = v.log(None, 1).unwrap()[0].commit.clone();
    let f = &v.show(&id, Context::Default).unwrap().files[0];
    assert_eq!(
        f.mode_change,
        Some(("100644".to_string(), "100755".to_string()))
    );
    assert!(f.hunks.is_empty());
}

fn rust_file(path: &str) -> FileDiff {
    file(
        Some(path),
        Some(path),
        vec![
            line(LineKind::Removed, 1, "fn old() {}"),
            line(LineKind::Added, 1, "fn new() {}"),
            line(LineKind::Context, 2, "\tlet s = \"a\tb\x1b\";"),
        ],
    )
}

#[test]
fn a_rust_hunk_gets_keyword_colours_under_the_row_backgrounds() {
    let (_tmp, mut app) = open(vec![rust_file("src/a.rs")]);
    let buf = draw(&mut app, 100, 12);
    let del = row_of(&buf, "fn old");
    let add = row_of(&buf, "fn new");
    let (xd, xa) = (text_x(&buf, del, "fn old"), text_x(&buf, add, "fn new"));
    let kw = buf[(xa, add)].style().fg;
    assert!(kw.is_some());
    assert_ne!(kw, Some(palette::GREEN), "a syntax colour, not the row's");
    assert_eq!(buf[(xd, del)].style().fg, kw, "the removed side too");
    assert_ne!(buf[(xa + 3, add)].style().fg, kw, "the name differs");
    assert_eq!(buf[(xa, add)].style().bg, Some(palette::ADD_BG));
    assert_eq!(buf[(xd, del)].style().bg, Some(palette::DEL_BG));

    // t9's display rules stay: a tab's cells share its span's colour; a
    // control character is still a dimmed `?`.
    let y = row_of(&buf, "let s");
    let x0 = text_x(&buf, y, "let s") - 4;
    let tab = buf[(x0, y)].style().fg;
    assert!(tab.is_some());
    for x in 1..4 {
        assert_eq!(buf[(x0 + x, y)].style().fg, tab, "tab cell {x}");
    }
    let s = text(&buf, y);
    let q = s.chars().position(|c| c == '?').unwrap() as u16;
    let qs = buf[(q, y)].style();
    assert_eq!(qs.fg, Some(palette::OVERLAY));
    assert!(qs.add_modifier.contains(Modifier::DIM));
    // The tab inside the string takes the string's colour.
    let chars: Vec<char> = s.chars().collect();
    let a = chars.windows(2).position(|w| w == ['"', 'a']).unwrap() as u16 + 1;
    let b = chars.iter().position(|&c| c == 'b').unwrap() as u16;
    assert!(b > a + 1, "the tab spans cells");
    let string = buf[(a, y)].style().fg;
    assert_ne!(string, tab);
    for x in a..=b {
        assert_eq!(buf[(x, y)].style().fg, string, "string cell {x}");
    }

    // The cursor row keeps its background and reversed cell.
    let r = diff_row(&app, "fn new() {}");
    goto(&mut app, r);
    let buf = draw(&mut app, 100, 12);
    assert_eq!(buf[(xa + 3, add)].style().bg, Some(palette::SURFACE));
    assert!(
        buf[(xa, add)]
            .style()
            .add_modifier
            .contains(Modifier::REVERSED)
    );
}

#[test]
fn highlight_false_and_unknown_extensions_render_plain() {
    for (path, on) in [("src/a.rs", false), ("notes.zzzz", true)] {
        let (_tmp, mut app) = open_with(vec![rust_file(path)], on);
        let buf = draw(&mut app, 100, 12);
        let add = row_of(&buf, "fn new");
        let xa = text_x(&buf, add, "fn new");
        assert_eq!(buf[(xa, add)].style().fg, Some(palette::GREEN), "{path}");
        let del = row_of(&buf, "fn old");
        let xd = text_x(&buf, del, "fn old");
        assert_eq!(buf[(xd, del)].style().fg, Some(palette::RED), "{path}");
    }
}
