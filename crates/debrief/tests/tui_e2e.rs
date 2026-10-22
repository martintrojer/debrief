//! End-to-end: the real binary in an isolated tmux server
//! (`tmux -L debrief-test-<pid>-<n>`).

mod common;

use std::path::Path;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use common::{BUBBLES_FILES, JjEnv, bubbles_repo, commit, git, have_jj, w};

static N: AtomicUsize = AtomicUsize::new(0);

struct Tmux {
    sock: String,
}

impl Tmux {
    fn cmd(&self, args: &[&str]) -> Output {
        Command::new("tmux")
            .args(["-L", &self.sock])
            .args(args)
            .env_remove("TMUX")
            .output()
            .expect("run tmux")
    }

    fn capture(&self) -> String {
        String::from_utf8_lossy(&self.cmd(&["capture-pane", "-p", "-t", "e2e"]).stdout).into()
    }

    fn keys(&self, keys: &[&str]) {
        let mut args = vec!["send-keys", "-t", "e2e"];
        args.extend_from_slice(keys);
        self.cmd(&args);
    }

    /// Poll the pane until `pred` holds, or panic with the last screen.
    fn wait_for(&self, what: &str, pred: impl Fn(&str) -> bool) -> String {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let screen = self.capture();
            if pred(&screen) {
                return screen;
            }
            if Instant::now() > deadline {
                panic!("timed out waiting for {what}; screen:\n{screen}");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Send `q`; wait for the session to end and return the exit status
    /// the pane recorded.
    fn quit(&self, status_file: &Path) -> String {
        self.keys(&["q"]);
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Ok(s) = std::fs::read_to_string(status_file) {
                return s.trim().to_string();
            }
            assert!(Instant::now() < deadline, "debrief did not quit on q");
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for Tmux {
    fn drop(&mut self) {
        let _ = self.cmd(&["kill-server"]);
    }
}

fn quote(p: &Path) -> String {
    format!("'{}'", p.display().to_string().replace('\'', r"'\''"))
}

/// Start the binary on `repo` in a fresh tmux server; the shell writes
/// debrief's exit status to `status_file`. Sessions go to `<cache>/state`,
/// never the real state dir, and `XDG_CONFIG_HOME` is the empty
/// `<cache>/config`, so the user's debrief config (and git's XDG ignore
/// file) never applies; `env` comes after and can override it.
fn start(repo: &Path, cache: &Path, status_file: &Path, env: &str) -> Option<Tmux> {
    start_with(repo, cache, status_file, env, "")
}

fn start_with(
    repo: &Path,
    cache: &Path,
    status_file: &Path,
    env: &str,
    flags: &str,
) -> Option<Tmux> {
    start_in(repo, cache, status_file, env, flags, false)
}

/// [`start_with`]; with `beside`, debrief runs in a second pane split
/// off an idle one (a window of one pane can't zoom), and is the active
/// pane `-t e2e` addresses.
fn start_in(
    repo: &Path,
    cache: &Path,
    status_file: &Path,
    env: &str,
    flags: &str,
    beside: bool,
) -> Option<Tmux> {
    if Command::new("tmux").arg("-V").output().is_err() {
        eprintln!("skipping tui_e2e: tmux not found on PATH");
        return None;
    }
    let tmux = Tmux {
        sock: format!(
            "debrief-test-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ),
    };
    let bin = env!("CARGO_BIN_EXE_debrief");
    let shell_cmd = format!(
        "env XDG_CONFIG_HOME={} {env} {} {} --cache-dir {} --state-dir {} {flags}; echo $? > {}",
        quote(&cache.join("config")),
        quote(Path::new(bin)),
        quote(repo),
        quote(cache),
        quote(&cache.join("state")),
        quote(status_file)
    );
    let first = if beside { "sleep 600" } else { &shell_cmd };
    let out = tmux.cmd(&[
        "-f",
        "/dev/null",
        "new-session",
        "-d",
        "-s",
        "e2e",
        "-x",
        "100",
        "-y",
        "20",
        first,
    ]);
    assert!(out.status.success(), "tmux new-session: {out:?}");
    if beside {
        // The pane outlives debrief: closing it would unzoom the window
        // on its own.
        let held = format!("{shell_cmd}; sleep 600");
        let out = tmux.cmd(&["split-window", "-t", "e2e", &held]);
        assert!(out.status.success(), "tmux split-window: {out:?}");
    }
    Some(tmux)
}

/// The status line's `cursor/total` position.
fn position(screen: &str) -> Option<String> {
    let last = screen.lines().rfind(|l| !l.trim().is_empty())?;
    last.split_whitespace().last().map(str::to_string)
}

#[test]
fn git_log_renders_moves_and_quits() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    std::fs::create_dir(&root).unwrap();
    git(&root, &["init", "-q"]);
    for i in 0..3 {
        std::fs::write(root.join("a.txt"), format!("{i}\n")).unwrap();
        commit(&root, &format!("git commit {i}"));
    }
    let status = dir.path().join("status");
    let Some(tmux) = start(&root, &dir.path().join("cache"), &status, "") else {
        return;
    };
    let screen = tmux.wait_for("the log", |s| s.contains("git commit 0"));
    assert!(screen.contains("git commit 2"), "{screen}");
    assert!(screen.contains("0 new"), "{screen}");
    assert_eq!(position(&screen).as_deref(), Some("1/3"), "{screen}");
    tmux.keys(&["j"]);
    tmux.wait_for("cursor on row 2", |s| position(s).as_deref() == Some("2/3"));
    assert_eq!(tmux.quit(&status), "0");
}

/// Only the real binary shows that folding is on by default, that `l` /
/// `h` reach the fold through the terminal's key decoding, and that the
/// diff on screen is the merge's first-parent one (its bubble's files).
#[test]
fn git_folded_log_opens_a_fold_and_the_merges_diff() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    bubbles_repo(&root);
    let status = dir.path().join("status");
    let Some(tmux) = start(&root, &dir.path().join("cache"), &status, "") else {
        return;
    };
    let octopus = |s: &str| {
        s.lines()
            .find(|l| l.contains("Octopus x y z"))
            .map(str::to_string)
    };
    // Row 0 is the octopus, a closed fold of 3 once its size is in.
    let screen = tmux.wait_for("the folded log", |s| {
        octopus(s).is_some_and(|l| l.contains("▸ +3"))
    });
    assert_eq!(position(&screen).as_deref(), Some("1/6"), "{screen}");
    assert!(!screen.contains("x1"), "{screen}");
    tmux.keys(&["l"]);
    tmux.wait_for("the fold's groups", |s| {
        octopus(s).is_some_and(|l| l.contains("▾ +3"))
            && s.lines().any(|l| l.contains("├") && l.ends_with("x1"))
            && s.lines().any(|l| l.contains("╰") && l.ends_with("z1"))
    });
    tmux.keys(&["h"]);
    tmux.wait_for("the fold closed", |s| !s.contains("x1"));
    tmux.keys(&["Enter"]);
    // The log's stats pane already lists the octopus's files, so wait
    // for what only the commit screen draws: each file's diff header
    // with its hunk.
    let files = BUBBLES_FILES[0].1;
    let screen = tmux.wait_for("the merge's first-parent diff", |s| {
        s.contains("Octopus x y z")
            && files
                .iter()
                .all(|f| s.contains(&format!("── {f} (added) ")))
            && s.matches("@@ -0,0 +1 @@").count() == files.len()
    });
    // The stats pane (under a full-width rule) is gone.
    assert!(
        !screen
            .lines()
            .any(|l| !l.trim().is_empty() && l.trim().chars().all(|c| c == '─')),
        "{screen}"
    );
    tmux.keys(&["q"]);
    tmux.wait_for("the log", |s| {
        octopus(s).is_some_and(|l| l.contains("▸ +3"))
    });
    assert_eq!(tmux.quit(&status), "0");
}

#[test]
fn not_a_repo_exits_2_with_the_error() {
    let dir = tempfile::tempdir().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_debrief"))
        .arg(dir.path())
        .arg("--cache-dir")
        .arg(dir.path().join("cache"))
        .arg("--state-dir")
        .arg(dir.path().join("state"))
        .env("XDG_CONFIG_HOME", dir.path().join("config"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.starts_with("debrief: not a git or jj repository: "),
        "{err}"
    );
}

#[test]
fn send_copies_the_review_with_one_osc52_write() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    std::fs::create_dir(&root).unwrap();
    git(&root, &["init", "-q"]);
    std::fs::write(root.join("a.txt"), "one\n").unwrap();
    commit(&root, "first");
    std::fs::write(root.join("a.txt"), "one\ntwo\n").unwrap();
    commit(&root, "second");
    let status = dir.path().join("status");
    // No config, so no [send] command: the review goes to the clipboard.
    let Some(tmux) = start(&root, &dir.path().join("cache"), &status, "") else {
        return;
    };
    tmux.wait_for("the log", |s| s.contains("second"));
    // Record every byte the pane's program writes.
    let raw = dir.path().join("pane.raw");
    let pipe = format!("cat >> {}", quote(&raw));
    assert!(
        tmux.cmd(&["pipe-pane", "-O", "-t", "e2e", &pipe])
            .status
            .success()
    );
    tmux.keys(&["gg", "Enter"]);
    tmux.wait_for("the diff", |s| s.contains("+two") || s.contains(" two"));
    tmux.keys(&["G"]);
    tmux.keys(&["c", "c"]);
    tmux.wait_for("the comment box", |s| s.contains(" comment L"));
    tmux.keys(&["-l", "please explain"]);
    tmux.keys(&["Enter"]);
    tmux.wait_for("saved", |s| s.contains("Comment added (1 in batch)"));
    tmux.keys(&["Space", "r", "r"]);
    tmux.wait_for("sent", |s| {
        s.contains("Review copied to clipboard (1 comments)")
    });
    // The pipe is asynchronous: give it a moment to drain.
    let deadline = Instant::now() + Duration::from_secs(5);
    let count = loop {
        let bytes = std::fs::read(&raw).unwrap_or_default();
        let n = bytes.windows(7).filter(|w| w == b"\x1b]52;c;").count();
        if n > 0 || Instant::now() > deadline {
            break n;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(count, 1, "exactly one OSC 52 write");
    tmux.keys(&["q"]);
    tmux.wait_for("the log", |s| s.contains("first"));
    assert_eq!(tmux.quit(&status), "0");
}

#[test]
fn git_working_copy_row_appears_live_and_opens() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    std::fs::create_dir(&root).unwrap();
    git(&root, &["init", "-q"]);
    std::fs::write(root.join("a.txt"), "one\n").unwrap();
    commit(&root, "first");
    let status = dir.path().join("status");
    let Some(tmux) = start(&root, &dir.path().join("cache"), &status, "") else {
        return;
    };
    let screen = tmux.wait_for("the log", |s| s.contains("first"));
    assert!(!screen.contains("Working copy"), "{screen}");
    // The tree watcher picks up an edit on its own.
    std::fs::write(root.join("a.txt"), "one\nlive edit\n").unwrap();
    let screen = tmux.wait_for("the row", |s| s.contains("✎ Working copy  1 file"));
    assert!(screen.contains("working copy dirty"), "{screen}");
    tmux.keys(&["gg", "Enter"]);
    tmux.wait_for("the diff", |s| {
        s.contains("Working copy (uncommitted) on ") && s.contains("+ live edit")
    });
    tmux.keys(&["q"]);
    tmux.wait_for("the log", |s| s.contains("first"));
    assert_eq!(tmux.quit(&status), "0");
}

#[test]
fn jj_snapshot_and_no_snapshot() {
    if !have_jj() {
        return;
    }
    let e = JjEnv::with_history(&[("jj first", vec![w("a.txt", "one\n")])], &[], 1);
    e.write("a.txt", "one\nunsnapshotted\n");
    let dir = e.root.parent().unwrap();
    let env = format!("JJ_CONFIG={}", quote(Path::new(&e.cfg)));
    let ops = e.op_count();
    // --no-snapshot: @ is as of the last jj command, nothing is written.
    let status = dir.join("status1");
    let Some(tmux) = start_with(&e.root, &dir.join("cache"), &status, &env, "--no-snapshot") else {
        return;
    };
    let screen = tmux.wait_for("the log", |s| s.contains("jj first"));
    assert!(screen.contains(" jj "), "{screen}");
    tmux.keys(&["j"]);
    tmux.wait_for("cursor on row 2", |s| {
        position(s).is_some_and(|p| p.starts_with("2/"))
    });
    tmux.keys(&["gg", "Enter"]);
    let screen = tmux.wait_for("@ open", |s| {
        s.contains("nothing to show") || s.contains("1/")
    });
    assert!(!screen.contains("unsnapshotted"), "{screen}");
    tmux.keys(&["q"]);
    tmux.wait_for("the log", |s| s.contains("jj first"));
    assert_eq!(tmux.quit(&status), "0");
    assert_eq!(e.op_count(), ops);
    drop(tmux);
    // Default: the snapshot makes the edit visible in @. Keep this run:
    // the only check that main.rs passes cli.open_options() to vcs::open.
    let status = dir.join("status2");
    let Some(tmux) = start_with(&e.root, &dir.join("cache"), &status, &env, "") else {
        return;
    };
    tmux.wait_for("the log", |s| s.contains("jj first"));
    tmux.keys(&["gg", "Enter"]);
    tmux.wait_for("the edit", |s| s.contains("+ unsnapshotted"));
    tmux.keys(&["q"]);
    tmux.wait_for("the log", |s| s.contains("jj first"));
    assert_eq!(tmux.quit(&status), "0");
}

#[test]
fn a_saved_session_goes_to_the_test_state_dir() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    std::fs::create_dir(&root).unwrap();
    git(&root, &["init", "-q"]);
    std::fs::write(root.join("a.txt"), "one\n").unwrap();
    commit(&root, "first");
    let status = dir.path().join("status");
    let cache = dir.path().join("cache");
    let Some(tmux) = start(&root, &cache, &status, "") else {
        return;
    };
    tmux.wait_for("the log", |s| s.contains("first"));
    tmux.keys(&["Enter"]);
    tmux.wait_for("the diff", |s| s.contains("+ one"));
    tmux.keys(&["Space", "s", "s"]);
    tmux.wait_for("saved", |s| s.contains("Session saved"));
    let n = walk(&cache.join("state"))
        .iter()
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .count();
    assert_eq!(n, 1, "one session file under the test state dir");
    tmux.keys(&["q"]);
    tmux.wait_for("the log", |s| s.contains("first"));
    assert_eq!(tmux.quit(&status), "0");
}

fn walk(dir: &Path) -> Vec<std::path::PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    rd.flat_map(|e| {
        let p = e.unwrap().path();
        if p.is_dir() { walk(&p) } else { vec![p] }
    })
    .collect()
}

#[test]
fn tmux_zoom_zooms_the_pane_and_unzooms_on_quit() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    std::fs::create_dir(&root).unwrap();
    git(&root, &["init", "-q"]);
    std::fs::write(root.join("a.txt"), "one\n").unwrap();
    commit(&root, "first");
    let cache = dir.path().join("cache");
    let cfg = cache.join("config/debrief");
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::write(cfg.join("config.toml"), "[view]\ntmux_zoom = true\n").unwrap();
    let status = dir.path().join("status");
    let Some(tmux) = start_in(&root, &cache, &status, "", "", true) else {
        return;
    };
    let zoomed = || {
        let out = tmux.cmd(&[
            "display-message",
            "-p",
            "-t",
            "e2e",
            "#{window_zoomed_flag}",
        ]);
        String::from_utf8_lossy(&out.stdout).trim() == "1"
    };
    tmux.wait_for("the log", |s| s.contains("first"));
    assert!(zoomed(), "zoomed while debrief runs");
    let panes = tmux.cmd(&["display-message", "-p", "-t", "e2e", "#{window_panes}"]);
    assert_eq!(String::from_utf8_lossy(&panes.stdout).trim(), "2");
    tmux.keys(&["q"]);
    let deadline = Instant::now() + Duration::from_secs(10);
    while !status.exists() {
        assert!(Instant::now() < deadline, "debrief did not quit on q");
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(std::fs::read_to_string(&status).unwrap().trim(), "0");
    let panes = tmux.cmd(&["display-message", "-p", "-t", "e2e", "#{window_panes}"]);
    assert_eq!(
        String::from_utf8_lossy(&panes.stdout).trim(),
        "2",
        "the pane is still there"
    );
    assert!(!zoomed(), "unzoomed after quit");
}
