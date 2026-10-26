//! Downloads, builds and installs resolved packages into a Nux workspace.
//!
//! A sync runs in stages, each of which can be called on its own:
//!
//! 1. [`Builder::plan`] compares the resolved build order with the
//!    workspace: what is installed, what is downloaded, which links exist.
//! 2. [`Builder::download`] fetches missing sources, several at a time.
//! 3. [`Builder::build`] builds what is not installed, dependencies first.
//! 4. [`Builder::link_executables`] points `bin/` at the `set_bin` targets.
//! 5. [`Builder::record_environment`] records the synced environment.
//!
//! [`Workspace::clean`] and [`Workspace::purge`] remove what is not needed,
//! and [`Workspace::reset`] everything.
//!
//! Progress is reported through a [`Reporter`]; this crate prints nothing.
//!
//! Build commands run in a [sandbox](nux_firebox) unless the
//! [`SandboxPolicy`] says otherwise: the host is read-only, personal files
//! are hidden, and only the package's `build/` and `install/` directories
//! can be written. See [`Builder::isolation`].

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use nux_executor::{Access, Executor, Sandbox};
use nux_resolver::{PackageVersionId, ResolvedPackage};
use serde_json::json;

mod cache;
mod clean;
mod env;
mod fetch;
mod plan;
mod purge;
mod reset;
mod state;
mod workspace;

pub use clean::{CleanItem, CleanKind, CleanOptions};
pub use plan::{BuildPlan, Link, LinkStatus, SourceStatus, Step};
pub use purge::{EnvironmentRecord, UnusedLink};
pub use reset::ResetOptions;
pub use state::{SourceIdentity, write_atomic};
pub use workspace::{PackagePaths, Workspace, WorkspaceLock};

use env::DependencyExports;
use state::{InstallRecord, Metadata};

/// How many sources are downloaded at the same time.
const PARALLEL_DOWNLOADS: usize = 4;
/// How many packages are built at the same time unless configured.
pub const DEFAULT_PARALLEL_BUILDS: usize = 4;
/// How many lines of `build.log` a failed build error includes.
const LOG_TAIL: usize = 15;

#[derive(Debug)]
pub enum BuildError {
    /// Neither `NUX_HOME` nor `HOME` is set.
    NoHome,
    Io {
        context: String,
        error: std::io::Error,
    },
    Download {
        package: String,
        version: String,
        message: String,
    },
    /// Several downloads failed; each is reported separately.
    Downloads(Vec<BuildError>),
    LocalSource {
        package: String,
        version: String,
        message: String,
    },
    Command(Box<CommandFailure>),
    /// The `set_bin` target was not produced by the build.
    Bin {
        package: String,
        version: String,
        path: PathBuf,
        reason: String,
    },
    /// An `export_lib` library was not produced by the build.
    Library {
        package: String,
        version: String,
        library: String,
        reason: String,
    },
    LinkConflict {
        path: PathBuf,
        reason: String,
    },
    /// A clean item failed a safeguard.
    Clean {
        path: PathBuf,
        reason: String,
    },
    /// Another process holds the workspace lock.
    Busy {
        home: PathBuf,
        pid: Option<u32>,
    },
    /// Builds must be sandboxed, but sandboxes cannot be created here.
    Sandbox(String),
}

/// A build command that failed.
#[derive(Debug)]
pub struct CommandFailure {
    pub package: String,
    pub version: String,
    pub command: String,
    /// How it exited, e.g. `exited with exit status: 1`.
    pub status: String,
    pub log: PathBuf,
    /// The end of the build log.
    pub tail: String,
}

impl BuildError {
    fn io(context: impl Into<String>, error: std::io::Error) -> Self {
        BuildError::Io {
            context: context.into(),
            error,
        }
    }
}

impl std::fmt::Display for BuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BuildError::NoHome => {
                f.write_str("cannot locate the Nux home: neither NUX_HOME nor HOME is set")
            }
            BuildError::Io { context, error } => write!(f, "{context}: {error}"),
            BuildError::Download {
                package,
                version,
                message,
            } => {
                write!(f, "failed to download {package} {version}: {message}")
            }
            BuildError::Downloads(errors) => {
                write!(f, "{} downloads failed", errors.len())?;
                for error in errors {
                    write!(f, "\n  {error}")?;
                }
                Ok(())
            }
            BuildError::LocalSource {
                package,
                version,
                message,
            } => write!(f, "{package} {version}: {message}"),
            BuildError::Command(failure) => {
                let CommandFailure {
                    package,
                    version,
                    command,
                    status,
                    log,
                    tail,
                } = &**failure;
                write!(
                    f,
                    "failed to build {package} {version}: `{command}` {status}"
                )?;
                if !tail.is_empty() {
                    write!(f, "\n{tail}")?;
                }
                write!(f, "\nfull log: {}", log.display())
            }
            BuildError::Bin {
                package,
                version,
                path,
                reason,
            } => write!(
                f,
                "{package} {version} declares `set_bin`, but `{}` {reason}",
                path.display()
            ),
            BuildError::Library {
                package,
                version,
                library,
                reason,
            } => {
                write!(
                    f,
                    "{package} {version} exports library `{library}`, but {reason}"
                )
            }
            BuildError::LinkConflict { path, reason } => {
                write!(f, "refusing to replace `{}`: {reason}", path.display())
            }
            BuildError::Busy { home, pid } => {
                write!(
                    f,
                    "the Nux workspace `{}` is in use by another nux process",
                    home.display()
                )?;
                if let Some(pid) = pid {
                    write!(f, " (pid {pid})")?;
                }
                Ok(())
            }
            BuildError::Clean { path, reason } => {
                write!(f, "refusing to remove `{}`: {reason}", path.display())
            }
            BuildError::Sandbox(reason) => write!(f, "builds must be sandboxed, but {reason}"),
        }
    }
}

impl std::error::Error for BuildError {}

/// Download progress; either part may be unknown.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Progress {
    pub fraction: Option<f64>,
    pub bytes_per_second: Option<f64>,
}

/// Observes a sync. Download callbacks may come from several threads.
pub trait Reporter: Sync {
    /// The source is already downloaded and valid.
    fn download_skipped(&self, _step: &Step) {}
    /// The source was queued, then taken from the download cache instead of
    /// the network. Defaults to a successful [`Reporter::download_finished`].
    fn download_cached(&self, step: &Step) {
        self.download_finished(step, Ok(()));
    }
    /// Called for every download, in plan order, before any starts.
    fn download_queued(&self, _step: &Step) {}
    fn download_progress(&self, _step: &Step, _progress: Progress) {}
    fn download_finished(&self, _step: &Step, _result: Result<(), &BuildError>) {}
    /// The package is already built and valid.
    fn build_skipped(&self, _step: &Step) {}
    fn build_started(&self, _step: &Step) {}
    fn build_finished(&self, _step: &Step, _result: Result<(), &BuildError>) {}
}

/// Whether build commands run in a sandbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SandboxMode {
    /// Sandboxed where possible; otherwise builds run directly.
    #[default]
    Auto,
    /// Sandboxed, or not built at all.
    Required,
    Off,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SandboxPolicy {
    pub mode: SandboxMode,
    /// Whether sandboxed builds can use the network.
    pub network: bool,
    /// Whether sandboxed builds share cargo's and go's download caches (in
    /// `cache/`), instead of starting from empty ones. Faster, but a build
    /// can then change what later builds get from these caches.
    pub shared_caches: bool,
}

impl Default for SandboxPolicy {
    fn default() -> Self {
        Self {
            mode: SandboxMode::Auto,
            network: true,
            shared_caches: false,
        }
    }
}

/// How build commands will run; see [`Builder::isolation`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Isolation {
    Sandboxed,
    /// The policy turns the sandbox off.
    Off,
    /// [`SandboxMode::Auto`], and sandboxes cannot be created: why.
    Unavailable(String),
}

/// Reports nothing.
pub struct Silent;

impl Reporter for Silent {}

pub struct Builder {
    workspace: Workspace,
    /// `source(local = ...)` paths are relative to this directory.
    local_root: PathBuf,
    /// Source identities sources must have, by package name and version.
    pins: HashMap<(String, String), SourceIdentity>,
    sandbox: SandboxPolicy,
    parallel_builds: usize,
}

impl Builder {
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        Self {
            workspace: Workspace::new(workspace),
            local_root: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            pins: HashMap::new(),
            sandbox: SandboxPolicy::default(),
            parallel_builds: DEFAULT_PARALLEL_BUILDS,
        }
    }

    /// Builds up to `builds` packages at the same time (at least one).
    pub fn with_parallel_builds(mut self, builds: usize) -> Self {
        self.parallel_builds = builds.max(1);
        self
    }

    pub fn with_sandbox(mut self, policy: SandboxPolicy) -> Self {
        self.sandbox = policy;
        self
    }

    /// How build commands will run under the sandbox policy. Fails with
    /// [`BuildError::Sandbox`] when the sandbox is required but cannot be
    /// created (unprivileged user namespaces are disabled, or Linux is
    /// older than 5.12).
    pub fn isolation(&self) -> Result<Isolation, BuildError> {
        if self.sandbox.mode == SandboxMode::Off {
            return Ok(Isolation::Off);
        }
        match Sandbox::available() {
            Ok(()) => Ok(Isolation::Sandboxed),
            Err(reason) => {
                let reason = format!("sandboxes cannot be created here: {reason}");
                match self.sandbox.mode {
                    SandboxMode::Required => Err(BuildError::Sandbox(reason)),
                    _ => Ok(Isolation::Unavailable(reason)),
                }
            }
        }
    }

    pub fn with_local_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.local_root = root.into();
        self
    }

    /// Requires these source identities (recorded in `Nux.lock`): pinned
    /// git revisions are checked out exactly, pinned checksums verified, and
    /// downloads or installations from another revision are not reused.
    pub fn with_pins(mut self, pins: HashMap<(String, String), SourceIdentity>) -> Self {
        self.pins = pins;
        self
    }

    pub fn workspace(&self) -> &Workspace {
        &self.workspace
    }

    pub fn bin_path(&self) -> PathBuf {
        self.workspace.bin_dir()
    }

    /// Inspects the workspace for `packages` (a build order, dependencies
    /// first); `requested` marks the versions the environment asked for.
    pub fn plan(
        &self,
        packages: Vec<ResolvedPackage>,
        requested: &[PackageVersionId],
    ) -> BuildPlan {
        plan::plan(
            &self.workspace,
            &self.local_root,
            &self.pins,
            packages,
            requested,
        )
    }

    /// Downloads the missing sources of packages that need building, from
    /// the download cache when it has them. Every download runs to
    /// completion; failures are returned together. Returns how many sources
    /// came from the network.
    pub fn download(&self, plan: &BuildPlan, reporter: &dyn Reporter) -> Result<usize, BuildError> {
        self.workspace.create()?;
        let mut queue = Vec::new();
        for step in plan.to_build() {
            match step.source {
                SourceStatus::Cached => reporter.download_skipped(step),
                SourceStatus::Missing => {
                    reporter.download_queued(step);
                    queue.push(step);
                }
                SourceStatus::None | SourceStatus::Local(_) => {}
            }
        }

        let next = AtomicUsize::new(0);
        let errors = Mutex::new(Vec::new());
        let from_network = AtomicUsize::new(0);
        std::thread::scope(|scope| {
            for _ in 0..PARALLEL_DOWNLOADS.min(queue.len()) {
                scope.spawn(|| {
                    while let Some(&step) = queue.get(next.fetch_add(1, Ordering::Relaxed)) {
                        let result = fetch::fetch(&self.workspace, step, &|progress| {
                            reporter.download_progress(step, progress)
                        });
                        match result {
                            Ok(fetch::Origin::Cache) => reporter.download_cached(step),
                            Ok(fetch::Origin::Network) => {
                                from_network.fetch_add(1, Ordering::Relaxed);
                                reporter.download_finished(step, Ok(()));
                            }
                            Err(error) => {
                                reporter.download_finished(step, Err(&error));
                                errors.lock().unwrap().push((step.package.id, error));
                            }
                        }
                    }
                });
            }
        });

        let mut errors = errors.into_inner().unwrap();
        errors.sort_by_key(|(id, _)| plan.steps.iter().position(|s| s.package.id == *id));
        let mut errors: Vec<_> = errors.into_iter().map(|(_, e)| e).collect();
        match errors.len() {
            0 => Ok(from_network.into_inner()),
            1 => Err(errors.pop().unwrap()),
            _ => Err(BuildError::Downloads(errors)),
        }
    }

    /// Builds every package that is not installed, group by group (see
    /// [`BuildPlan::build_groups`]), up to the configured number at the
    /// same time. After a failure, no new build starts; those running
    /// finish, and the first failure in plan order is returned. Each
    /// success is recorded immediately, so a failure does not throw away
    /// other work. Returns how many packages were built.
    pub fn build(&self, plan: &BuildPlan, reporter: &dyn Reporter) -> Result<usize, BuildError> {
        let sandboxed = self.isolation()? == Isolation::Sandboxed;
        self.workspace.create()?;
        for step in plan.steps.iter().filter(|s| s.installed) {
            reporter.build_skipped(step);
        }
        // Every build may use every CPU, and the system shares them between
        // builds running together. Splitting them instead was slower: a long
        // build left alone at the end of a group kept its small share.
        let jobs = std::thread::available_parallelism().map_or(1, |n| n.get());
        let mut built = 0;
        for group in plan.build_groups() {
            let workers = self.parallel_builds.min(group.len());
            let next = AtomicUsize::new(0);
            let succeeded = AtomicUsize::new(0);
            let failures = Mutex::new(Vec::new());
            let work = || {
                while failures.lock().unwrap().is_empty() {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(&step) = group.get(index) else { break };
                    reporter.build_started(step);
                    let result =
                        self.build_step(step, &DependencyExports::of(plan, step), sandboxed, jobs);
                    reporter.build_finished(step, result.as_ref().map(|_| ()));
                    match result {
                        Ok(()) => {
                            succeeded.fetch_add(1, Ordering::Relaxed);
                        }
                        Err(error) => failures.lock().unwrap().push((index, error)),
                    }
                }
            };
            if workers == 1 {
                work();
            } else {
                std::thread::scope(|scope| {
                    for _ in 0..workers {
                        scope.spawn(work);
                    }
                });
            }
            built += succeeded.into_inner();
            let mut failures = failures.into_inner().unwrap();
            failures.sort_by_key(|(index, _)| *index);
            if let Some((_, error)) = failures.into_iter().next() {
                return Err(error);
            }
        }
        Ok(built)
    }

    fn build_step(
        &self,
        step: &Step,
        exports: &DependencyExports,
        sandboxed: bool,
        jobs: usize,
    ) -> Result<(), BuildError> {
        let package = &step.package;
        let paths = &step.paths;
        let (name, version) = (&package.name, &package.version);

        // Forget the previous installation first: if this build is
        // interrupted, the half-written prefix must not look installed.
        let mut metadata = Metadata::read(&paths.metadata);
        if metadata.installed.take().is_some() {
            metadata.write(&paths.metadata, name, version)?;
        }
        for dir in [&paths.build, &paths.install] {
            reset_dir(dir)?;
        }

        let (source_dir, source_path) = match &step.source {
            SourceStatus::None => (paths.build.clone(), paths.build.clone()),
            SourceStatus::Local(path) => {
                fetch::local_source(path).map_err(|message| BuildError::LocalSource {
                    package: name.clone(),
                    version: version.clone(),
                    message,
                })?
            }
            SourceStatus::Cached | SourceStatus::Missing => {
                match &metadata.source.as_ref().and_then(|s| s.file.clone()) {
                    Some(file) => (paths.source.clone(), paths.source.join(file)),
                    None => (paths.source.clone(), paths.source.clone()),
                }
            }
        };
        // Builds run on a copy, so the downloaded source stays pristine and
        // reusable; a single-file source is only referenced.
        if source_path.is_dir() && source_path != paths.build {
            copy_dir(&source_path, &paths.build)?;
        }

        // A fresh environment per package: nothing leaks between builds.
        let path = std::env::var_os("PATH")
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut executor = Executor::new();
        executor.set_cwd(&paths.build);
        executor.set_env("NUX_HOME", self.workspace.home().display().to_string());
        executor.set_env("NUX_PREFIX", paths.install.display().to_string());
        executor.set_env("NUX_SOURCE", source_dir.display().to_string());
        executor.set_env("NUX_SOURCE_PATH", source_path.display().to_string());
        executor.set_env("NUX_BUILD", paths.build.display().to_string());
        executor.set_env("NUX_JOBS", jobs.to_string());
        // Links are only created once everything is built, so dependencies'
        // executables are reached through their install prefixes.
        let mut search: Vec<String> = exports
            .bin
            .iter()
            .map(|d| d.display().to_string())
            .collect();
        search.push(self.workspace.bin_dir().display().to_string());
        search.push(path);
        executor.set_env("PATH", search.join(":"));
        for (name, value) in exports.vars(|name| std::env::var(name).ok()) {
            executor.set_env(name, value);
        }
        if sandboxed {
            let path = executor.get_env("PATH").cloned().unwrap_or_default();
            let mut sandbox = self.sandbox_for(paths, &source_dir, &path);
            if self.sandbox.shared_caches {
                for (variable, dir) in self.shared_caches()? {
                    sandbox.expose(&dir, Access::ReadWrite);
                    executor.set_env(variable, dir.display().to_string());
                }
            }
            executor.set_sandbox(Some(sandbox));
        }

        let log = std::fs::File::create(&paths.log)
            .map_err(|e| BuildError::io(format!("cannot create `{}`", paths.log.display()), e))?;
        for command in &package.commands {
            // The command first, so the log reads like a terminal session.
            let logged = writeln!(&log, "$ {command}").and_then(|()| (&log).flush());
            logged.map_err(|e| {
                BuildError::io(format!("cannot write `{}`", paths.log.display()), e)
            })?;
            if let Err(status) = executor.execute_logged(command, &log) {
                let _ = std::fs::remove_dir_all(&paths.install);
                return Err(BuildError::Command(Box::new(CommandFailure {
                    package: name.clone(),
                    version: version.clone(),
                    command: command.clone(),
                    status: status.trim_start_matches("Command ").to_string(),
                    log: paths.log.clone(),
                    tail: tail(&paths.log, LOG_TAIL),
                })));
            }
        }

        if let Some(bin) = &package.bin {
            let target = paths.install.join(bin);
            if let Err(reason) = plan::executable(&target) {
                let _ = std::fs::remove_dir_all(&paths.install);
                return Err(BuildError::Bin {
                    package: name.clone(),
                    version: version.clone(),
                    path: target,
                    reason,
                });
            }
        }
        for library in &package.libraries {
            if let Err(reason) = plan::library(&paths.install, library) {
                let _ = std::fs::remove_dir_all(&paths.install);
                return Err(BuildError::Library {
                    package: name.clone(),
                    version: version.clone(),
                    library: library.name.clone(),
                    reason,
                });
            }
        }

        // Intermediate files are not needed once installed.
        let _ = std::fs::remove_dir_all(&paths.build);
        metadata.installed = Some(InstallRecord {
            fingerprint: step.fingerprint.clone(),
            bin: package.bin.clone(),
            identity: metadata
                .source
                .as_ref()
                .map(|s| s.identity.clone())
                .unwrap_or_default(),
        });
        metadata.write(&paths.metadata, name, version)
    }

    /// What a build of `paths` sees: the workspace read-only (sources,
    /// dependencies, links), its own `build/` and `install/` writable, a
    /// local source read-only, the programs on `path`, and an empty `$HOME`
    /// and `$TMPDIR` for caches.
    fn sandbox_for(&self, paths: &PackagePaths, source: &Path, path: &str) -> Sandbox {
        let mut sandbox = Sandbox::new();
        sandbox
            .network(self.sandbox.network)
            .expose(self.workspace.home(), Access::ReadOnly)
            .expose(source, Access::ReadOnly)
            .expose(&paths.build, Access::ReadWrite)
            .expose(&paths.install, Access::ReadWrite)
            .expose_programs(path.as_ref());
        for variable in ["HOME", "TMPDIR"] {
            if let Some(dir) = std::env::var_os(variable).filter(|d| !d.is_empty()) {
                sandbox.directory(dir);
            }
        }
        sandbox
    }

    /// The tool caches sandboxed builds share, with the variable pointing
    /// each tool at its directory; created if needed.
    fn shared_caches(&self) -> Result<[(&'static str, PathBuf); 2], BuildError> {
        let cache = self.workspace.cache_dir();
        let caches = [
            ("CARGO_HOME", cache.join("cargo")),
            ("GOMODCACHE", cache.join("go").join("mod")),
        ];
        for (_, dir) in &caches {
            std::fs::create_dir_all(dir)
                .map_err(|e| BuildError::io(format!("cannot create `{}`", dir.display()), e))?;
        }
        Ok(caches)
    }

    /// Creates or updates the links in `bin/`. Fails without changing
    /// anything if a link would replace a file nux does not manage.
    /// Returns how many links changed.
    pub fn link_executables(&self, plan: &BuildPlan) -> Result<usize, BuildError> {
        let bin = self.workspace.bin_dir();
        if let Some(link) = plan.conflicts().next() {
            let LinkStatus::Conflict(reason) = &link.status else {
                unreachable!()
            };
            return Err(BuildError::LinkConflict {
                path: bin.join(&link.name),
                reason: reason.clone(),
            });
        }
        std::fs::create_dir_all(&bin)
            .map_err(|e| BuildError::io(format!("cannot create `{}`", bin.display()), e))?;

        let mut changed = 0;
        for link in plan
            .links
            .iter()
            .filter(|l| l.status != LinkStatus::Current)
        {
            let path = bin.join(&link.name);
            let target = bin.join(&link.target);
            if let Err(reason) = plan::executable(&target) {
                return Err(BuildError::Bin {
                    package: link.package.clone(),
                    version: link.version.clone(),
                    path: target,
                    reason,
                });
            }
            // Replace atomically, so the command never disappears.
            let tmp = bin.join(format!(".{}.tmp", link.name));
            let _ = std::fs::remove_file(&tmp);
            std::os::unix::fs::symlink(&link.target, &tmp)
                .and_then(|()| std::fs::rename(&tmp, &path))
                .map_err(|e| BuildError::io(format!("cannot link `{}`", path.display()), e))?;
            changed += 1;
        }
        Ok(changed)
    }

    /// Records which versions the environment `name`, defined in `path`,
    /// resolved to. `nux purge` keeps what the records of existing
    /// environments use.
    pub fn record_environment(
        &self,
        name: &str,
        path: &Path,
        plan: &BuildPlan,
    ) -> Result<(), BuildError> {
        let packages: Vec<_> = plan
            .steps
            .iter()
            .map(|s| json!({ "name": s.package.name, "version": s.package.version, "requested": s.requested }))
            .collect();
        let path = std::fs::canonicalize(path)
            .or_else(|_| std::path::absolute(path))
            .unwrap_or_else(|_| path.to_path_buf());
        let value = json!({ "name": name, "path": path, "packages": packages });
        let path = self.workspace.record_path(name);
        let text =
            serde_json::to_string_pretty(&value).expect("json values always serialize") + "\n";
        write_atomic(&path, text.as_bytes())
    }
}

fn reset_dir(dir: &Path) -> Result<(), BuildError> {
    if dir.exists() {
        std::fs::remove_dir_all(dir)
            .map_err(|e| BuildError::io(format!("cannot clean `{}`", dir.display()), e))?;
    }
    std::fs::create_dir_all(dir)
        .map_err(|e| BuildError::io(format!("cannot create `{}`", dir.display()), e))
}

/// Copies the contents of `from` into `to`, keeping symlinks and skipping
/// `.git`, which builds do not need.
fn copy_dir(from: &Path, to: &Path) -> Result<(), BuildError> {
    let context = |path: &Path| format!("cannot copy `{}`", path.display());
    let entries = std::fs::read_dir(from).map_err(|e| BuildError::io(context(from), e))?;
    for entry in entries {
        let entry = entry.map_err(|e| BuildError::io(context(from), e))?;
        let (source, target) = (entry.path(), to.join(entry.file_name()));
        if entry.file_name() == ".git" {
            continue;
        }
        let kind = entry
            .file_type()
            .map_err(|e| BuildError::io(context(&source), e))?;
        if kind.is_symlink() {
            let link =
                std::fs::read_link(&source).map_err(|e| BuildError::io(context(&source), e))?;
            std::os::unix::fs::symlink(link, &target)
                .map_err(|e| BuildError::io(context(&source), e))?;
        } else if kind.is_dir() {
            std::fs::create_dir_all(&target).map_err(|e| BuildError::io(context(&source), e))?;
            copy_dir(&source, &target)?;
        } else {
            std::fs::copy(&source, &target).map_err(|e| BuildError::io(context(&source), e))?;
        }
    }
    Ok(())
}

/// The last `lines` lines of `path`.
fn tail(path: &Path, lines: usize) -> String {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let all: Vec<_> = text.lines().collect();
    all[all.len().saturating_sub(lines)..].join("\n")
}

#[cfg(test)]
mod tests;
