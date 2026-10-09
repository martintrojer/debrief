//! Command line: `debrief [PATH] [--revset R] [--limit N] [--cache-dir DIR]`.

use std::path::PathBuf;

use clap::Parser;

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
}

/// A startup failure: the message for stderr, and exit code 2.
#[derive(Debug, PartialEq, Eq)]
pub struct StartError(pub String);

impl Cli {
    pub fn path(&self) -> PathBuf {
        self.path.clone().unwrap_or_else(|| PathBuf::from("."))
    }

    /// The start options; `default_cache` is `debrief_review::cache_dir()`.
    pub fn start_options(
        &self,
        default_cache: Option<PathBuf>,
    ) -> Result<StartOptions, StartError> {
        let cache_dir = self.cache_dir.clone().or(default_cache).ok_or_else(|| {
            StartError("debrief: cannot determine cache directory; use --cache-dir".into())
        })?;
        Ok(StartOptions {
            cache_dir,
            revset: self.revset.clone(),
            limit: self.limit,
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
        let o = c.start_options(None).unwrap();
        assert_eq!(o.cache_dir, PathBuf::from("/c"));
        assert_eq!(o.revset.as_deref(), Some("main.."));
        assert_eq!(o.limit, 5);
    }

    #[test]
    fn defaults() {
        let c = Cli::try_parse_from(["debrief"]).unwrap();
        assert_eq!(c.path(), PathBuf::from("."));
        let o = c.start_options(Some("/d/debrief".into())).unwrap();
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
            c.start_options(None).unwrap_err().0,
            "debrief: cannot determine cache directory; use --cache-dir"
        );
    }

    #[test]
    fn open_errors_keep_the_chain() {
        let e = anyhow::anyhow!("inner").context("outer");
        assert_eq!(open_error(&e).0, "debrief: outer: inner");
    }
}
