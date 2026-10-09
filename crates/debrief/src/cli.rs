//! Command line: `debrief [PATH] [--revset R] [--limit N] [--cache-dir DIR] [--config PATH]`.

use std::path::PathBuf;

use clap::Parser;

use crate::app::config::Config;
use crate::app::{DEFAULT_LIMIT, StartOptions};

#[derive(Debug, Parser)]
#[command(
    name = "debrief",
    version,
    about = "Catch up with what the agents committed, and answer back"
)]
pub struct Cli {
    /// A path inside the git or jj repository (default: the working directory).
    pub path: Option<PathBuf>,
    /// Revset (jj) or revision range (git) to list instead of the default.
    #[arg(long, value_name = "R")]
    pub revset: Option<String>,
    /// Most commits listed.
    #[arg(long, value_name = "N", default_value_t = DEFAULT_LIMIT)]
    pub limit: usize,
    /// Where seen state and reviews live (default: the user cache dir's
    /// `debrief`, or `$DEBRIEF_CACHE_DIR`).
    #[arg(long, value_name = "DIR")]
    pub cache_dir: Option<PathBuf>,
    /// Config file (default: `$XDG_CONFIG_HOME/debrief/config.toml`, else
    /// `~/.config/debrief/config.toml`).
    #[arg(long, value_name = "PATH")]
    pub config: Option<PathBuf>,
}

/// A startup failure: the message for stderr, and exit code 2.
#[derive(Debug, PartialEq, Eq)]
pub struct StartError(pub String);

impl Cli {
    pub fn path(&self) -> PathBuf {
        self.path.clone().unwrap_or_else(|| PathBuf::from("."))
    }

    /// The start options; `default_cache` is `debrief_review::cache_dir()`,
    /// `default_config` is `Config::default_path()` (a missing file is the
    /// defaults).
    pub fn start_options(
        &self,
        default_cache: Option<PathBuf>,
        default_config: Option<PathBuf>,
    ) -> Result<StartOptions, StartError> {
        let cache_dir = self.cache_dir.clone().or(default_cache).ok_or_else(|| {
            StartError("debrief: cannot determine cache directory; use --cache-dir".into())
        })?;
        let config = match self.config.clone().or(default_config) {
            Some(p) => Config::load(&p).map_err(|e| StartError(format!("debrief: {e:#}")))?,
            None => Config::defaults(),
        };
        Ok(StartOptions {
            cache_dir,
            revset: self.revset.clone(),
            limit: self.limit,
            config,
        })
    }
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
        ])
        .unwrap();
        assert_eq!(c.path(), PathBuf::from("repo"));
        let o = c.start_options(None, None).unwrap();
        assert_eq!(o.cache_dir, PathBuf::from("/c"));
        assert_eq!(o.revset.as_deref(), Some("main.."));
        assert_eq!(o.limit, 5);
    }

    #[test]
    fn defaults() {
        let c = Cli::try_parse_from(["debrief"]).unwrap();
        assert_eq!(c.path(), PathBuf::from("."));
        let o = c.start_options(Some("/d/debrief".into()), None).unwrap();
        assert_eq!(
            o.cache_dir,
            PathBuf::from("/d/debrief"),
            "no extra /debrief"
        );
        assert_eq!(o.limit, 200);
        assert_eq!(o.revset, None);
    }

    #[test]
    fn no_cache_dir_is_an_error() {
        let c = Cli::try_parse_from(["debrief"]).unwrap();
        assert_eq!(
            c.start_options(None, None).unwrap_err().0,
            "debrief: cannot determine cache directory; use --cache-dir"
        );
    }

    #[test]
    fn config_flag_loads_the_file() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("c.toml");
        std::fs::write(&p, "[send]\ncommand = [\"x\"]\n").unwrap();
        let c = Cli::try_parse_from(["debrief", "--config", p.to_str().unwrap()]).unwrap();
        assert_eq!(
            c.start_options(Some("/d".into()), None)
                .unwrap()
                .config
                .send
                .command,
            ["x"]
        );
        std::fs::write(&p, "[bad]\n").unwrap();
        let e = c.start_options(Some("/d".into()), None).unwrap_err().0;
        assert!(e.starts_with("debrief: config error in "), "{e}");
    }

    #[test]
    fn open_errors_keep_the_chain() {
        let e = anyhow::anyhow!("inner").context("outer");
        assert_eq!(open_error(&e).0, "debrief: outer: inner");
    }
}
