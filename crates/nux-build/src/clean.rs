//! Removing what a workspace can do without.
//!
//! By default only leftovers go: interrupted downloads, build directories
//! of failed builds, build logs and temporary files. With
//! [`CleanOptions::sources`], downloaded sources go too (they are only
//! needed to rebuild), along with versions that were never installed. With
//! [`CleanOptions::cache`], the [download cache](crate::cache) goes.
//!
//! Safeguards: installations (`install/` with a record in `package.json`)
//! and any version a `bin/` link points into are never removed, nothing
//! outside `packages/`, `bin/` and `cache/` is touched, and symlinks are
//! removed as links, never followed.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::state::Metadata;
use crate::{BuildError, Workspace};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CleanKind {
    IncompleteDownload,
    BuildDirectory,
    BuildLog,
    TemporaryFile,
    DownloadedSource,
    /// A version directory without an installation (only with `sources`).
    UninstalledPackage,
    /// A version no environment uses (`purge`).
    UnusedPackage,
    /// A link in `bin/` into an unused version (`purge`).
    Link,
    /// The record of an environment whose `Nux.toml` is gone (`purge`).
    EnvironmentRecord,
    /// A part of the download cache (only with `cache`), or all of it
    /// (`reset`).
    Cache,
    /// Every link Nux created in `bin/` (`reset`).
    Links,
    /// Every installed package and downloaded source (`reset`).
    Packages,
    /// The environment records (`reset`).
    Records,
}

impl std::fmt::Display for CleanKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            CleanKind::IncompleteDownload => "incomplete download",
            CleanKind::BuildDirectory => "build directory",
            CleanKind::BuildLog => "build log",
            CleanKind::TemporaryFile => "temporary file",
            CleanKind::DownloadedSource => "downloaded source",
            CleanKind::UninstalledPackage => "uninstalled package",
            CleanKind::UnusedPackage => "unused package",
            CleanKind::Link => "executable link",
            CleanKind::EnvironmentRecord => "stale record",
            CleanKind::Cache => "download cache",
            CleanKind::Links => "executable links",
            CleanKind::Packages => "packages",
            CleanKind::Records => "environment records",
        })
    }
}

/// Something `clean` or `purge` removes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanItem {
    pub kind: CleanKind,
    pub path: PathBuf,
    /// The package version it belongs to, if any.
    pub package: Option<(String, String)>,
    pub bytes: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CleanOptions {
    /// Also remove downloaded sources and never-installed versions.
    pub sources: bool,
    /// Also empty the download cache: git mirrors, downloaded files, and
    /// the tool caches builds share.
    pub cache: bool,
}

impl Workspace {
    /// Lists what [`Workspace::clean`] would remove, without removing it.
    pub fn clean_plan(&self, options: CleanOptions) -> Result<Vec<CleanItem>, BuildError> {
        let mut items = Vec::new();
        let active = self.linked_versions();

        for (name, version) in self.version_dirs()? {
            let paths = self.package(&name, &version);
            let installed = Metadata::read(&paths.metadata).installed.is_some() && paths.install.is_dir();
            let package = Some((name.clone(), version.clone()));
            let item = |kind, path: PathBuf| CleanItem { kind, bytes: size(&path), path, package: package.clone() };

            if options.sources && !installed && !active.contains(&paths.root) {
                items.push(item(CleanKind::UninstalledPackage, paths.root.clone()));
                continue;
            }
            for (kind, path) in [
                (CleanKind::IncompleteDownload, paths.root.join("source.tmp")),
                (CleanKind::BuildDirectory, paths.build.clone()),
                (CleanKind::BuildLog, paths.log.clone()),
                (CleanKind::TemporaryFile, paths.metadata.with_extension("tmp")),
            ] {
                if path.symlink_metadata().is_ok() {
                    items.push(item(kind, path));
                }
            }
            if options.sources && paths.source.symlink_metadata().is_ok() {
                items.push(item(CleanKind::DownloadedSource, paths.source.clone()));
            }
        }

        if options.cache
            && let Ok(entries) = std::fs::read_dir(self.cache_dir())
        {
            let mut parts: Vec<PathBuf> = entries.filter_map(|e| e.ok()).map(|e| e.path()).collect();
            parts.sort();
            items.extend(parts.into_iter().map(|path| CleanItem { kind: CleanKind::Cache, bytes: size(&path), path, package: None }));
        }

        // Links being replaced are written as `bin/.<name>.tmp`.
        if let Ok(entries) = std::fs::read_dir(self.bin_dir()) {
            let mut temporary: Vec<_> = entries
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| {
                    let name = p.file_name().unwrap_or_default().to_string_lossy();
                    name.starts_with('.') && name.ends_with(".tmp")
                })
                .collect();
            temporary.sort();
            items.extend(temporary.into_iter().map(|path| CleanItem {
                kind: CleanKind::TemporaryFile,
                bytes: size(&path),
                path,
                package: None,
            }));
        }
        Ok(items)
    }

    /// Removes `items` (from [`Workspace::clean_plan`]), re-checking each
    /// against the safeguards first. Returns the bytes freed.
    pub fn clean(&self, items: &[CleanItem]) -> Result<u64, BuildError> {
        let active = self.linked_versions();
        let mut freed = 0;
        for item in items {
            self.check_removable(item, &active)?;
            if item.path.symlink_metadata().is_err() {
                continue;
            }
            if item.kind == CleanKind::Cache {
                crate::cache::make_writable(&item.path);
            }
            remove(&item.path)?;
            freed += item.bytes;

            if let (CleanKind::DownloadedSource, Some((name, version))) = (item.kind, &item.package) {
                let paths = self.package(name, version);
                let mut metadata = Metadata::read(&paths.metadata);
                if metadata.source.take().is_some() {
                    metadata.write(&paths.metadata, name, version)?;
                }
            }
            if item.kind == CleanKind::UninstalledPackage
                && let Some(parent) = item.path.parent()
            {
                // Drop `packages/<name>` once its last version is gone.
                let _ = std::fs::remove_dir(parent);
            }
        }
        Ok(freed)
    }

    fn check_removable(&self, item: &CleanItem, active: &HashSet<PathBuf>) -> Result<(), BuildError> {
        let refuse = |reason: &str| BuildError::Clean { path: item.path.clone(), reason: reason.into() };
        let packages = self.packages_dir();
        let inside = match item.kind {
            CleanKind::TemporaryFile if item.package.is_none() => item.path.parent() == Some(self.bin_dir().as_path()),
            CleanKind::Cache => item.path.parent() == Some(self.cache_dir().as_path()),
            CleanKind::UninstalledPackage => item.path.parent().and_then(Path::parent) == Some(packages.as_path()),
            _ => item.path.parent().and_then(Path::parent).and_then(Path::parent) == Some(packages.as_path()),
        };
        if !inside {
            return Err(refuse("it is not part of the Nux workspace layout"));
        }
        if item.path.file_name().is_some_and(|n| n == "install" || n == "package.json") {
            return Err(refuse("installations and their records are never cleaned"));
        }
        let version_dir = match item.kind {
            CleanKind::UninstalledPackage => Some(item.path.as_path()),
            _ if item.package.is_some() => item.path.parent(),
            _ => None,
        };
        if item.kind == CleanKind::UninstalledPackage
            && let Some(dir) = version_dir
        {
            if active.contains(dir) {
                return Err(refuse("an executable link points into it"));
            }
            if Metadata::read(&dir.join("package.json")).installed.is_some() {
                return Err(refuse("it is installed"));
            }
        }
        Ok(())
    }

    /// `packages/<name>/<version>` directories, sorted.
    pub(crate) fn version_dirs(&self) -> Result<Vec<(String, String)>, BuildError> {
        let mut dirs = Vec::new();
        let list = |dir: &Path| -> Result<Vec<String>, BuildError> {
            let entries = match std::fs::read_dir(dir) {
                Ok(entries) => entries,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
                Err(e) => return Err(BuildError::io(format!("cannot read `{}`", dir.display()), e)),
            };
            let mut names: Vec<String> = entries
                .filter_map(|e| e.ok())
                // Real directories only: never descend through a symlink.
                .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            Ok(names)
        };
        for name in list(&self.packages_dir())? {
            for version in list(&self.packages_dir().join(&name))? {
                dirs.push((name.clone(), version));
            }
        }
        Ok(dirs)
    }

    /// Version directories that a link in `bin/` points into.
    fn linked_versions(&self) -> HashSet<PathBuf> {
        let Ok(entries) = std::fs::read_dir(self.bin_dir()) else {
            return HashSet::new();
        };
        entries
            .filter_map(|e| e.ok())
            .filter_map(|e| std::fs::read_link(e.path()).ok())
            .filter_map(|target| {
                // `../packages/<name>/<version>/install/...`
                let mut parts = target.strip_prefix("../packages").ok()?.components();
                let name = parts.next()?;
                let version = parts.next()?;
                Some(self.packages_dir().join(name).join(version))
            })
            .collect()
    }
}

pub(crate) fn remove(path: &Path) -> Result<(), BuildError> {
    let result = match path.symlink_metadata() {
        Ok(m) if m.is_dir() => std::fs::remove_dir_all(path),
        Ok(_) => std::fs::remove_file(path),
        Err(_) => return Ok(()),
    };
    result.map_err(|e| BuildError::io(format!("cannot remove `{}`", path.display()), e))
}

/// Total size of the files under `path`, without following symlinks.
pub(crate) fn size(path: &Path) -> u64 {
    let Ok(metadata) = path.symlink_metadata() else { return 0 };
    if !metadata.is_dir() {
        return metadata.len();
    }
    std::fs::read_dir(path)
        .map(|entries| entries.filter_map(|e| e.ok()).map(|e| size(&e.path())).sum())
        .unwrap_or(0)
}
