//! `C-h/j/k/l` (`[keys] pane_nav`): move to the pane left / below / above
//! / right, as in nvim. On the commit screen the panes are the file list
//! and the diff (side by side: its old and new halves), in screen order.
//! Where there is no pane that way (the edge, up / down, the log) and
//! debrief runs inside tmux or rex, the move goes on to the multiplexer's
//! pane there (`tmux select-pane`, `rex focus`), as vim-tmux-navigator
//! does (ramble's approach).

use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::App;
use super::keys::{Action, KeyResult};
use super::launch::{Exit, LaunchCommand};

pub use debrief_follow::Dir;
use debrief_follow::Mux;

/// How long the multiplexer hand-off may hold up the UI.
pub const MUX_WAIT: Duration = Duration::from_millis(200);

/// The command that moves `mux`'s focus toward `dir`.
pub fn focus_command(mux: &Mux, dir: Dir) -> LaunchCommand {
    LaunchCommand {
        name: mux.name().into(),
        argv: mux.focus_argv(dir),
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

    /// Move to the pane toward `dir`; with none there inside tmux or rex,
    /// move to the multiplexer's pane that way instead.
    pub(super) fn pane_nav(&mut self, dir: Dir) {
        if self.pane_move(dir) {
            return;
        }
        if let Some(mux) = Mux::detect(&*self.env) {
            // Failure (no pane there, the multiplexer gone) is not ours
            // to report.
            let _ = (self.pane_runner)(&focus_command(&mux, dir));
        }
    }

    /// Replace the runner of the multiplexer hand-off at a pane edge and
    /// of a follow launcher's split (tests record the command).
    pub fn with_pane_runner(
        mut self,
        runner: impl FnMut(&LaunchCommand) -> anyhow::Result<Exit> + 'static,
    ) -> Self {
        self.pane_runner = Box::new(runner);
        self
    }
}

/// The pane runner gave up waiting; the command goes on (a slow
/// multiplexer still opens its split).
#[derive(Debug)]
pub struct StillRunning;

impl std::fmt::Display for StillRunning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("still running")
    }
}

impl std::error::Error for StillRunning {}

/// Default pane runner: start the command with no terminal I/O and wait at
/// most [`MUX_WAIT`]; a slower command finishes on its own.
pub fn spawn_capped(cmd: &LaunchCommand) -> anyhow::Result<Exit> {
    spawn_within(cmd, MUX_WAIT)
}

/// [`spawn_capped`] with the wait as a parameter.
fn spawn_within(cmd: &LaunchCommand, wait: Duration) -> anyhow::Result<Exit> {
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
    let deadline = Instant::now() + wait;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Exit::Code(status.code().unwrap_or(-1)));
        }
        if Instant::now() >= deadline {
            // Reap it off the UI thread.
            std::thread::spawn(move || child.wait());
            return Err(anyhow::Error::new(StillRunning)
                .context(format!("{prog} still running after {wait:?}")));
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
    fn spawn_capped_returns_the_exit_code_of_a_quick_command() {
        // A generous wait: process start-up on a loaded machine can take
        // longer than the 200 ms the UI allows, and that isn't what this
        // checks.
        assert!(matches!(
            spawn_within(&cmd(&["sh", "-c", "exit 3"]), Duration::from_secs(10)),
            Ok(Exit::Code(3))
        ));
        assert!(spawn_capped(&cmd(&["/nonexistent/debrief-test"])).is_err());
    }

    #[test]
    fn spawn_capped_never_waits_much_longer_than_the_cap() {
        let t = Instant::now();
        assert!(spawn_capped(&cmd(&["sleep", "5"])).is_err());
        assert!(t.elapsed() < MUX_WAIT + Duration::from_secs(1));
    }
}
