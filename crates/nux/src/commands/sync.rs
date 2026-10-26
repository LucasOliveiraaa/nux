//! `nux sync`: load an environment, resolve it, then let `nux-build`
//! download, build and link what is missing.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use nux_build::{BuildPlan, Builder, Isolation, LinkStatus, SourceStatus, Workspace};
use nux_eval::Host;
use nux_log::{Style, Term};
use nux_resolver::{PackageVersionId, ResolveError, ResolvedPackage};

use crate::commands::lock_workspace;
use crate::config::Config;
use crate::environment::{Environment, Selection};
use crate::lockfile::Lock;
use crate::repository;
use crate::ui::{SyncReporter, confirm, count, write_header, write_packages};

#[derive(Debug, Clone, Default)]
pub struct Options {
    pub environment: Selection,
    /// Do not ask for confirmation.
    pub yes: bool,
    /// Fail instead of creating or changing `Nux.lock`.
    pub locked: bool,
    /// Wait for another process using the workspace instead of failing.
    pub wait: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Synced,
    UpToDate,
    Cancelled,
}

/// Everything `sync` reads and writes besides the filesystem.
pub struct Io<W, R> {
    pub term: Term,
    pub out: W,
    pub input: R,
    pub workspace: Workspace,
    /// `$PATH`, to tell whether `bin/` still needs adding to it.
    pub path: Option<std::ffi::OsString>,
    /// What recipes see of the host (`host_bin`, `use_host`...).
    pub host: Host,
    pub config: Config,
}

/// A change to the environment file (from `nux install` or `nux remove`),
/// written only once the sync goes ahead: after the environment resolves
/// and the user confirms.
#[derive(Debug, Clone)]
pub struct Edit {
    pub path: PathBuf,
    /// The new contents; `None` when the file stays as it is.
    pub text: Option<String>,
    /// Describes the change; printed before resolving.
    pub message: String,
}

impl Edit {
    /// Writes the file, reporting it with `indent`.
    fn save(&self, out: &mut impl Write, indent: &str) -> Result<(), String> {
        let Some(text) = &self.text else {
            return Ok(());
        };
        nux_build::write_atomic(&self.path, text.as_bytes()).map_err(|e| e.to_string())?;
        writeln!(out, "\n{indent}Updated `{}`.", self.path.display())
            .map_err(|e| format!("cannot write output: {e}"))
    }
}

/// Errors are returned as messages to print after `error: `.
pub fn run<W, R>(options: &Options, io: Io<W, R>) -> Result<Outcome, String>
where
    W: Write + Send + Clone,
    R: BufRead,
{
    run_with(options, io, |config| {
        Ok((Environment::select(&options.environment, config)?, None))
    })
}

/// Syncs the environment `prepare` loads, saving its [`Edit`], if any, once
/// the sync goes ahead.
pub fn run_with<W, R>(
    options: &Options,
    io: Io<W, R>,
    prepare: impl FnOnce(&Config) -> Result<(Environment, Option<Edit>), String>,
) -> Result<Outcome, String>
where
    W: Write + Send + Clone,
    R: BufRead,
{
    let Io {
        term,
        mut out,
        mut input,
        workspace,
        path,
        host,
        config,
    } = io;
    let w = |e: std::io::Error| format!("cannot write output: {e}");

    write_header(&mut out, term).map_err(w)?;
    let (env, edit) = prepare(&config)?;
    let global = if env.global { " (global)" } else { "" };
    writeln!(
        out,
        "Environment: {}{global}\n",
        term.paint(Style::Emphasis, &env.name)
    )
    .map_err(w)?;
    if let Some(edit) = &edit {
        writeln!(out, "{}\n", edit.message).map_err(w)?;
    }

    let lock_path = Lock::path(&env);
    let old_lock = Lock::read(&lock_path)?;
    if options.locked && old_lock.is_none() {
        return Err(format!(
            "`{}` does not exist; run `nux sync` without `--locked` to create it",
            lock_path.display()
        ));
    }

    writeln!(out, "Resolving dependencies...").map_err(w)?;
    let (packages, requested) = resolve_environment(&env, &host, old_lock.as_ref(), config.jobs)?;

    // From here on the workspace is read and changed: one process at a time.
    let _workspace_lock = lock_workspace(&workspace, options.wait, &mut out)?;
    writeln!(out, "Checking installed packages...").map_err(w)?;
    let workspace_existed = workspace.exists();
    let pins = old_lock
        .as_ref()
        .map(|lock| lock.pins(&packages))
        .unwrap_or_default();
    let mut builder = Builder::new(workspace.home())
        .with_local_root(env.root())
        .with_pins(pins)
        .with_sandbox(config.sandbox)
        .with_parallel_builds(config.parallel_builds);
    let plan = builder.plan(packages, &requested);
    if options.locked
        && let Some(old) = &old_lock
        && let Some(difference) = old.difference(&Lock::from_plan(&plan, &workspace, Some(old)))
    {
        return Err(format!(
            "`{}` is out of date: {difference}; run `nux sync` without `--locked` to update it",
            lock_path.display()
        ));
    }
    // Writes `Nux.lock` when the resolution or the sources changed.
    let update_lock = |plan: &BuildPlan| -> Result<bool, String> {
        let new = Lock::from_plan(plan, &workspace, old_lock.as_ref());
        if options.locked || old_lock.as_ref() == Some(&new) {
            return Ok(false);
        }
        new.write(&lock_path)?;
        Ok(true)
    };
    for warning in &plan.warnings {
        writeln!(out, "{} {warning}", term.paint(Style::Warning, "warning:")).map_err(w)?;
    }
    // Links into what this environment no longer needs, and nothing else does.
    let dropped = workspace
        .dropped_versions(&env.name, &plan)
        .map_err(|e| e.to_string())?;
    let unused_links = workspace.links_into(&dropped, &plan);
    if let Some(conflict) = plan.conflicts().next() {
        // Checked before any work, so nothing is built in vain.
        let LinkStatus::Conflict(reason) = &conflict.status else {
            unreachable!()
        };
        return Err(format!(
            "cannot link `{}`: `{}` already exists and {reason}",
            conflict.name,
            workspace.bin_dir().join(&conflict.name).display()
        ));
    }

    if plan.is_up_to_date() {
        if let Some(edit) = &edit {
            edit.save(&mut out, "")?;
        }
        builder
            .record_environment(&env.name, &env.path, &plan)
            .map_err(|e| e.to_string())?;
        if update_lock(&plan)? {
            writeln!(out, "\nUpdated `{}`.", lock_path.display()).map_err(w)?;
        }
        if unused_links.is_empty() {
            writeln!(
                out,
                "\n{}",
                term.paint(Style::Success, "Environment is already up to date.")
            )
            .map_err(w)?;
            return Ok(Outcome::UpToDate);
        }
        writeln!(out).map_err(w)?;
        remove_links(&workspace, &unused_links, &mut out)?;
        writeln!(
            out,
            "\n{}",
            term.paint(Style::Success, "Nux finished syncing the environment.")
        )
        .map_err(w)?;
        write_purge_hint(&dropped, term, &mut out)?;
        return Ok(Outcome::Synced);
    }

    let to_build: Vec<_> = plan.to_build().collect();
    if !to_build.is_empty() {
        // Before asking: a required sandbox that cannot be created stops here.
        if let Isolation::Unavailable(reason) = builder.isolation().map_err(|e| e.to_string())? {
            writeln!(
                out,
                "{} builds will not be sandboxed: {reason}. Set `mode = \"off\"` under `[sandbox]` in config.toml to silence this.",
                term.paint(Style::Warning, "warning:")
            )
            .map_err(w)?;
        }
        writeln!(out, "\nThe environment requests the following packages:").map_err(w)?;
        write_packages(
            &mut out,
            plan.steps
                .iter()
                .filter(|s| s.requested)
                .map(|s| (s.package.name.as_str(), s.package.version.as_str())),
        )
        .map_err(w)?;
        let hosted: Vec<_> = plan
            .steps
            .iter()
            .filter_map(|s| Some((s, s.host()?)))
            .collect();
        if !hosted.is_empty() {
            writeln!(out, "\nThe following packages are provided by the host:").map_err(w)?;
            let width = hosted
                .iter()
                .map(|(s, _)| s.package.name.chars().count())
                .max()
                .unwrap_or(0);
            let version_width = hosted
                .iter()
                .map(|(s, _)| s.package.version.chars().count())
                .max()
                .unwrap_or(0);
            for (step, path) in hosted {
                writeln!(
                    out,
                    "  {:<width$} {:<version_width$} {}",
                    step.package.name,
                    step.package.version,
                    path.display()
                )
                .map_err(w)?;
            }
        }
        writeln!(out, "\nThe following packages will be built:").map_err(w)?;
        write_packages(
            &mut out,
            to_build
                .iter()
                .map(|s| (s.package.name.as_str(), s.package.version.as_str())),
        )
        .map_err(w)?;
        writeln!(
            out,
            "\n{}, {}.",
            term.paint(
                Style::Emphasis,
                format!("{} to build", count(to_build.len(), "package"))
            ),
            term.paint(
                Style::Emphasis,
                format!("{} to download", count(plan.downloads(), "source"))
            ),
        )
        .map_err(w)?;

        if !options.yes && !confirm(&mut out, &mut input)? {
            writeln!(out, "\nSync cancelled; nothing was changed.").map_err(w)?;
            return Ok(Outcome::Cancelled);
        }
    }

    if let Some(edit) = &edit {
        edit.save(&mut out, "  ")?;
    }
    if !workspace_existed {
        writeln!(out, "\nNo Nux workspace found, creating one...").map_err(w)?;
    }
    workspace.create().map_err(|e| e.to_string())?;

    let names = plan.steps.iter().map(|s| s.package.name.as_str());
    let reporter = SyncReporter::new(term, names, out.clone(), out.clone());
    if !to_build.is_empty() {
        download(&builder, &plan, &reporter, term, &mut out)?;
    }
    // Sources are now downloaded and verified: their identities are known.
    if update_lock(&plan)? {
        writeln!(out, "\n  Updated `{}`.", lock_path.display()).map_err(w)?;
    }
    if !to_build.is_empty() {
        build(&mut builder, &plan, &reporter, term, &mut out)?;
    }

    writeln!(out, "\n  Updating environment...").map_err(w)?;
    builder
        .record_environment(&env.name, &env.path, &plan)
        .map_err(|e| e.to_string())?;
    writeln!(out, "  Updating executable links...").map_err(w)?;
    builder.link_executables(&plan).map_err(|e| e.to_string())?;
    if !unused_links.is_empty() {
        remove_links(&workspace, &unused_links, &mut out)?;
    }

    writeln!(
        out,
        "\n{}",
        term.paint(Style::Success, "Nux finished syncing the environment.")
    )
    .map_err(w)?;
    write_purge_hint(&dropped, term, &mut out)?;
    if !plan.links.is_empty() && !on_path(&workspace.bin_dir(), path.as_deref()) {
        let bin = bin_for_shell(&workspace);
        writeln!(
            out,
            "\n{}\nAdd the following line to your shell's rc file:\n\n  export PATH=\"{bin}:$PATH\"",
            term.paint(Style::Warning, format!("You have not added {bin} to your PATH.")),
        )
        .map_err(w)?;
    }
    Ok(Outcome::Synced)
}

/// Removes links to versions the environment no longer needs.
fn remove_links(
    workspace: &Workspace,
    links: &[nux_build::UnusedLink],
    out: &mut impl Write,
) -> Result<(), String> {
    workspace.remove_links(links).map_err(|e| e.to_string())?;
    let names: Vec<_> = links.iter().map(|l| l.name.as_str()).collect();
    writeln!(
        out,
        "  Removed {}: {}",
        if names.len() == 1 { "link" } else { "links" },
        names.join(", ")
    )
    .map_err(|e| format!("cannot write output: {e}"))
}

/// Points at `nux purge` when versions are left unused.
fn write_purge_hint(
    dropped: &[(String, String)],
    term: Term,
    out: &mut impl Write,
) -> Result<(), String> {
    if dropped.is_empty() {
        return Ok(());
    }
    let list: Vec<_> = dropped
        .iter()
        .map(|(name, version)| format!("{name} {version}"))
        .collect();
    let (are, them) = if list.len() == 1 {
        ("is", "it")
    } else {
        ("are", "them")
    };
    writeln!(
        out,
        "\n{} {are} no longer used by any environment; run `{}` to remove {them}.",
        list.join(", "),
        term.paint(Style::Emphasis, "nux purge")
    )
    .map_err(|e| format!("cannot write output: {e}"))
}

/// How to write the bin directory in a shell rc file.
fn bin_for_shell(workspace: &Workspace) -> String {
    if std::env::var_os("NUX_HOME").is_some_and(|h| !h.is_empty()) {
        return "$NUX_HOME/bin".into();
    }
    let home = std::env::var_os("HOME").map(PathBuf::from);
    match home.as_deref().and_then(|home| {
        workspace
            .bin_dir()
            .strip_prefix(home)
            .ok()
            .map(Path::to_path_buf)
    }) {
        Some(relative) => format!("$HOME/{}", relative.display()),
        None => workspace.bin_dir().display().to_string(),
    }
}

/// Downloads the missing sources, with progress.
fn download<W: Write + Send + Clone>(
    builder: &Builder,
    plan: &BuildPlan,
    reporter: &SyncReporter<W>,
    term: Term,
    out: &mut W,
) -> Result<(), String> {
    let w = |e: std::io::Error| format!("cannot write output: {e}");
    if !plan
        .to_build()
        .any(|s| matches!(s.source, SourceStatus::Cached | SourceStatus::Missing))
    {
        return Ok(());
    }
    writeln!(out).map_err(w)?;
    let started = Instant::now();
    let downloaded = builder
        .download(plan, reporter)
        .map_err(|e| e.to_string())?;
    if downloaded > 0 {
        writeln!(
            out,
            "\n  {} {} in {}",
            term.paint(Style::Success, "Finished downloading"),
            count(downloaded, "source"),
            nux_log::duration(started.elapsed())
        )
        .map_err(w)?;
    }
    Ok(())
}

/// Builds what is not installed, with progress.
fn build<W: Write + Send + Clone>(
    builder: &mut Builder,
    plan: &BuildPlan,
    reporter: &SyncReporter<W>,
    term: Term,
    out: &mut W,
) -> Result<(), String> {
    let w = |e: std::io::Error| format!("cannot write output: {e}");
    writeln!(out).map_err(w)?;
    let started = Instant::now();
    let built = builder.build(plan, reporter).map_err(|e| e.to_string())?;
    writeln!(
        out,
        "\n  {} {} in {}",
        term.paint(Style::Success, "Finished building"),
        count(built, "package"),
        nux_log::duration(started.elapsed())
    )
    .map_err(w)
}

/// Loads the environment's repositories and resolves its packages: the build
/// order and the versions selected for the requests.
/// With a `lock`, its versions are kept while they still match. `jobs`
/// recipes are evaluated at the same time.
pub fn resolve_environment(
    env: &Environment,
    host: &Host,
    lock: Option<&Lock>,
    jobs: usize,
) -> Result<(Vec<ResolvedPackage>, Vec<PackageVersionId>), String> {
    let mut resolver = repository::load(&env.repositories, host, jobs)?;
    if let Some(lock) = lock {
        lock.prefer_in(&mut resolver);
    }
    let packages = resolver
        .resolve(&env.packages)
        .map_err(|e| explain(e, env))?;
    let requested = env
        .packages
        .iter()
        .map(|r| {
            resolver
                .select(&r.name, &r.version)
                .expect("resolve selected it already")
        })
        .collect();
    Ok((packages, requested))
}

pub fn on_path(bin: &Path, path: Option<&std::ffi::OsStr>) -> bool {
    let bin = std::fs::canonicalize(bin).unwrap_or_else(|_| bin.to_path_buf());
    path.is_some_and(|path| {
        std::env::split_paths(path).any(|p| std::fs::canonicalize(&p).unwrap_or(p) == bin)
    })
}

/// Adds where definitions were looked for to "package is not defined" errors.
fn explain(err: ResolveError, env: &Environment) -> String {
    let unknown = match &err {
        ResolveError::UnknownPackage { .. } => true,
        ResolveError::InDependency { error, .. } => {
            matches!(**error, ResolveError::UnknownPackage { .. })
        }
        _ => false,
    };
    if !unknown {
        return err.to_string();
    }
    let searched: Vec<_> = env
        .repositories
        .iter()
        .map(|r| format!("`{}`", r.display()))
        .collect();
    format!("{err} (searched: {})", searched.join(", "))
}
