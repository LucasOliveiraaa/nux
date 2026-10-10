//! Removing what no environment uses any more.
//!
//! Every sync records the versions its environment needs in
//! `state/environments/<name>.json`, along with the path of its `Nux.toml`.
//! A version no record mentions is unused: [`Workspace::purge_plan`] lists
//! those, the links into them, and the records of environments whose
//! `Nux.toml` is gone. All of it can be rebuilt by a sync.
//!
//! Safeguards: everything is checked again against freshly read records
//! right before it is removed, only links into `../packages/` are removed,
//! and nothing outside `packages/`, `bin/` and `state/environments/` is
//! touched.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::clean::{remove, size};
use crate::{BuildError, BuildPlan, CleanItem, CleanKind, Workspace};

/// A package name and version.
type Version = (String, String);

/// What an environment needed at its last sync.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvironmentRecord {
    pub name: String,
    /// Its `Nux.toml`; unknown in records written by older versions of Nux.
    pub path: Option<PathBuf>,
    /// Every version it needs, as `(name, version)`.
    pub packages: Vec<Version>,
    /// The record file.
    pub file: PathBuf,
}

impl EnvironmentRecord {
    /// Its `Nux.toml` no longer exists. A record without a path is never stale.
    pub fn is_stale(&self) -> bool {
        self.path.as_ref().is_some_and(|path| !path.exists())
    }
}

/// A link in `bin/` into a version that is no longer used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnusedLink {
    pub name: String,
    pub package: String,
    pub version: String,
}

impl Workspace {
    /// Every environment record, sorted by file name.
    pub fn environment_records(&self) -> Result<Vec<EnvironmentRecord>, BuildError> {
        let dir = self.state_dir().join("environments");
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(BuildError::io(format!("cannot read `{}`", dir.display()), e)),
        };
        let mut files: Vec<PathBuf> = entries
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|ext| ext == "json"))
            .collect();
        files.sort();
        files.iter().map(|file| read_record(file)).collect()
    }

    pub(crate) fn record_path(&self, name: &str) -> PathBuf {
        self.state_dir().join("environments").join(format!("{name}.json"))
    }

    /// Versions the environment `name` needed at its last sync that `plan`
    /// no longer needs, no other environment uses, and that are in the
    /// workspace: what syncing `plan` leaves unused.
    pub fn dropped_versions(&self, name: &str, plan: &BuildPlan) -> Result<Vec<Version>, BuildError> {
        let records = self.environment_records()?;
        let Some(old) = records.iter().find(|r| r.name == name) else {
            return Ok(Vec::new());
        };
        let needed: HashSet<Version> =
            plan.steps.iter().map(|s| (s.package.name.clone(), s.package.version.clone())).collect();
        let used = used_versions(records.iter().filter(|r| r.name != name));
        let mut dropped: Vec<Version> = old
            .packages
            .iter()
            .filter(|v| !needed.contains(*v) && !used.contains(*v) && self.package(&v.0, &v.1).root.is_dir())
            .cloned()
            .collect();
        dropped.sort();
        dropped.dedup();
        Ok(dropped)
    }

    /// The links `plan` leaves behind: links into `versions`, and links to
    /// an executable that `plan` now links under another name (its recipe
    /// renamed it with `set_bin(..., name = ...)`). Commands `plan` links
    /// itself are replaced instead, so they are left out.
    pub fn links_into(&self, versions: &[Version], plan: &BuildPlan) -> Vec<UnusedLink> {
        let renamed = |name: &str| {
            let Ok(target) = std::fs::read_link(self.bin_dir().join(name)) else { return false };
            plan.links.iter().any(|link| link.target == target && link.name != name)
        };
        self.managed_links()
            .into_iter()
            .filter(|(name, version)| versions.contains(version) || renamed(name))
            .filter(|(name, _)| !plan.links.iter().any(|l| &l.name == name))
            .map(|(name, (package, version))| UnusedLink { name, package, version })
            .collect()
    }

    /// Removes `links`, each only while it still points into the same
    /// version. Returns how many were removed.
    pub fn remove_links(&self, links: &[UnusedLink]) -> Result<usize, BuildError> {
        let mut removed = 0;
        for link in links {
            let path = self.bin_dir().join(&link.name);
            let current = std::fs::read_link(&path).ok().and_then(|target| link_version(&target));
            if current != Some((link.package.clone(), link.version.clone())) {
                continue;
            }
            std::fs::remove_file(&path).map_err(|e| BuildError::io(format!("cannot remove `{}`", path.display()), e))?;
            removed += 1;
        }
        Ok(removed)
    }

    /// Lists what [`Workspace::purge`] would remove, without removing it:
    /// links into unused versions, unused versions, then stale records.
    pub fn purge_plan(&self) -> Result<Vec<CleanItem>, BuildError> {
        let records = self.environment_records()?;
        let used = used_versions(&records);
        let mut items = Vec::new();
        for (name, version) in self.managed_links() {
            if !used.contains(&version) {
                let path = self.bin_dir().join(name);
                items.push(CleanItem { kind: CleanKind::Link, bytes: size(&path), path, package: None });
            }
        }
        for version in self.version_dirs()? {
            if !used.contains(&version) {
                let path = self.package(&version.0, &version.1).root;
                items.push(CleanItem { kind: CleanKind::UnusedPackage, bytes: size(&path), path, package: Some(version) });
            }
        }
        for record in records.into_iter().filter(EnvironmentRecord::is_stale) {
            let path = record.file;
            items.push(CleanItem { kind: CleanKind::EnvironmentRecord, bytes: size(&path), path, package: None });
        }
        Ok(items)
    }

    /// Removes `items` (from [`Workspace::purge_plan`]), re-checking each
    /// against the records as they are now. Returns the bytes freed.
    pub fn purge(&self, items: &[CleanItem]) -> Result<u64, BuildError> {
        // An environment may have been synced since the plan was made.
        let records = self.environment_records()?;
        let used = used_versions(&records);
        let mut freed = 0;
        for item in items {
            if item.path.symlink_metadata().is_err() {
                continue;
            }
            self.check_purgeable(item, &records, &used)?;
            remove(&item.path)?;
            freed += item.bytes;
            if item.kind == CleanKind::UnusedPackage
                && let Some(parent) = item.path.parent()
            {
                // Drop `packages/<name>` once its last version is gone.
                let _ = std::fs::remove_dir(parent);
            }
        }
        Ok(freed)
    }

    fn check_purgeable(
        &self,
        item: &CleanItem,
        records: &[EnvironmentRecord],
        used: &HashSet<Version>,
    ) -> Result<(), BuildError> {
        let refuse = |reason: &str| BuildError::Clean { path: item.path.clone(), reason: reason.into() };
        let outside = || refuse("it is not part of the Nux workspace layout");
        let version = match item.kind {
            CleanKind::Link => {
                if item.path.parent() != Some(self.bin_dir().as_path()) {
                    return Err(outside());
                }
                let target = std::fs::read_link(&item.path).map_err(|_| refuse("it is not a link created by nux"))?;
                Some(link_version(&target).ok_or_else(|| refuse("it is not a link created by nux"))?)
            }
            CleanKind::UnusedPackage => match &item.package {
                Some((name, version)) if self.package(name, version).root == item.path => {
                    Some((name.clone(), version.clone()))
                }
                _ => return Err(outside()),
            },
            CleanKind::EnvironmentRecord => {
                if item.path.parent() != Some(self.state_dir().join("environments").as_path()) {
                    return Err(outside());
                }
                if !records.iter().any(|r| r.file == item.path && r.is_stale()) {
                    return Err(refuse("its environment still exists"));
                }
                None
            }
            _ => return Err(refuse("it is not something `purge` removes")),
        };
        if version.is_some_and(|v| used.contains(&v)) {
            return Err(refuse("an environment uses it"));
        }
        Ok(())
    }

    /// Links in `bin/` into `../packages/`, with the version each points
    /// into, sorted by name. Temporary links (`.<name>.tmp`) are left out.
    fn managed_links(&self) -> Vec<(String, Version)> {
        let Ok(entries) = std::fs::read_dir(self.bin_dir()) else {
            return Vec::new();
        };
        let mut links: Vec<_> = entries
            .filter_map(|e| e.ok())
            .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
            .filter_map(|e| {
                let version = link_version(&std::fs::read_link(e.path()).ok()?)?;
                Some((e.file_name().to_string_lossy().into_owned(), version))
            })
            .collect();
        links.sort();
        links
    }
}

/// Versions the live (not stale) records use.
fn used_versions<'a>(records: impl IntoIterator<Item = &'a EnvironmentRecord>) -> HashSet<Version> {
    records.into_iter().filter(|r| !r.is_stale()).flat_map(|r| r.packages.iter().cloned()).collect()
}

/// The version a link target `../packages/<name>/<version>/...` points into.
fn link_version(target: &Path) -> Option<Version> {
    let mut parts = target.strip_prefix("../packages").ok()?.components();
    let name = parts.next()?.as_os_str().to_string_lossy().into_owned();
    let version = parts.next()?.as_os_str().to_string_lossy().into_owned();
    Some((name, version))
}

/// Reads a record. An unreadable one is an error rather than empty: purging
/// on a partial view would remove what that environment uses.
fn read_record(file: &Path) -> Result<EnvironmentRecord, BuildError> {
    let invalid = |reason: &str| {
        BuildError::io(
            format!("invalid environment record `{}`", file.display()),
            std::io::Error::new(std::io::ErrorKind::InvalidData, reason.to_string()),
        )
    };
    let text = std::fs::read_to_string(file).map_err(|e| BuildError::io(format!("cannot read `{}`", file.display()), e))?;
    let value: Value = serde_json::from_str(&text).map_err(|e| invalid(&e.to_string()))?;
    let string = |v: &Value, key: &str| v.get(key).and_then(Value::as_str).map(str::to_string);
    let name = string(&value, "name").ok_or_else(|| invalid("it has no `name`"))?;
    let packages = value
        .get("packages")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid("it has no `packages`"))?
        .iter()
        .map(|p| Some((string(p, "name")?, string(p, "version")?)))
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| invalid("a package has no `name` or `version`"))?;
    Ok(EnvironmentRecord { name, path: string(&value, "path").map(PathBuf::from), packages, file: file.to_path_buf() })
}
