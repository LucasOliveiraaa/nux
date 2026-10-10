//! The on-disk layout of a Nux home (`~/.nux` by default).
//!
//! ```text
//! $NUX_HOME/
//! ├── bin/                      executable links
//! ├── packages/<name>/<version>/
//! │   ├── source/               downloaded source (git clone or file)
//! │   ├── build/                scratch copy the build commands run in
//! │   ├── install/              $NUX_PREFIX
//! │   ├── build.log             output of the last build
//! │   └── package.json          source and installation records
//! ├── state/environments/<name>.json
//! └── .lock                     held by the process modifying the workspace
//! ```

use std::path::{Path, PathBuf};

use std::io::Write;

use crate::BuildError;

/// Proof that this process holds a workspace's lock; see [`Workspace::lock`].
#[derive(Debug)]
pub struct WorkspaceLock {
    file: std::fs::File,
}

impl Drop for WorkspaceLock {
    fn drop(&mut self) {
        // Closing the file is not enough: a child process forked by another
        // thread may still share the open file (until it execs), keeping
        // the lock. Unlocking releases it for every holder of the file.
        let _ = self.file.unlock();
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    home: PathBuf,
}

/// Paths of one package version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackagePaths {
    pub root: PathBuf,
    pub source: PathBuf,
    pub build: PathBuf,
    pub install: PathBuf,
    pub log: PathBuf,
    pub metadata: PathBuf,
}

impl Workspace {
    pub fn new(home: impl Into<PathBuf>) -> Self {
        Self { home: home.into() }
    }

    /// `$NUX_HOME`, or `~/.nux`.
    pub fn from_env() -> Result<Self, BuildError> {
        if let Some(home) = std::env::var_os("NUX_HOME").filter(|h| !h.is_empty()) {
            return Ok(Self::new(home));
        }
        std::env::var_os("HOME")
            .filter(|h| !h.is_empty())
            .map(|home| Self::new(PathBuf::from(home).join(".nux")))
            .ok_or(BuildError::NoHome)
    }

    pub fn home(&self) -> &Path {
        &self.home
    }

    pub fn bin_dir(&self) -> PathBuf {
        self.home.join("bin")
    }

    pub fn packages_dir(&self) -> PathBuf {
        self.home.join("packages")
    }

    pub fn state_dir(&self) -> PathBuf {
        self.home.join("state")
    }

    pub fn exists(&self) -> bool {
        self.packages_dir().is_dir()
    }

    /// Creates the top-level directories; existing ones are kept.
    pub fn create(&self) -> Result<(), BuildError> {
        for dir in [self.bin_dir(), self.packages_dir(), self.state_dir()] {
            std::fs::create_dir_all(&dir).map_err(|e| BuildError::io(format!("cannot create `{}`", dir.display()), e))?;
        }
        Ok(())
    }

    /// Versions of `name` with an installation record, sorted by version.
    pub fn installed_versions(&self, name: &str) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(self.packages_dir().join(name)) else {
            return Vec::new();
        };
        let mut versions: Vec<String> = entries
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|version| {
                let paths = self.package(name, version);
                paths.install.is_dir() && crate::state::Metadata::read(&paths.metadata).installed.is_some()
            })
            .collect();
        versions.sort_by_cached_key(|v| nux_resolver::Version::parse(v).ok());
        versions
    }

    /// Takes the workspace's lock: an OS file lock on `.lock`, so that only one
    /// process modifies installed packages, links and records at a time.
    /// It is released when the guard is dropped, or by the operating system
    /// when the process exits or crashes. Other workspaces are not affected.
    ///
    /// When another process holds it, this fails with [`BuildError::Busy`],
    /// or, with `wait`, calls `on_wait` with the holder's process id (if
    /// known) and blocks until it is free.
    pub fn lock(&self, wait: bool, on_wait: impl FnOnce(Option<u32>)) -> Result<WorkspaceLock, BuildError> {
        let path = self.home.join(".lock");
        let io = |e| BuildError::io(format!("cannot lock `{}`", path.display()), e);
        std::fs::create_dir_all(&self.home).map_err(io)?;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(io)?;
        match file.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => {
                let pid = std::fs::read_to_string(&path).ok().and_then(|s| s.trim().parse().ok());
                if !wait {
                    return Err(BuildError::Busy { home: self.home.clone(), pid });
                }
                on_wait(pid);
                file.lock().map_err(io)?;
            }
            Err(std::fs::TryLockError::Error(e)) => return Err(io(e)),
        }
        // For the busy message of the next process.
        let mut writer = &file;
        file.set_len(0)
            .and_then(|()| writeln!(writer, "{}", std::process::id()))
            .map_err(io)?;
        Ok(WorkspaceLock { file })
    }

    /// The identity of the downloaded source of `name` `version`, or of the
    /// source its installation was built from.
    pub fn source_identity(&self, name: &str, version: &str) -> Option<crate::SourceIdentity> {
        let metadata = crate::state::Metadata::read(&self.package(name, version).metadata);
        let downloaded = metadata.source.map(|s| s.identity);
        let installed = metadata.installed.map(|i| i.identity);
        downloaded.into_iter().chain(installed).find(|identity| !identity.is_empty())
    }

    pub fn package(&self, name: &str, version: &str) -> PackagePaths {
        let root = self.packages_dir().join(name).join(version);
        PackagePaths {
            source: root.join("source"),
            build: root.join("build"),
            install: root.join("install"),
            log: root.join("build.log"),
            metadata: root.join("package.json"),
            root,
        }
    }
}
