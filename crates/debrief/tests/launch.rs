//! `<leader>o` (the editor at the working-copy file and line) and the
//! `.md` hand-off to ramble. The editor is a script that records its argv,
//! injected through `$VISUAL`; effects run without a terminal.

mod common;

use std::path::PathBuf;

use common::{commit, git};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use debrief::app::config::Config;
use debrief::app::{App, Effect, RowKind, Screen, StartOptions};
use debrief::vcs::{self, LineKind};

struct Fixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    cache: PathBuf,
    script: PathBuf,
    out: PathBuf,
}

/// `base` then `change`: e.txt drops line 5 and edits line 8, d.txt is
/// deleted, gone.txt added (then removed from the working copy), notes.md
/// added, tail.txt loses its last two lines.
fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir(&root).unwrap();
    git(&root, &["init", "-q"]);
    let lines: Vec<String> = (1..=10).map(|i| format!("e{i}")).collect();
    std::fs::write(root.join("e.txt"), lines.join("\n") + "\n").unwrap();
    std::fs::write(root.join("d.txt"), "doomed\n").unwrap();
    std::fs::write(root.join("tail.txt"), "t1\nt2\nt3\n").unwrap();
    commit(&root, "base");
    let mut lines = lines;
    lines[7] = "e8 edited".into();
    lines.remove(4);
    std::fs::write(root.join("e.txt"), lines.join("\n") + "\n").unwrap();
    std::fs::remove_file(root.join("d.txt")).unwrap();
    std::fs::write(root.join("gone.txt"), "g\n").unwrap();
    std::fs::write(root.join("notes.md"), "# notes\n").unwrap();
    std::fs::write(root.join("tail.txt"), "t1\n").unwrap();
    commit(&root, "change\n\nbody");
    std::fs::remove_file(root.join("gone.txt")).unwrap();
    let root = root.canonicalize().unwrap();
    let out = tmp.path().join("argv");
    let script = tmp.path().join("fake-editor");
    std::fs::write(
        &script,
        format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\n", out.display()),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    Fixture {
        cache: tmp.path().join("cache"),
        _tmp: tmp,
        root,
        script,
        out,
    }
}

impl Fixture {
    fn app_with(&self, config: Config) -> App {
        let opts = StartOptions {
            config,
            ..StartOptions::new(self.cache.clone())
        };
        let script = self.script.display().to_string();
        App::new(
            vcs::open(&self.root, vcs::OpenOptions::default()).unwrap(),
            opts,
        )
        .unwrap()
        .with_env(move |k| (k == "VISUAL").then(|| format!("{script} --wait")))
    }

    fn app(&self) -> App {
        self.app_with(Config::defaults())
    }

    /// The app with `change` open.
    fn open(&self) -> App {
        let mut app = self.app();
        keys(&mut app, "gg");
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.screen(), Screen::Commit(0));
        app
    }

    /// Run the queued effect and return the arguments the script saw.
    fn run(&self, app: &mut App) -> Vec<String> {
        let _ = std::fs::remove_file(&self.out);
        assert!(
            app.pending_effect().is_some(),
            "nothing queued: {}",
            app.status()
        );
        app.run_pending_effect();
        std::fs::read_to_string(&self.out)
            .unwrap_or_else(|_| panic!("script not run: {}", app.status()))
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn edit_argv(&self, app: &mut App) -> Vec<String> {
        keys(app, " o");
        self.run(app)
    }

    fn file(&self, name: &str) -> String {
        self.root.join(name).display().to_string()
    }
}

fn press(app: &mut App, code: KeyCode) {
    app.key(KeyEvent::new(code, KeyModifiers::NONE));
}

fn keys(app: &mut App, s: &str) {
    s.chars().for_each(|c| press(app, KeyCode::Char(c)));
}

/// Put the cursor on the first row matching `pred` (via `j`).
fn goto(app: &mut App, pred: impl Fn(&App, usize) -> bool) {
    let i = (0..app.commit_rows().len())
        .find(|&i| pred(app, i))
        .expect("no such row");
    keys(app, "gg");
    for _ in 0..i {
        keys(app, "j");
    }
    assert_eq!(app.commit_cursor().row, i);
}

fn text_row(text: &'static str) -> impl Fn(&App, usize) -> bool {
    move |a, i| a.commit_rows()[i].text == text
}

fn header(path: &'static str) -> impl Fn(&App, usize) -> bool {
    move |a, i| {
        let r = &a.commit_rows()[i];
        r.kind == RowKind::FileHeader && r.text.contains(path)
    }
}

fn expect(f: &Fixture, file: &str, line: u32) -> Vec<String> {
    vec!["--wait".into(), format!("+{line}"), f.file(file)]
}

#[test]
fn added_line_opens_its_new_line() {
    let f = fixture();
    let mut app = f.open();
    goto(&mut app, |a, i| {
        a.commit_rows()[i].text == "e8 edited"
            && a.commit_rows()[i].kind == RowKind::Line(LineKind::Added)
    });
    assert_eq!(f.edit_argv(&mut app), expect(&f, "e.txt", 7));
}

#[test]
fn removed_line_opens_the_neighbouring_new_line() {
    let f = fixture();
    let mut app = f.open();
    goto(&mut app, text_row("e5"));
    assert_eq!(
        app.commit_rows()[app.commit_cursor().row].kind,
        RowKind::Line(LineKind::Removed)
    );
    // Line 6 now sits where line 5 was.
    assert_eq!(f.edit_argv(&mut app), expect(&f, "e.txt", 5));
    // A removal at the end of a file: the line after the last new one.
    goto(&mut app, text_row("t3"));
    assert_eq!(f.edit_argv(&mut app), expect(&f, "tail.txt", 2));
}

#[test]
fn hunk_and_file_headers() {
    let f = fixture();
    let mut app = f.open();
    goto(&mut app, |a, i| {
        let r = &a.commit_rows()[i];
        r.kind == RowKind::HunkHeader
            && a.commit_diff().unwrap().files[r.file.unwrap()]
                .new_path
                .as_deref()
                == Some("e.txt")
    });
    let r = &app.commit_rows()[app.commit_cursor().row];
    let start = app.commit_diff().unwrap().files[r.file.unwrap()].hunks[r.hunk.unwrap()].new_start;
    assert_eq!(f.edit_argv(&mut app), expect(&f, "e.txt", start));
    goto(&mut app, header("e.txt"));
    assert_eq!(f.edit_argv(&mut app), expect(&f, "e.txt", 1));
}

#[test]
fn refusals_run_nothing() {
    let f = fixture();
    let mut app = f.open();
    goto(&mut app, header("d.txt"));
    keys(&mut app, " o");
    assert_eq!(app.pending_effect(), None);
    assert_eq!(app.status(), "d.txt isn't in the working copy");
    goto(&mut app, header("gone.txt"));
    keys(&mut app, " o");
    assert_eq!(app.pending_effect(), None);
    assert_eq!(app.status(), "gone.txt isn't in the working copy");
    keys(&mut app, "gg");
    assert_eq!(app.commit_rows()[0].kind, RowKind::Message);
    keys(&mut app, " o");
    assert_eq!(app.pending_effect(), None);
    assert_eq!(app.status(), "No file here");
}

#[test]
fn cursor_and_screen_stay_after_the_editor() {
    let f = fixture();
    let mut app = f.open();
    goto(&mut app, text_row("e8 edited"));
    keys(&mut app, "ll");
    let (cur, scroll, screen) = (app.commit_cursor(), app.commit_scroll(), app.screen());
    let id = app.commit_id().unwrap().to_string();
    f.edit_argv(&mut app);
    assert_eq!(app.screen(), screen);
    assert_eq!(app.commit_id(), Some(id.as_str()));
    assert_eq!(app.commit_cursor(), cur);
    assert_eq!(app.commit_scroll(), scroll);
    assert_eq!(app.pending_effect(), None);
}

#[test]
fn log_opens_the_first_changed_file_at_its_first_hunk() {
    let f = fixture();
    let mut app = f.app();
    keys(&mut app, "gg");
    assert_eq!(app.screen(), Screen::Log);
    // d.txt is deleted, so e.txt; its first hunk starts at context line 2.
    assert_eq!(f.edit_argv(&mut app), expect(&f, "e.txt", 2));
    assert_eq!(app.screen(), Screen::Log);
}

#[test]
fn log_on_an_empty_commit_says_so() {
    let f = fixture();
    git(&f.root, &["commit", "-q", "--allow-empty", "-m", "empty"]);
    let mut app = f.app();
    keys(&mut app, "gg");
    keys(&mut app, " o");
    assert_eq!(app.pending_effect(), None);
    assert_eq!(app.status(), "No files in this commit");
}

#[test]
fn launchers_are_configurable() {
    let f = fixture();
    let mut c = Config::parse(
        "[[launch]]\nname = \"edit\"\nkey = \"<leader>x\"\ncommand = [\"${editor}\", \"${file}:${line}\"]\n",
    )
    .unwrap();
    c.launch.retain(|l| l.name == "edit");
    let mut app = f.app_with(c);
    keys(&mut app, "gg");
    press(&mut app, KeyCode::Enter);
    goto(&mut app, text_row("e8 edited"));
    keys(&mut app, " o");
    assert_eq!(app.pending_effect(), None, "the old key is gone");
    keys(&mut app, " x");
    let argv = f.run(&mut app);
    assert_eq!(argv[1], format!("{}:7", f.file("e.txt")));
}

/// Configured launcher keys win over built-in `<leader>` keys (ramble's
/// order): `rx` and `rrx` are reachable, and `ex` beats the commit
/// screen's `<leader>e` because a launcher prefix waits.
#[test]
fn configured_launcher_keys_win_over_built_ins() {
    let f = fixture();
    let launcher = |name: &str, key: &str| {
        format!(
            "[[launch]]\nname = \"{name}\"\nkey = \"{key}\"\ncommand = [\"${{editor}}\", \"{name}\"]\n"
        )
    };
    let toml = ["rx", "rrx", "ex"]
        .map(|k| launcher(k, &format!("<leader>{k}")))
        .concat();
    let mut c = Config::parse(&toml).unwrap();
    c.launch
        .retain(|l| ["rx", "rrx", "ex"].contains(&l.name.as_str()));
    let mut app = f.app_with(c);
    for (screen, open) in [("log", false), ("commit", true)] {
        if open {
            keys(&mut app, "gg");
            press(&mut app, KeyCode::Enter);
            assert_eq!(app.screen(), Screen::Commit(0));
        }
        for k in ["rx", "rrx", "ex"] {
            keys(&mut app, &format!(" {k}"));
            assert!(
                matches!(app.pending_effect(), Some(Effect::Launch(l)) if l.name == k),
                "<leader>{k} on {screen}: {:?} / {}",
                app.pending_effect(),
                app.status()
            );
            assert_eq!(f.run(&mut app).last().map(String::as_str), Some(k));
        }
    }
    // The built-ins still work where no launcher claims the sequence.
    keys(&mut app, " rl");
    assert!(app.picker().is_some(), "<leader>rl still opens the picker");
}

#[test]
fn enter_on_a_md_header_runs_the_md_launcher() {
    let f = fixture();
    let mut c = Config::defaults();
    let md = c.launch.iter_mut().find(|l| l.name == "md").unwrap();
    md.command = vec![f.script.display().to_string(), "${file}".into()];
    let mut app = f.app_with(c);
    keys(&mut app, "gg");
    press(&mut app, KeyCode::Enter);
    goto(&mut app, header("notes.md"));
    press(&mut app, KeyCode::Enter);
    assert!(matches!(app.pending_effect(), Some(Effect::Launch(l)) if l.name == "md"));
    assert_eq!(f.run(&mut app), [f.file("notes.md")]);
    // Not .md: nothing runs.
    goto(&mut app, header("e.txt"));
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.pending_effect(), None);
}

#[test]
fn md_hand_off_refuses_missing_files_and_other_review_roots() {
    let f = fixture();
    std::fs::remove_file(f.root.join("notes.md")).unwrap();
    let mut app = f.open();
    goto(&mut app, header("notes.md"));
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.pending_effect(), None);
    assert_eq!(app.status(), "Not in the working copy");

    // A git repo nested inside: ramble would use the nested root.
    let nested = f.root.join("sub");
    std::fs::create_dir(&nested).unwrap();
    git(&nested, &["init", "-q"]);
    std::fs::write(nested.join("x.md"), "x\n").unwrap();
    std::fs::write(f.root.join("notes.md"), "# notes\n").unwrap();
    // Track sub/x.md in the outer repo as plain files (not a submodule).
    std::fs::rename(nested.join(".git"), f.root.join("sub-git")).unwrap();
    git(&f.root, &["add", "sub/x.md"]);
    git(&f.root, &["commit", "-q", "-m", "nested md"]);
    std::fs::rename(f.root.join("sub-git"), nested.join(".git")).unwrap();
    let mut app = f.app();
    keys(&mut app, "gg");
    press(&mut app, KeyCode::Enter);
    goto(&mut app, header("sub/x.md"));
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.pending_effect(), None);
    assert_eq!(app.status(), "File belongs to a different review root");
}

#[test]
fn files_pane_opens_the_selected_file_at_line_one() {
    let f = fixture();
    let mut app = f.open();
    let mut t = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
    t.draw(|fr| app.render(fr)).unwrap();
    app.key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL));
    keys(&mut app, "h");
    let names: Vec<_> = app
        .commit_diff()
        .unwrap()
        .files
        .iter()
        .map(|f| f.new_path.clone())
        .collect();
    let i = names
        .iter()
        .position(|n| n.as_deref() == Some("notes.md"))
        .unwrap();
    keys(&mut app, "gg");
    for _ in 0..i {
        keys(&mut app, "j");
    }
    assert_eq!(f.edit_argv(&mut app), expect(&f, "notes.md", 1));
}
