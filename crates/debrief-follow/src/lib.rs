//! Follow the editor's cursor, shared by debrief and ramble.
//!
//! A following tool listens on
//! `<dir>/debrief-follow-<uid>/<scope>.<tool>.<pid>.sock`. Editor hooks run
//! `debrief --goto FILE:LINE` (or `ramble --goto`), which writes
//! `<line> <absolute path>\n` to every socket in its scope. The scope is
//! the tmux window (`t<server pid>w<window id>`), or `any` outside tmux; a
//! sender in scope `any` writes to every socket. Each listener ignores
//! paths it has no use for.
//!
//! Specs: debrief `docs/specs/2026-10-31-follow-editor.md` and ramble
//! `docs/specs/2026-10-22-follow-cursor.md`.

use std::path::{Path, PathBuf};

#[cfg(unix)]
mod sock;
#[cfg(unix)]
pub use sock::*;

/// The scope of a sender or listener outside tmux.
pub const ANY: &str = "any";

/// The socket directory's name before the uid.
pub const DIR_PREFIX: &str = "debrief-follow-";

/// `FILE:LINE`, split on the last `:` so FILE may contain colons; LINE is
/// 1-based.
pub fn parse_goto(s: &str) -> Result<(PathBuf, usize), String> {
    let (file, line) = s
        .rsplit_once(':')
        .ok_or_else(|| "expected FILE:LINE".to_string())?;
    let line: usize = line
        .parse()
        .ok()
        .filter(|&n| n >= 1)
        .ok_or_else(|| format!("bad line number {line:?}"))?;
    if file.is_empty() {
        return Err("expected FILE:LINE".into());
    }
    Ok((PathBuf::from(file), line))
}

/// `file` resolved against `cwd` and canonicalised when possible: what
/// `--goto` sends.
pub fn goto_path(cwd: &Path, file: &Path) -> PathBuf {
    let abs = cwd.join(file);
    abs.canonicalize().unwrap_or(abs)
}

/// `--goto`: tell the following tools in this process's scope that the
/// editor is on `line` of `path` (absolute). Never fails: editor hooks run
/// it on every cursor move. `Err` only where sockets are unsupported.
pub fn goto(path: &Path, line: usize) -> Result<(), String> {
    #[cfg(unix)]
    {
        send_all(&default_dir(), &current_scope(), line, path);
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = (path, line);
        Err("--goto needs Linux or macOS".into())
    }
}

/// The tmux pane to split for a follow launcher: `$TMUX_PANE` when inside
/// tmux (`$TMUX` set too), else `None`.
pub fn tmux_pane(env: impl Fn(&str) -> Option<String>) -> Option<String> {
    let set = |k| env(k).filter(|v: &String| !v.is_empty());
    set("TMUX")?;
    set("TMUX_PANE")
}

/// The command that runs `argv` in a new pane beside `pane`, starting in
/// `cwd`, never through a shell. tmux hands a lone word to `sh -c` and
/// execs two or more, so a one-word `argv` runs through `env`. The cwd's
/// `#` is doubled: tmux expands formats (`#{…}`) in `-c`.
pub fn split_argv(pane: &str, cwd: &Path, argv: &[String]) -> Vec<String> {
    let mut out: Vec<String> = ["tmux", "split-window", "-h", "-t", pane, "-c"]
        .map(String::from)
        .into();
    out.push(cwd.display().to_string().replace('#', "##"));
    out.push("--".into());
    if argv.len() == 1 {
        out.push("env".into());
    }
    out.extend(argv.iter().cloned());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn goto_splits_on_the_last_colon() {
        assert_eq!(parse_goto("a:b.md:12"), Ok(("a:b.md".into(), 12)));
        for bad in ["a.md", "a.md:0", "a.md:x", ":3"] {
            assert!(parse_goto(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn goto_path_resolves_against_cwd() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a.rs"), "").unwrap();
        let canon = d.path().canonicalize().unwrap();
        assert_eq!(goto_path(d.path(), Path::new("a.rs")), canon.join("a.rs"));
        assert_eq!(
            goto_path(Path::new("/x"), Path::new("/n/gone")),
            Path::new("/n/gone")
        );
        assert_eq!(
            goto_path(Path::new("/x"), Path::new("gone")),
            Path::new("/x/gone")
        );
    }

    #[test]
    fn tmux_pane_needs_both_variables() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |k: &str| {
                pairs
                    .iter()
                    .find(|(n, _)| *n == k)
                    .map(|(_, v)| v.to_string())
            }
        };
        assert_eq!(
            tmux_pane(env(&[("TMUX", "/s,1,0"), ("TMUX_PANE", "%3")])),
            Some("%3".into())
        );
        assert_eq!(tmux_pane(env(&[("TMUX_PANE", "%3")])), None);
        assert_eq!(
            tmux_pane(env(&[("TMUX", "/s,1,0"), ("TMUX_PANE", "")])),
            None
        );
    }

    #[test]
    fn a_lone_word_runs_through_env_and_hashes_are_escaped() {
        let one = ["/r/a;touch x.md".to_string()];
        assert_eq!(
            split_argv("%3", Path::new("/r/#{x}"), &one)[6..],
            ["/r/##{x}", "--", "env", "/r/a;touch x.md"]
        );
    }

    #[test]
    fn split_runs_the_argv_after_dashdash() {
        let argv = ["nvim", "+3", "a b.rs"].map(String::from);
        assert_eq!(
            split_argv("%3", Path::new("/r"), &argv),
            [
                "tmux",
                "split-window",
                "-h",
                "-t",
                "%3",
                "-c",
                "/r",
                "--",
                "nvim",
                "+3",
                "a b.rs"
            ]
        );
    }
}
