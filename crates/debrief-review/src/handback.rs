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

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            _ => break None,
        }
    };
    let Some(status) = status else {
        // Killing the child breaks the pipe, so the writer thread ends too (unless a
        // grandchild holds stdin; then it is left detached rather than waited on).
        kill(&mut child);
        return false;
    };
    while !writer.is_finished() {
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(10));
    }
    let wrote = writer.join().is_ok_and(|r| r.is_ok());
    wrote && status.success()
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
