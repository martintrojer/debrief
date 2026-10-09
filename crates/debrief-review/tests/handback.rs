use std::fs;
use std::path::Path;

use debrief_review::handback::{Sent, send};
use tempfile::TempDir;

fn run(command: &[&str], markdown: &str, root: &Path) -> (Sent, Option<String>) {
    let command: Vec<String> = command.iter().map(|s| s.to_string()).collect();
    let mut copied = None;
    let sent = send(&command, markdown, root, &mut |md: &str| {
        copied = Some(md.to_owned());
        Ok(())
    })
    .unwrap();
    (sent, copied)
}

#[test]
fn empty_command_uses_clipboard() {
    assert_eq!(
        run(&[], "md", Path::new("/")),
        (Sent::Clipboard, Some("md".into()))
    );
}

#[test]
fn command_gets_markdown_and_root() {
    let tmp = TempDir::new().unwrap();
    let out = tmp.path().join("out");
    let script = format!(
        "cat > '{0}' && printf '%s' \"$DEBRIEF_ROOT\" > '{0}.root'",
        out.display()
    );
    let (sent, copied) = run(&["sh", "-c", &script], "# Review\nbody\n", tmp.path());
    assert_eq!((sent, copied), (Sent::Command, None));
    assert_eq!(fs::read_to_string(&out).unwrap(), "# Review\nbody\n");
    assert_eq!(
        fs::read_to_string(tmp.path().join("out.root")).unwrap(),
        tmp.path().display().to_string()
    );
}

#[test]
fn failing_command_falls_back() {
    assert_eq!(
        run(&["false"], "md", Path::new("/")),
        (Sent::Clipboard, Some("md".into()))
    );
}

#[test]
fn missing_binary_falls_back() {
    assert_eq!(
        run(&["/nonexistent/debrief-send"], "md", Path::new("/")),
        (Sent::Clipboard, Some("md".into()))
    );
}
