use std::process::ExitCode;

mod cli;
mod commands;
mod config;
mod environment;
mod help;
mod lockfile;
mod repository;
mod ui;

#[cfg(test)]
mod tests;

use cli::Command;
use commands::{clean, init, install, purge, remove, reset, status, sync};
use config::Config;
use nux_eval::Host;
use nux_build::Workspace;
use nux_log::{Style, Term};

fn main() -> ExitCode {
    let command = match cli::parse(std::env::args().skip(1)) {
        Ok(command) => command,
        Err(e) => {
            eprintln!("{} {e}\n\n{}", Term::stderr().paint(Style::Error, "error:"), e.hint());
            return ExitCode::from(2);
        }
    };
    match run(command) {
        Ok(true) => ExitCode::SUCCESS,
        // Cancelled at the confirmation prompt.
        Ok(false) => ExitCode::FAILURE,
        Err(message) => {
            eprintln!("\n{} {message}", Term::stderr().paint(Style::Error, "error:"));
            ExitCode::FAILURE
        }
    }
}

/// Runs `command`; `Ok(false)` means the user declined to continue.
fn run(command: Command) -> Result<bool, String> {
    let term = Term::stdout();
    // What Nux built itself is not the host's.
    let host = |workspace: &Workspace| Host::from_env(vec![workspace.home().to_path_buf()]);
    let sync_io = |config: Config| -> Result<_, String> {
        let workspace = config.workspace()?;
        Ok(sync::Io {
            term,
            out: ui::Stdout,
            input: std::io::stdin().lock(),
            host: host(&workspace),
            workspace,
            path: std::env::var_os("PATH"),
            config,
        })
    };
    match command {
        Command::Help(topic) => {
            print!("{}", topic.map_or_else(help::general, help::page));
            Ok(true)
        }
        Command::Init { path } => init::run(path.as_deref(), &mut ui::Stdout, term).map(|_| true),
        Command::Sync { environment, yes, locked, wait } => {
            let io = sync_io(Config::from_env()?)?;
            sync::run(&sync::Options { environment, yes, locked, wait }, io).map(|o| o != sync::Outcome::Cancelled)
        }
        Command::Install { packages, environment, yes, wait } => {
            let io = sync_io(Config::from_env()?)?;
            install::run(&install::Options { packages, environment, yes, wait }, io)
                .map(|o| o != sync::Outcome::Cancelled)
        }
        Command::Remove { packages, environment, yes, wait } => {
            let io = sync_io(Config::from_env()?)?;
            remove::run(&remove::Options { packages, environment, yes, wait }, io)
                .map(|o| o != sync::Outcome::Cancelled)
        }
        Command::Status { environment } => {
            let config = Config::from_env()?;
            let workspace = config.workspace()?;
            status::run(&environment, &config, &mut ui::Stdout, term, &workspace, &host(&workspace)).map(|_| true)
        }
        Command::Clean { yes, sources, cache, dry_run, wait } => {
            let options = clean::Options { yes, sources, cache, dry_run, wait };
            let workspace = Config::from_env()?.workspace()?;
            clean::run(options, &mut ui::Stdout, &mut std::io::stdin().lock(), term, &workspace)
                .map(|o| o != clean::Outcome::Cancelled)
        }
        Command::Purge { yes, dry_run, wait } => {
            let options = purge::Options { yes, dry_run, wait };
            let workspace = Config::from_env()?.workspace()?;
            purge::run(options, &mut ui::Stdout, &mut std::io::stdin().lock(), term, &workspace)
                .map(|o| o != purge::Outcome::Cancelled)
        }
        Command::Reset { yes, keep_cache, dry_run, wait } => {
            let options = reset::Options { yes, keep_cache, dry_run, wait };
            let workspace = Config::from_env()?.workspace()?;
            reset::run(options, &mut ui::Stdout, &mut std::io::stdin().lock(), term, &workspace)
                .map(|o| o != reset::Outcome::Cancelled)
        }
    }
}
