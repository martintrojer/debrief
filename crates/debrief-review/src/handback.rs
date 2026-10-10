//! Hand a review back: pipe the markdown to a configured command, or fall back to the clipboard.

use std::io::{self, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(10);

/// Where the markdown went.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sent {
    Command,
    Clipboard,
}

/// Runs `command` with `markdown` on stdin and `DEBRIEF_ROOT=<root>` set. An empty
/// command, a spawn failure, a non-zero exit, a stdin write failure or a 10 s
/// timeout fall back to `clipboard(markdown)`. Clipboard errors are returned.
/// Never clears the review; callers clear after `Ok`.
pub fn send(
    command: &[String],
    markdown: &str,
    root: &Path,
    clipboard: &mut dyn FnMut(&str) -> io::Result<()>,
) -> io::Result<Sent> {
    send_with_timeout(command, markdown, root, clipboard, TIMEOUT)
}

fn send_with_timeout(
    command: &[String],
    markdown: &str,
    root: &Path,
    clipboard: &mut dyn FnMut(&str) -> io::Result<()>,
    timeout: Duration,
) -> io::Result<Sent> {
    if run(command, markdown, root, timeout) {
        Ok(Sent::Command)
    } else {
        clipboard(markdown)?;
        Ok(Sent::Clipboard)
    }
}

/// How long a child whose input pipe broke has to exit before it counts as
/// stuck (a child that exits as it closes its input isn't reapable at once).
const EXIT_GRACE: Duration = Duration::from_millis(500);

/// True if the command ran, took all of stdin and exited 0 within `timeout`.
fn run(command: &[String], markdown: &str, root: &Path, timeout: Duration) -> bool {
    let Some((program, args)) = command.split_first() else {
        return false;
    };
    let Ok(mut child) = Command::new(program)
        .args(args)
        .env("DEBRIEF_ROOT", root)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    let Some(mut stdin) = child.stdin.take() else {
        kill(&mut child);
        return false;
    };
    let input = markdown.to_owned();
    // Dropping stdin at the end of the thread closes it.
    let writer = thread::spawn(move || stdin.write_all(input.as_bytes()));

    // Poll the writer alongside the child. A write error is final, so kill
    // and fall back at once rather than waiting for the child, except a
    // broken pipe from a child that already exited: a command may ignore
    // its input, and then its exit status decides.
    let deadline = Instant::now() + timeout;
    let mut writer = Some(writer);
    // When the pipe broke: a child that exits right after closing its input
    // may not be reapable yet, so it gets a moment before it counts as stuck.
    let mut broken_pipe: Option<Instant> = None;
    loop {
        if writer.as_ref().is_some_and(|w| w.is_finished()) {
            match writer.take().map(|w| w.join()) {
                Some(Ok(Ok(()))) => {}
                Some(Ok(Err(e))) if e.kind() == std::io::ErrorKind::BrokenPipe => {
                    broken_pipe = Some(Instant::now());
                }
                _ => {
                    kill(&mut child);
                    return false;
                }
            }
        }
        match child.try_wait() {
            // Success needs stdin settled; if the writer is still going, keep polling.
            Ok(Some(status)) if writer.is_none() => return status.success(),
            // The pipe broke and the child still runs after the grace period:
            // it closed its input without exiting, so it won't get the
            // review. Fall back.
            Ok(None) if broken_pipe.is_some_and(|t| t.elapsed() >= EXIT_GRACE) => {
                kill(&mut child);
                return false;
            }
            Ok(_) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            _ => {
                // Killing the child breaks the pipe, so the writer thread ends too (unless a
                // grandchild holds stdin; then it is left detached rather than waited on).
                kill(&mut child);
                return false;
            }
        }
    }
}

fn kill(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeout_kills_a_child_that_never_reads_and_falls_back() {
        let cmd = vec!["sleep".to_owned(), "30".to_owned()];
        let input = "x".repeat(200 * 1024);
        let mut copied = None;
        let start = Instant::now();
        let sent = send_with_timeout(
            &cmd,
            &input,
            Path::new("/"),
            &mut |md: &str| {
                copied = Some(md.len());
                Ok(())
            },
            Duration::from_millis(200),
        )
        .unwrap();
        assert_eq!(sent, Sent::Clipboard);
        assert_eq!(copied, Some(input.len()));
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "{:?}",
            start.elapsed()
        );
    }

    #[test]
    fn stdin_write_failure_falls_back_promptly_and_reaps_the_child() {
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("pid");
        let script = format!(
            "echo $$ > '{}'; exec 0<&-; exec sleep 30",
            pidfile.display()
        );
        let cmd = vec!["sh".to_owned(), "-c".to_owned(), script];
        let input = "x".repeat(200 * 1024);
        let start = Instant::now();
        let sent = send(&cmd, &input, Path::new("/"), &mut |_: &str| Ok(())).unwrap();
        assert_eq!(sent, Sent::Clipboard);
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "{:?}",
            start.elapsed()
        );
        // A reaped pid is gone; a zombie would still answer `kill -0`.
        let pid = std::fs::read_to_string(&pidfile).unwrap();
        let alive = Command::new("kill")
            .args(["-0", pid.trim()])
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success();
        assert!(!alive, "child {} not reaped", pid.trim());
    }

    /// A command that ignores its input and exits 0 (`true`) has handled
    /// the review; the broken pipe it leaves is not a failure.
    #[test]
    fn a_command_that_exits_0_without_reading_counts_as_sent() {
        let input = "x".repeat(1024 * 1024);
        for _ in 0..20 {
            let mut copied = false;
            let sent = send(
                &["true".to_owned()],
                &input,
                Path::new("/"),
                &mut |_: &str| {
                    copied = true;
                    Ok(())
                },
            )
            .unwrap();
            assert_eq!(sent, Sent::Command);
            assert!(!copied);
        }
        // Exiting non-zero without reading still falls back.
        let sent = send(
            &["false".to_owned()],
            &input,
            Path::new("/"),
            &mut |_: &str| Ok(()),
        )
        .unwrap();
        assert_eq!(sent, Sent::Clipboard);
    }

    #[test]
    fn clipboard_errors_propagate() {
        let err = send_with_timeout(
            &[],
            "md",
            Path::new("/"),
            &mut |_: &str| Err(io::Error::other("no clipboard")),
            Duration::from_millis(200),
        )
        .unwrap_err();
        assert_eq!(err.to_string(), "no clipboard");
    }
}
