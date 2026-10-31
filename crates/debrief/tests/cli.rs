//! The binary's `--goto` (the editor hook's client): it needs no repo or
//! terminal and always exits 0, even with nobody listening.

use std::process::Command;

#[test]
fn goto_exits_0_with_no_listener_and_rejects_a_bad_line() {
    let d = tempfile::tempdir().unwrap();
    let run = |arg: &str| {
        Command::new(env!("CARGO_BIN_EXE_debrief"))
            .args(["--goto", arg])
            .current_dir(d.path())
            // An empty socket base: no listener, nothing of the user's.
            .env("XDG_RUNTIME_DIR", d.path())
            .output()
            .unwrap()
    };
    let out = run("a.rs:3");
    assert!(out.status.success(), "{out:?}");
    assert!(out.stdout.is_empty() && out.stderr.is_empty(), "{out:?}");
    let out = run("a.rs:0");
    assert_eq!(out.status.code(), Some(2), "{out:?}");
}
