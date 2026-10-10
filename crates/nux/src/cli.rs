//! Command-line argument parsing.

use std::path::PathBuf;

use crate::environment::Selection;
use crate::help::Topic;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// The general help, or a command's.
    Help(Option<Topic>),
    Init { path: Option<PathBuf> },
    Sync { environment: Selection, yes: bool, locked: bool, wait: bool },
    Status { environment: Selection },
    /// Each package with its requirement, if one was given.
    Install { packages: Vec<(String, Option<String>)>, environment: Selection, yes: bool, wait: bool },
    Remove { packages: Vec<String>, environment: Selection, yes: bool, wait: bool },
    Clean { yes: bool, sources: bool, cache: bool, dry_run: bool, wait: bool },
    Purge { yes: bool, dry_run: bool, wait: bool },
    Reset { yes: bool, keep_cache: bool, dry_run: bool, wait: bool },
}

/// A usage error; the message is meant to be followed by [`CliError::hint`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliError {
    pub message: String,
    /// The command whose arguments were wrong, if any.
    pub command: Option<Topic>,
}

impl CliError {
    fn new(message: impl Into<String>) -> Self {
        Self { message: message.into(), command: None }
    }

    /// Where to read about correct usage.
    pub fn hint(&self) -> String {
        match self.command {
            Some(topic) => format!("Run `nux help {}` for usage.", topic.name()),
            None => "Run `nux help` for usage.".into(),
        }
    }
}

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// Parses the arguments that follow the program name.
///
/// `nux help [command]`, `nux --help [command]` and `nux <command> --help`
/// (`-h` anywhere among the command's arguments) ask for help.
pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Command, CliError> {
    let mut args = args.into_iter();
    let Some(command) = args.next() else {
        return Err(CliError::new("no command given"));
    };
    if matches!(command.as_str(), "help" | "-h" | "--help") {
        return match (args.next(), args.next()) {
            (None, _) => Ok(Command::Help(None)),
            (Some(name), None) => match Topic::from_name(&name) {
                Some(topic) => Ok(Command::Help(Some(topic))),
                None => {
                    let commands: Vec<_> = Topic::ALL.iter().map(|t| t.name()).collect();
                    Err(CliError::new(format!("no help for `{name}`, which is not a command (commands: {})", commands.join(", "))))
                }
            },
            (Some(_), Some(extra)) => Err(CliError { command: Some(Topic::Help), ..CliError::new(format!("unexpected argument `{extra}`")) }),
        };
    }
    let Some(topic) = Topic::from_name(&command) else {
        return Err(CliError::new(if command.starts_with('-') {
            format!("unknown option `{command}`")
        } else {
            format!("unknown command `{command}`")
        }));
    };
    let args: Vec<String> = args.collect();
    if args.iter().any(|arg| arg == "-h" || arg == "--help") {
        return Ok(Command::Help(Some(topic)));
    }
    let args = args.into_iter();
    let parsed = match topic {
        Topic::Init => parse_init(args),
        Topic::Sync => parse_sync(args),
        Topic::Status => parse_status(args),
        Topic::Install => parse_install(args),
        Topic::Remove => parse_remove(args),
        Topic::Clean => parse_clean(args),
        Topic::Purge => parse_purge(args),
        Topic::Reset => parse_reset(args),
        Topic::Help => unreachable!("handled above"),
    };
    parsed.map_err(|error| CliError { command: Some(topic), ..error })
}

fn parse_init(args: impl Iterator<Item = String>) -> Result<Command, CliError> {
    let mut path = None;
    for arg in args {
        match arg.as_str() {
            _ if arg.starts_with('-') => return Err(unexpected(&arg, "init")),
            _ if path.is_some() => return Err(unexpected(&arg, "init")),
            _ => path = Some(PathBuf::from(arg)),
        }
    }
    Ok(Command::Init { path })
}

fn parse_sync(mut args: impl Iterator<Item = String>) -> Result<Command, CliError> {
    let mut environment = Selection::default();
    let (mut yes, mut locked, mut wait) = (false, false, false);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-y" | "--yes" => yes = true,
            "--locked" => locked = true,
            "--wait" => wait = true,
            _ if environment_option(&arg, &mut args, &mut environment)? => {}
            _ => return Err(unexpected(&arg, "sync")),
        }
    }
    Ok(Command::Sync { environment, yes, locked, wait })
}

fn parse_status(mut args: impl Iterator<Item = String>) -> Result<Command, CliError> {
    let mut environment = Selection::default();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            _ if environment_option(&arg, &mut args, &mut environment)? => {}
            _ => return Err(unexpected(&arg, "status")),
        }
    }
    Ok(Command::Status { environment })
}

/// What `install` and `remove` take.
struct EditArgs {
    packages: Vec<String>,
    environment: Selection,
    yes: bool,
    wait: bool,
}

/// Parses `install` and `remove`.
fn parse_edit(mut args: impl Iterator<Item = String>, command: &str) -> Result<EditArgs, CliError> {
    let mut environment = Selection::default();
    let (mut packages, mut yes, mut wait) = (Vec::new(), false, false);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-y" | "--yes" => yes = true,
            "--wait" => wait = true,
            _ if environment_option(&arg, &mut args, &mut environment)? => {}
            _ if arg.starts_with('-') => return Err(unexpected(&arg, command)),
            _ if packages.contains(&arg) => return Err(CliError::new(format!("`{arg}` is given more than once"))),
            _ => packages.push(arg),
        }
    }
    if packages.is_empty() {
        return Err(CliError::new(format!("`{command}` expects at least one package")));
    }
    Ok(EditArgs { packages, environment, yes, wait })
}

fn parse_install(args: impl Iterator<Item = String>) -> Result<Command, CliError> {
    let EditArgs { packages, environment, yes, wait } = parse_edit(args, "install")?;
    let mut parsed: Vec<(String, Option<String>)> = Vec::new();
    for package in packages {
        let (name, requirement) = match package.split_once('@') {
            Some((name, requirement)) => (name.to_string(), Some(requirement.trim().to_string())),
            None => (package.clone(), None),
        };
        if name.is_empty() {
            return Err(CliError::new(format!("`{package}` has no package name before `@`")));
        }
        if requirement.as_deref() == Some("") {
            return Err(CliError::new(format!("`{package}` expects a version requirement after `@`")));
        }
        if parsed.iter().any(|(n, _)| *n == name) {
            return Err(CliError::new(format!("`{name}` is given more than once")));
        }
        parsed.push((name, requirement));
    }
    Ok(Command::Install { packages: parsed, environment, yes, wait })
}

fn parse_remove(args: impl Iterator<Item = String>) -> Result<Command, CliError> {
    let EditArgs { packages, environment, yes, wait } = parse_edit(args, "remove")?;
    if let Some(package) = packages.iter().find(|p| p.contains('@')) {
        return Err(CliError::new(format!("`{package}`: `remove` takes package names, without a version")));
    }
    Ok(Command::Remove { packages, environment, yes, wait })
}

fn parse_clean(args: impl Iterator<Item = String>) -> Result<Command, CliError> {
    let (mut yes, mut sources, mut cache, mut dry_run, mut wait) = (false, false, false, false, false);
    for arg in args {
        match arg.as_str() {
            "-y" | "--yes" => yes = true,
            "--sources" => sources = true,
            "--cache" => cache = true,
            "--dry-run" => dry_run = true,
            "--wait" => wait = true,
            _ => return Err(unexpected(&arg, "clean")),
        }
    }
    Ok(Command::Clean { yes, sources, cache, dry_run, wait })
}

fn parse_purge(args: impl Iterator<Item = String>) -> Result<Command, CliError> {
    let (mut yes, mut dry_run, mut wait) = (false, false, false);
    for arg in args {
        match arg.as_str() {
            "-y" | "--yes" => yes = true,
            "--dry-run" => dry_run = true,
            "--wait" => wait = true,
            _ => return Err(unexpected(&arg, "purge")),
        }
    }
    Ok(Command::Purge { yes, dry_run, wait })
}

fn parse_reset(args: impl Iterator<Item = String>) -> Result<Command, CliError> {
    let (mut yes, mut keep_cache, mut dry_run, mut wait) = (false, false, false, false);
    for arg in args {
        match arg.as_str() {
            "-y" | "--yes" => yes = true,
            "--keep-cache" => keep_cache = true,
            "--dry-run" => dry_run = true,
            "--wait" => wait = true,
            _ => return Err(unexpected(&arg, "reset")),
        }
    }
    Ok(Command::Reset { yes, keep_cache, dry_run, wait })
}

fn unexpected(arg: &str, command: &str) -> CliError {
    CliError::new(format!("unexpected argument `{arg}` for `{command}`"))
}

/// Handles `-g`, `--global`, `-e <path>`, `--environment <path>` and
/// `--environment=<path>`. Returns whether `arg` was one of them.
fn environment_option(
    arg: &str,
    args: &mut impl Iterator<Item = String>,
    environment: &mut Selection,
) -> Result<bool, CliError> {
    let both = || CliError::new("`--global` and `--environment` cannot be used together");
    if arg == "-g" || arg == "--global" {
        if environment.path.is_some() {
            return Err(both());
        }
        environment.global = true;
        return Ok(true);
    }
    let value = match arg {
        "-e" | "--environment" => args.next().ok_or_else(|| CliError::new(format!("`{arg}` expects a path")))?,
        _ => match arg.strip_prefix("--environment=") {
            Some(value) => value.to_string(),
            None => return Ok(false),
        },
    };
    if value.is_empty() {
        return Err(CliError::new(format!("`{arg}` expects a path")));
    }
    if environment.global {
        return Err(both());
    }
    if environment.path.replace(PathBuf::from(value)).is_some() {
        return Err(CliError::new("the environment is given more than once"));
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parses `args`; errors are their message.
    fn run(args: &[&str]) -> Result<Command, String> {
        parse(args.iter().map(|a| a.to_string())).map_err(|e| e.message)
    }

    fn global() -> Selection {
        Selection { path: None, global: true }
    }

    fn env(path: Option<&str>) -> Selection {
        Selection { path: path.map(PathBuf::from), global: false }
    }

    fn sync(path: Option<&str>) -> Result<Command, String> {
        Ok(Command::Sync { environment: env(path), yes: false, locked: false, wait: false })
    }

    fn err(message: &str) -> Result<Command, String> {
        Err(message.into())
    }

    #[test]
    fn help() {
        for args in [&["help"][..], &["-h"], &["--help"]] {
            assert_eq!(run(args), Ok(Command::Help(None)), "{args:?}");
        }
        let sync_help = Ok(Command::Help(Some(Topic::Sync)));
        for args in [&["help", "sync"][..], &["--help", "sync"], &["sync", "--help"], &["sync", "-e", "x", "-h"]] {
            assert_eq!(run(args), sync_help, "{args:?}");
        }
        // Help wins over arguments that are wrong.
        assert_eq!(run(&["sync", "--bogus", "-h"]), sync_help);
        assert_eq!(run(&["install", "--help"]), Ok(Command::Help(Some(Topic::Install))));
        assert_eq!(run(&["help", "help"]), Ok(Command::Help(Some(Topic::Help))));
        assert_eq!(
            run(&["help", "build"]),
            err("no help for `build`, which is not a command (commands: init, sync, status, install, remove, clean, purge, reset, help)")
        );
        assert_eq!(run(&["help", "sync", "now"]), err("unexpected argument `now`"));
    }

    #[test]
    fn usage_errors_point_at_the_right_help() {
        let hint = |args: &[&str]| parse(args.iter().map(|a| a.to_string())).unwrap_err().hint();
        assert_eq!(hint(&["sync", "--bogus"]), "Run `nux help sync` for usage.");
        assert_eq!(hint(&["install"]), "Run `nux help install` for usage.");
        assert_eq!(hint(&["build"]), "Run `nux help` for usage.");
        assert_eq!(hint(&[]), "Run `nux help` for usage.");
    }

    #[test]
    fn sync_environment() {
        assert_eq!(run(&["sync"]), sync(None));
        assert_eq!(run(&["sync", "-e", "dev"]), sync(Some("dev")));
        assert_eq!(run(&["sync", "--environment", "a/nux.toml"]), sync(Some("a/nux.toml")));
        assert_eq!(run(&["sync", "--environment=dev"]), sync(Some("dev")));
        let yes = Ok(Command::Sync { environment: env(Some("dev")), yes: true, locked: false, wait: false });
        assert_eq!(run(&["sync", "-y", "-e", "dev"]), yes);
        assert_eq!(run(&["sync", "-e", "dev", "--yes"]), yes);
        assert_eq!(
            run(&["sync", "--locked", "--wait"]),
            Ok(Command::Sync { environment: env(None), yes: false, locked: true, wait: true })
        );
        let global = Ok(Command::Sync { environment: global(), yes: false, locked: false, wait: false });
        assert_eq!(run(&["sync", "-g"]), global);
        assert_eq!(run(&["sync", "--global"]), global);
    }

    #[test]
    fn install_and_remove() {
        let install = |packages: &[(&str, Option<&str>)], environment, yes, wait| {
            let packages = packages.iter().map(|(n, r)| (n.to_string(), r.map(str::to_string))).collect();
            Ok(Command::Install { packages, environment, yes, wait })
        };
        assert_eq!(run(&["install", "jq"]), install(&[("jq", None)], env(None), false, false));
        assert_eq!(
            run(&["install", "jq@~1.7", "-g", "cmake@>=4, <5", "-y", "--wait"]),
            install(&[("jq", Some("~1.7")), ("cmake", Some(">=4, <5"))], global(), true, true)
        );
        assert_eq!(run(&["install", "-e", "dev", "jq"]), install(&[("jq", None)], env(Some("dev")), false, false));
        assert_eq!(run(&["install"]), err("`install` expects at least one package"));
        assert_eq!(run(&["install", "-g"]), err("`install` expects at least one package"));
        assert_eq!(run(&["install", "jq@"]), err("`jq@` expects a version requirement after `@`"));
        assert_eq!(run(&["install", "@1"]), err("`@1` has no package name before `@`"));
        assert_eq!(run(&["install", "jq", "jq@1"]), err("`jq` is given more than once"));
        assert_eq!(run(&["install", "jq", "--locked"]), err("unexpected argument `--locked` for `install`"));

        let remove = |packages: &[&str], environment| {
            let packages = packages.iter().map(|p| p.to_string()).collect();
            Ok(Command::Remove { packages, environment, yes: false, wait: false })
        };
        assert_eq!(run(&["remove", "jq", "cmake"]), remove(&["jq", "cmake"], env(None)));
        assert_eq!(run(&["remove", "--global", "jq"]), remove(&["jq"], global()));
        assert_eq!(run(&["remove"]), err("`remove` expects at least one package"));
        assert_eq!(run(&["remove", "jq", "jq"]), err("`jq` is given more than once"));
        assert_eq!(run(&["remove", "jq@1"]), err("`jq@1`: `remove` takes package names, without a version"));
    }

    #[test]
    fn init_status_and_clean() {
        assert_eq!(run(&["init"]), Ok(Command::Init { path: None }));
        assert_eq!(run(&["init", "envs/school"]), Ok(Command::Init { path: Some("envs/school".into()) }));
        assert_eq!(run(&["init", "-h"]), Ok(Command::Help(Some(Topic::Init))));
        assert_eq!(run(&["init", "a", "b"]), err("unexpected argument `b` for `init`"));
        assert_eq!(run(&["init", "--force"]), err("unexpected argument `--force` for `init`"));

        assert_eq!(run(&["status"]), Ok(Command::Status { environment: env(None) }));
        assert_eq!(run(&["status", "--environment=dev"]), Ok(Command::Status { environment: env(Some("dev")) }));
        assert_eq!(run(&["status", "-g"]), Ok(Command::Status { environment: global() }));
        assert_eq!(run(&["status", "-y"]), err("unexpected argument `-y` for `status`"));

        let clean = |yes, sources, cache, dry_run, wait| Ok(Command::Clean { yes, sources, cache, dry_run, wait });
        assert_eq!(run(&["clean"]), clean(false, false, false, false, false));
        assert_eq!(run(&["clean", "--sources", "-y", "--dry-run", "--wait"]), clean(true, true, false, true, true));
        assert_eq!(run(&["clean", "--cache"]), clean(false, false, true, false, false));
        assert_eq!(run(&["clean", "--locked"]), err("unexpected argument `--locked` for `clean`"));
        assert_eq!(run(&["clean", "-e", "x"]), err("unexpected argument `-e` for `clean`"));

        let purge = |yes, dry_run, wait| Ok(Command::Purge { yes, dry_run, wait });
        assert_eq!(run(&["purge"]), purge(false, false, false));
        assert_eq!(run(&["purge", "-y", "--dry-run", "--wait"]), purge(true, true, true));
        assert_eq!(run(&["purge", "--sources"]), err("unexpected argument `--sources` for `purge`"));

        let reset = |yes, keep_cache, dry_run, wait| Ok(Command::Reset { yes, keep_cache, dry_run, wait });
        assert_eq!(run(&["reset"]), reset(false, false, false, false));
        assert_eq!(run(&["reset", "-y", "--keep-cache", "--dry-run", "--wait"]), reset(true, true, true, true));
        assert_eq!(run(&["reset", "-e", "x"]), err("unexpected argument `-e` for `reset`"));
    }

    #[test]
    fn usage_errors() {
        assert_eq!(run(&[]), err("no command given"));
        assert_eq!(run(&["build"]), err("unknown command `build`"));
        assert_eq!(run(&["--verbose"]), err("unknown option `--verbose`"));
        assert_eq!(run(&["sync", "-e"]), err("`-e` expects a path"));
        assert_eq!(run(&["sync", "--environment="]), err("`--environment=` expects a path"));
        assert_eq!(run(&["sync", "-e", "a", "-e", "b"]), err("the environment is given more than once"));
        assert_eq!(run(&["sync", "extra"]), err("unexpected argument `extra` for `sync`"));
        let both = err("`--global` and `--environment` cannot be used together");
        assert_eq!(run(&["sync", "-g", "-e", "a"]), both);
        assert_eq!(run(&["status", "--environment=a", "--global"]), both);
    }
}
