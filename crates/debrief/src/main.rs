use std::process::ExitCode;

use clap::Parser;
use debrief::app::{App, run};
use debrief::cli::{Cli, StartError, init_config, open_error};

fn main() -> ExitCode {
    let cli = Cli::parse();
    if let Some((file, line)) = &cli.goto {
        let cwd = std::env::current_dir().unwrap_or_default();
        return match debrief_follow::goto(&debrief_follow::goto_path(&cwd, file), *line) {
            Ok(()) => ExitCode::SUCCESS,
            Err(msg) => {
                eprintln!("debrief: {msg}");
                ExitCode::from(2)
            }
        };
    }
    if cli.follow && cfg!(not(unix)) {
        eprintln!("debrief: --follow needs Linux or macOS");
        return ExitCode::from(2);
    }
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
    let mut opts = cli.start_options(
        debrief_review::cache_dir(),
        debrief::session::state_dir(),
        debrief::app::config::Config::default_path(),
    )?;
    debrief::vcs::set_git_program(&opts.config.vcs.git);
    let vcs = debrief::vcs::open(&cli.path(), cli.open_options()).map_err(|e| open_error(&e))?;
    opts.path = cli.path_filter(vcs.root())?;
    App::new(vcs, opts).map_err(|e| open_error(&e))
}
