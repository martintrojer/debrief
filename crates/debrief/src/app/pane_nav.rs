//! `C-h/j/k/l` (`[keys] pane_nav`): move to the pane left / below / above
//! / right, as in nvim. On the commit screen the panes are the file list
//! and the diff (side by side: its old and new halves), in screen order.
//! Where there is no pane that way (the edge, up / down, the log) and
//! debrief runs inside tmux, the move goes on to the tmux pane there
//! (`tmux select-pane`), as vim-tmux-navigator does (ramble's approach).

use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::App;
use super::keys::{Action, KeyResult};
use super::launch::{Exit, LaunchCommand};

/// How long the tmux hand-off may hold up the UI.
pub const TMUX_WAIT: Duration = Duration::from_millis(200);

/// A pane direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    Left,
    Down,
    Up,
    Right,
}

impl Dir {
    /// The `tmux select-pane` flag for this direction.
    pub fn tmux_flag(self) -> &'static str {
        match self {
            Dir::Left => "-L",
            Dir::Down => "-D",
            Dir::Up => "-U",
            Dir::Right => "-R",
        }
    }
}

/// The `tmux select-pane` command for `dir`.
pub fn tmux_command(dir: Dir) -> LaunchCommand {
    LaunchCommand {
        name: "tmux".into(),
        argv: vec!["tmux".into(), "select-pane".into(), dir.tmux_flag().into()],
        cwd: ".".into(),
    }
}

/// `C-h/j/k/l` as a [`Dir`].
fn ctrl_dir(k: &KeyEvent) -> Option<Dir> {
    if !k.modifiers.contains(KeyModifiers::CONTROL) {
        return None;
    }
    Some(match k.code {
        KeyCode::Char('h') => Dir::Left,
        KeyCode::Char('j') => Dir::Down,
        KeyCode::Char('k') => Dir::Up,
        KeyCode::Char('l') => Dir::Right,
        _ => return None,
    })
}

impl App {
    /// `C-h/j/k/l` while `[keys] pane_nav` is on.
    pub(super) fn pane_nav_keymap(&self, keys: &[KeyEvent]) -> Option<KeyResult> {
        if !self.config.keys.pane_nav {
            return None;
        }
        let [k] = keys else { return None };
        ctrl_dir(k).map(|d| KeyResult::Action(Action::PaneNav(d)))
    }

    /// Move to the pane toward `dir`; with none there and `$TMUX` set,
    /// select the tmux pane that way instead.
    pub(super) fn pane_nav(&mut self, dir: Dir) {
        if !self.pane_move(dir) && self.in_tmux() {
            // Failure (no pane there, tmux gone) is not ours to report.
            let _ = (self.pane_runner)(&tmux_command(dir));
        }
    }

    fn in_tmux(&self) -> bool {
        (self.env)("TMUX").is_some_and(|v| !v.is_empty())
    }

    /// Replace the runner of the tmux hand-off at a pane edge (tests
    /// record the command).
    pub fn with_pane_runner(
        mut self,
        runner: impl FnMut(&LaunchCommand) -> anyhow::Result<Exit> + 'static,
    ) -> Self {
        self.pane_runner = Box::new(runner);
        self
    }
}

/// Default pane runner: start the command with no terminal I/O and wait at
/// most [`TMUX_WAIT`]; a slower command finishes on its own.
pub fn spawn_capped(cmd: &LaunchCommand) -> anyhow::Result<Exit> {
    use std::process::{Command, Stdio};
    let (prog, args) = cmd
        .argv
        .split_first()
        .ok_or_else(|| anyhow::anyhow!("empty command"))?;
    let mut child = Command::new(prog)
        .args(args)
        .current_dir(&cmd.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let deadline = Instant::now() + TMUX_WAIT;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Exit::Code(status.code().unwrap_or(-1)));
        }
        if Instant::now() >= deadline {
            // Reap it off the UI thread.
            std::thread::spawn(move || child.wait());
            anyhow::bail!("{prog} still running after {TMUX_WAIT:?}");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(argv: &[&str]) -> LaunchCommand {
        LaunchCommand {
            name: "t".into(),
            argv: argv.iter().map(|s| s.to_string()).collect(),
            cwd: ".".into(),
        }
    }

    #[test]
    fn flags_follow_tmux() {
        let flags: Vec<_> = [Dir::Left, Dir::Down, Dir::Up, Dir::Right]
            .map(|d| tmux_command(d).argv.join(" "))
            .to_vec();
        assert_eq!(
            flags,
            [
                "tmux select-pane -L",
                "tmux select-pane -D",
                "tmux select-pane -U",
                "tmux select-pane -R"
            ]
        );
    }

    #[test]
    fn spawn_capped_returns_the_exit_code_of_a_quick_command() {
        assert!(matches!(
            spawn_capped(&cmd(&["sh", "-c", "exit 3"])),
            Ok(Exit::Code(3))
        ));
        assert!(spawn_capped(&cmd(&["/nonexistent/debrief-test"])).is_err());
    }

    #[test]
    fn spawn_capped_never_waits_much_longer_than_the_cap() {
        let t = Instant::now();
        assert!(spawn_capped(&cmd(&["sleep", "5"])).is_err());
        assert!(t.elapsed() < TMUX_WAIT + Duration::from_secs(1));
    }
}
