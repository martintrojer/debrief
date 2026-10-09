//! End-to-end: the real binary in an isolated tmux server
//! (`tmux -L debrief-test-<pid>-<n>`).

mod common;

use std::path::Path;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use common::{JjEnv, commit, git, have_jj};

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
/// debrief's exit status to `status_file`.
fn start(repo: &Path, cache: &Path, status_file: &Path, env: &str) -> Option<Tmux> {
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
        "env {env} {} {} --cache-dir {}; echo $? > {}",
        quote(Path::new(bin)),
        quote(repo),
        quote(cache),
        quote(status_file)
    );
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
        &shell_cmd,
    ]);
    assert!(out.status.success(), "tmux new-session: {out:?}");
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

#[test]
fn jj_log_renders_moves_and_quits() {
    if !have_jj() {
        return;
    }
    let e = JjEnv::new(false);
    for i in 0..2 {
        e.write("a.txt", &format!("{i}\n"));
        e.jj(&["--quiet", "commit", "-m", &format!("jj commit {i}")]);
    }
    let dir = e.root.parent().unwrap();
    let status = dir.join("status");
    let env = format!("JJ_CONFIG={}", quote(Path::new(&e.cfg)));
    let Some(tmux) = start(&e.root, &dir.join("cache"), &status, &env) else {
        return;
    };
    let screen = tmux.wait_for("the log", |s| s.contains("jj commit 0"));
    assert!(screen.contains(" jj "), "{screen}");
    tmux.keys(&["j"]);
    tmux.wait_for("cursor on row 2", |s| {
        position(s).is_some_and(|p| p.starts_with("2/"))
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
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.starts_with("debrief: not a git or jj repository: "),
        "{err}"
    );
}
