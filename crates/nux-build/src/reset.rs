//! Resetting a workspace: removing everything Nux put in it, as if it had
//! never run.
//!
//! Only what Nux creates goes: `packages/`, `state/`, `cache/` (unless
//! kept) and the links in `bin/` into `../packages/`. Anything else in the
//! home or in `bin/` is left alone, so a home pointed at the wrong
//! directory loses nothing that is not Nux's. The `.lock` file stays: other
//! processes may be waiting on it.

use std::path::{Path, PathBuf};

use crate::cache::make_writable;
use crate::clean::{remove, size};
use crate::{BuildError, CleanItem, CleanKind, Workspace};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ResetOptions {
    /// Keep the download cache, so sources need no download afterwards.
    pub keep_cache: bool,
}

impl Workspace {
    /// Lists what [`Workspace::reset`] would remove: the links first, so
    /// that none is ever left pointing at a removed package.
    pub fn reset_plan(&self, options: ResetOptions) -> Result<Vec<CleanItem>, BuildError> {
        let mut items = Vec::new();
        let links = self.nux_links();
        if !links.is_empty() {
            let bytes = links.iter().map(|l| size(l)).sum();
            items.push(CleanItem {
                kind: CleanKind::Links,
                path: self.bin_dir(),
                package: None,
                bytes,
            });
        }
        let mut parts = vec![
            (CleanKind::Packages, self.packages_dir()),
            (CleanKind::Records, self.state_dir()),
        ];
        if !options.keep_cache {
            parts.push((CleanKind::Cache, self.cache_dir()));
        }
        for (kind, path) in parts {
            if path.symlink_metadata().is_ok() {
                items.push(CleanItem {
                    kind,
                    bytes: size(&path),
                    path,
                    package: None,
                });
            }
        }
        Ok(items)
    }

    /// What a reset leaves because Nux did not create it: unknown entries
    /// of the home, and files in `bin/` that are not Nux's links.
    pub fn reset_keeps(&self) -> Vec<PathBuf> {
        let known = ["bin", "packages", "state", "cache", ".lock"];
        let mut kept: Vec<PathBuf> = entries(self.home())
            .into_iter()
            .filter(|p| !p.file_name().is_some_and(|n| known.iter().any(|k| n == *k)))
            .collect();
        let links = self.nux_links();
        kept.extend(
            entries(&self.bin_dir())
                .into_iter()
                .filter(|p| !links.contains(p)),
        );
        kept.sort();
        kept
    }

    /// Removes `items` (from [`Workspace::reset_plan`]), checking again that
    /// each is a part of the workspace. Returns the bytes freed.
    pub fn reset(&self, items: &[CleanItem]) -> Result<u64, BuildError> {
        let mut freed = 0;
        for item in items {
            let expected = match item.kind {
                CleanKind::Links => self.bin_dir(),
                CleanKind::Packages => self.packages_dir(),
                CleanKind::Records => self.state_dir(),
                CleanKind::Cache => self.cache_dir(),
                _ => PathBuf::new(),
            };
            if item.path != expected {
                let reason = "it is not something `reset` removes".into();
                return Err(BuildError::Clean {
                    path: item.path.clone(),
                    reason,
                });
            }
            if item.kind == CleanKind::Links {
                for link in self.nux_links() {
                    std::fs::remove_file(&link).map_err(|e| {
                        BuildError::io(format!("cannot remove `{}`", link.display()), e)
                    })?;
                }
                // Kept when something else is in it.
                let _ = std::fs::remove_dir(&item.path);
            } else {
                // Builds and Go leave read-only files and directories.
                make_writable(&item.path);
                remove(&item.path)?;
            }
            freed += item.bytes;
        }
        Ok(freed)
    }

    /// The links in `bin/` Nux created: into `../packages/`, or being
    /// replaced (`.<name>.tmp`).
    fn nux_links(&self) -> Vec<PathBuf> {
        entries(&self.bin_dir())
            .into_iter()
            .filter(|path| {
                let Ok(target) = std::fs::read_link(path) else {
                    return false;
                };
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                target.starts_with("../packages")
                    || (name.starts_with('.') && name.ends_with(".tmp"))
            })
            .collect()
    }
}

/// The entries of `dir`, sorted; none if it cannot be read.
fn entries(dir: &Path) -> Vec<PathBuf> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|e| e.filter_map(|e| e.ok()).map(|e| e.path()).collect())
        .unwrap_or_default();
    entries.sort();
    entries
}
