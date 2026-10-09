use std::process::ExitCode;

use clap::Parser;
use debrief::app::{App, run};
use debrief::cli::{Cli, StartError, open_error};

fn main() -> ExitCode {
    let cli = Cli::parse();
    match start(&cli) {
        Ok(app) => match run::run(app) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("debrief: {e:#}");
                ExitCode::FAILURE
            }
        },
        Err(StartError(msg)) => {
            eprintln!("{msg}");
            ExitCode::from(2)
        }
    }
}

fn start(cli: &Cli) -> Result<App, StartError> {
    let opts = cli.start_options(
        debrief_review::cache_dir(),
        debrief::app::config::Config::default_path(),
    )?;
    let vcs = debrief::vcs::open(&cli.path()).map_err(|e| open_error(&e))?;
    App::new(vcs, opts).map_err(|e| open_error(&e))
}
