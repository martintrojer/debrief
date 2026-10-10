use std::process::ExitCode;

use clap::Parser;
use debrief::app::{App, run};
use debrief::cli::{Cli, StartError, init_config, open_error};

fn main() -> ExitCode {
    let cli = Cli::parse();
    if cli.init_config {
        return match init_config(&cli, debrief::app::config::Config::default_path()) {
            Ok(path) => {
                println!("{}", path.display());
                ExitCode::SUCCESS
            }
            Err(msg) => {
                eprintln!("{msg}");
                ExitCode::FAILURE
            }
        };
    }
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
    let vcs = debrief::vcs::open(&cli.path(), cli.open_options()).map_err(|e| open_error(&e))?;
    App::new(vcs, opts).map_err(|e| open_error(&e))
}
