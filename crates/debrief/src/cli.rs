//! Command line: `debrief [PATH] [--branch B | --revset R] [--limit N] [--cache-dir DIR]
//! [--state-dir DIR] [--config PATH] [--no-snapshot] [--follow]`, `debrief --init-config
//! [--config PATH]`, or `debrief --goto FILE:LINE`.

use std::path::PathBuf;

use clap::Parser;

use crate::app::StartOptions;
use crate::app::config::{self, Config};
use crate::vcs::OpenOptions;

#[derive(Debug, Parser)]
#[command(
    name = "debrief",
    version,
    about = "Catch up with what the agents committed, and answer back"
)]
pub struct Cli {
    /// A path inside the git or jj repository (default: the working
    /// directory). A file or directory below the repository's root also
    /// filters the log to the commits touching it.
    pub path: Option<PathBuf>,
    /// Revset (jj) or revision range (git) to list instead of `[log]
    /// revset`, else the current commit and its ancestors.
    #[arg(long, value_name = "R", conflicts_with = "branch")]
    pub revset: Option<String>,
    /// The branch (git) or bookmark (jj) to list: its commits not in
    /// trunk (default: the current branch). `<leader>b` switches.
    #[arg(long, value_name = "B")]
    pub branch: Option<String>,
    /// Commits loaded at first and per page (default: `[log] limit`, else
    /// 200).
    #[arg(long, value_name = "N")]
    pub limit: Option<usize>,
    /// Where seen state and reviews live (default: the user cache dir's
    /// `debrief`, or `$DEBRIEF_CACHE_DIR`).
    #[arg(long, value_name = "DIR")]
    pub cache_dir: Option<PathBuf>,
    /// Where saved review sessions live (default: `$DEBRIEF_STATE_DIR`,
    /// else `$XDG_STATE_HOME/debrief` or `~/.local/state/debrief`).
    #[arg(long, value_name = "DIR")]
    pub state_dir: Option<PathBuf>,
    /// Config file (default: `$XDG_CONFIG_HOME/debrief/config.toml`, else
    /// `~/.config/debrief/config.toml`).
    #[arg(long, value_name = "PATH")]
    pub config: Option<PathBuf>,
    /// jj: don't snapshot the working copy; `@` shows as of the last jj
    /// command and debrief writes nothing to the repo.
    #[arg(long)]
    pub no_snapshot: bool,
    /// Write the commented default config to the config path (or
    /// `--config`) and exit; an existing file is left alone (exit 1).
    #[arg(long, conflicts_with = "goto")]
    pub init_config: bool,
    /// Follow the editor's cursor: the open commit's diff moves to where
    /// `--goto` (an editor hook) says it is. `<leader>O` starts it too.
    #[arg(long)]
    pub follow: bool,
    /// Tell following debriefs and rambles that the editor's cursor is on
    /// FILE:LINE, then exit (0 even when none listens).
    #[arg(
        long,
        value_name = "FILE:LINE",
        value_parser = debrief_follow::parse_goto,
        conflicts_with_all = [
            "path", "revset", "branch", "limit", "cache_dir", "state_dir",
            "config", "no_snapshot", "follow",
        ]
    )]
    pub goto: Option<(PathBuf, usize)>,
}

/// A startup failure: the message for stderr, and exit code 2.
#[derive(Debug, PartialEq, Eq)]
pub struct StartError(pub String);

impl Cli {
    pub fn path(&self) -> PathBuf {
        self.path.clone().unwrap_or_else(|| PathBuf::from("."))
    }

    /// The log's path filter for a repository at `root`: `PATH` relative
    /// to it, `None` without a `PATH` or when it is the root.
    pub fn path_filter(&self, root: &std::path::Path) -> Result<Option<String>, StartError> {
        match &self.path {
            None => Ok(None),
            Some(p) => crate::vcs::path_in(root, p).map_err(|e| open_error(&e)),
        }
    }

    /// How to open the repository.
    pub fn open_options(&self) -> OpenOptions {
        OpenOptions {
            snapshot: !self.no_snapshot,
        }
    }

    /// The start options; `default_cache` is `debrief_review::cache_dir()`,
    /// `default_state` is `session::state_dir()`, `default_config` is
    /// `Config::default_path()` (a missing file is the defaults).
    pub fn start_options(
        &self,
        default_cache: Option<PathBuf>,
        default_state: Option<PathBuf>,
        default_config: Option<PathBuf>,
    ) -> Result<StartOptions, StartError> {
        let cache_dir = self.cache_dir.clone().or(default_cache).ok_or_else(|| {
            StartError("debrief: cannot determine cache directory; use --cache-dir".into())
        })?;
        let state_dir = self.state_dir.clone().or(default_state).ok_or_else(|| {
            StartError("debrief: cannot determine state directory; use --state-dir".into())
        })?;
        let config = match self.config.clone().or(default_config) {
            Some(p) => Config::load(&p).map_err(|e| StartError(format!("debrief: {e:#}")))?,
            None => Config::defaults(),
        };
        Ok(StartOptions {
            cache_dir,
            state_dir,
            // A configured revset stays for the picker's `revset:` entry;
            // `--branch` is what the log starts on.
            revset: self.revset.clone().or_else(|| config.log.revset.clone()),
            branch: self.branch.clone(),
            path: None,
            limit: self.limit.unwrap_or(config.log.limit),
            config,
            follow: self.follow,
        })
    }
}

/// `--init-config`: write [`config::DEFAULT_TOML`] to `--config`, else
/// `default_config`. `Ok` is the path written (for stdout); `Err` the
/// stderr line (exit 1).
pub fn init_config(cli: &Cli, default_config: Option<PathBuf>) -> Result<PathBuf, String> {
    let path = cli.config.clone().or(default_config).ok_or_else(|| {
        "debrief: cannot find a config directory (set HOME or XDG_CONFIG_HOME)".to_string()
    })?;
    config::write_default(&path).map_err(|e| format!("debrief: {e:#}"))?;
    Ok(path)
}

/// The stderr line for an error while opening: `debrief: ` and the whole
/// anyhow chain, unchanged.
pub fn open_error(e: &anyhow::Error) -> StartError {
    StartError(format!("debrief: {e:#}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_parse() {
        let c = Cli::try_parse_from([
            "debrief",
            "repo",
            "--revset",
            "main..",
            "--limit",
            "5",
            "--cache-dir",
            "/c",
            "--state-dir",
            "/s",
        ])
        .unwrap();
        assert_eq!(c.path(), PathBuf::from("repo"));
        let o = c.start_options(None, None, None).unwrap();
        assert_eq!(o.cache_dir, PathBuf::from("/c"));
        assert_eq!(o.state_dir, PathBuf::from("/s"));
        assert_eq!(o.revset.as_deref(), Some("main.."));
        assert_eq!(o.limit, 5);
        assert!(c.open_options().snapshot);
        let c = Cli::try_parse_from(["debrief", "--no-snapshot"]).unwrap();
        assert!(!c.open_options().snapshot);
    }

    #[test]
    fn defaults() {
        let c = Cli::try_parse_from(["debrief"]).unwrap();
        assert_eq!(c.path(), PathBuf::from("."));
        let o = c
            .start_options(Some("/d/debrief".into()), Some("/s/debrief".into()), None)
            .unwrap();
        assert_eq!(
            o.cache_dir,
            PathBuf::from("/d/debrief"),
            "no extra /debrief"
        );
        assert_eq!(o.state_dir, PathBuf::from("/s/debrief"));
        assert_eq!(o.limit, 200);
        assert_eq!(o.revset, None);
    }

    #[test]
    fn log_config_applies_unless_a_flag_is_given() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("c.toml");
        std::fs::write(&p, "[log]\nrevset = \"main..\"\nlimit = 7\n").unwrap();
        let opts = |args: &[&str]| {
            let mut argv = vec!["debrief", "--config", p.to_str().unwrap()];
            argv.extend(args);
            Cli::try_parse_from(argv)
                .unwrap()
                .start_options(Some("/d".into()), Some("/s".into()), None)
                .unwrap()
        };
        let o = opts(&[]);
        assert_eq!((o.revset.as_deref(), o.limit), (Some("main.."), 7));
        let o = opts(&["--revset", "::@", "--limit", "3"]);
        assert_eq!((o.revset.as_deref(), o.limit), (Some("::@"), 3));
        let o = opts(&["--limit", "3"]);
        assert_eq!((o.revset.as_deref(), o.limit), (Some("main.."), 3));
        let o = opts(&["--revset", "::@"]);
        assert_eq!((o.revset.as_deref(), o.limit), (Some("::@"), 7));
        // A branch starts the log; the configured revset stays pickable.
        let o = opts(&["--branch", "feat/x"]);
        assert_eq!(
            (o.revset.as_deref(), o.branch.as_deref()),
            (Some("main.."), Some("feat/x"))
        );
    }

    #[test]
    fn branch_and_revset_conflict() {
        let e = Cli::try_parse_from(["debrief", "--branch", "b", "--revset", "r"]).unwrap_err();
        assert_eq!(e.kind(), clap::error::ErrorKind::ArgumentConflict);
        assert_eq!(e.exit_code(), 2);
    }

    #[test]
    fn follow_reaches_the_start_options_and_goto_stands_alone() {
        let c = Cli::try_parse_from(["debrief", "--follow"]).unwrap();
        let o = c
            .start_options(Some("/c".into()), Some("/s".into()), None)
            .unwrap();
        assert!(o.follow);
        let c = Cli::try_parse_from(["debrief", "--goto", "a:b.rs:12"]).unwrap();
        assert_eq!(c.goto, Some(("a:b.rs".into(), 12)));
        for argv in [
            &["debrief", "--goto", "a.rs:1", "--follow"][..],
            &["debrief", "--goto", "a.rs:1", "some/path"],
            &["debrief", "--goto", "a.rs:1", "--init-config"],
        ] {
            let e = Cli::try_parse_from(argv).unwrap_err();
            assert_eq!(
                e.kind(),
                clap::error::ErrorKind::ArgumentConflict,
                "{argv:?}"
            );
        }
        assert!(Cli::try_parse_from(["debrief", "--goto", "a.rs:0"]).is_err());
    }

    #[test]
    fn no_cache_dir_is_an_error() {
        let c = Cli::try_parse_from(["debrief"]).unwrap();
        assert_eq!(
            c.start_options(None, Some("/s".into()), None)
                .unwrap_err()
                .0,
            "debrief: cannot determine cache directory; use --cache-dir"
        );
        assert_eq!(
            c.start_options(Some("/c".into()), None, None)
                .unwrap_err()
                .0,
            "debrief: cannot determine state directory; use --state-dir"
        );
    }

    #[test]
    fn config_flag_loads_the_file() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("c.toml");
        std::fs::write(&p, "[send]\ncommand = [\"x\"]\n").unwrap();
        let c = Cli::try_parse_from(["debrief", "--config", p.to_str().unwrap()]).unwrap();
        assert_eq!(
            c.start_options(Some("/d".into()), Some("/s".into()), None)
                .unwrap()
                .config
                .send
                .command,
            ["x"]
        );
        std::fs::write(&p, "[bad]\n").unwrap();
        let e = c
            .start_options(Some("/d".into()), Some("/s".into()), None)
            .unwrap_err()
            .0;
        assert!(
            e.starts_with(&format!(
                "debrief: config error at {} line 1: ",
                p.display()
            )),
            "{e}"
        );
    }

    #[test]
    fn open_errors_keep_the_chain() {
        let e = anyhow::anyhow!("inner").context("outer");
        assert_eq!(open_error(&e).0, "debrief: outer: inner");
    }
}
