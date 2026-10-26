//! The download cache, `$NUX_HOME/cache`, shared by every package version:
//!
//! ```text
//! cache/
//! ├── git/<name>-<hash>.git        a bare mirror per repository URL
//! ├── downloads/sha256/<hex>       downloaded files, by content
//! ├── cargo/                       CARGO_HOME of builds (shared caches only)
//! └── go/mod/                      GOMODCACHE of builds (shared caches only)
//! ```
//!
//! Mirrors only hold the commits Nux needed, fetched with `--depth 1`, each
//! kept by a `refs/nux/<commit>` ref. A commit `Nux.lock` pins is taken
//! from the mirror without any network access, and so is a file whose
//! locked checksum is in `downloads/`. What comes out of the cache is
//! checked like a download: commits by id, files by SHA-256.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::Workspace;

impl Workspace {
    pub fn cache_dir(&self) -> PathBuf {
        self.home().join("cache")
    }

    /// The bare mirror of the git repository at `url`.
    pub(crate) fn git_mirror(&self, url: &str) -> PathBuf {
        let name = url
            .trim_end_matches('/')
            .rsplit(['/', ':'])
            .next()
            .unwrap_or("repository");
        let name: String = name
            .trim_end_matches(".git")
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || "-_.".contains(c) {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        self.cache_dir()
            .join("git")
            .join(format!("{name}-{}.git", &hash(url)[..16]))
    }

    /// Where a downloaded file with this SHA-256 is kept.
    pub(crate) fn cached_download(&self, sha256: &str) -> PathBuf {
        self.cache_dir()
            .join("downloads")
            .join("sha256")
            .join(sha256)
    }
}

/// Holds `<path>.lock` while alive: one fetch per mirror at a time, between
/// download threads (each opens its own file, so they exclude each other).
pub(crate) struct CacheLock(File);

impl CacheLock {
    pub fn take(path: &Path) -> Result<Self, String> {
        let lock = path.with_extension("lock");
        let context = |e: std::io::Error| format!("cannot lock `{}`: {e}", lock.display());
        if let Some(parent) = lock.parent() {
            std::fs::create_dir_all(parent).map_err(context)?;
        }
        let file = File::options()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&lock)
            .map_err(context)?;
        file.lock().map_err(context)?;
        Ok(Self(file))
    }
}

impl Drop for CacheLock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

/// Creates the bare mirror `dir` if it does not exist yet.
pub(crate) fn init_mirror(dir: &Path) -> Result<(), String> {
    if dir.join("HEAD").is_file() {
        return Ok(());
    }
    // Created aside, then moved into place: never half a repository.
    let tmp = dir.with_extension(format!("tmp-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    git(None, &["init", "--quiet", "--bare"], Some(&tmp))?;
    std::fs::rename(&tmp, dir).map_err(|e| format!("cannot create `{}`: {e}", dir.display()))
}

/// Whether the mirror has `commit`.
pub(crate) fn has_commit(mirror: &Path, commit: &str) -> bool {
    git(
        Some(mirror),
        &["cat-file", "-e", &format!("{commit}^{{commit}}")],
        None,
    )
    .is_ok()
}

/// The commit `reference` points at in the mirror.
pub(crate) fn resolve(mirror: &Path, reference: &str) -> Result<String, String> {
    git(
        Some(mirror),
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{reference}^{{commit}}"),
        ],
        None,
    )
}

/// Keeps `commit` in the mirror for good, under `refs/nux/<commit>`.
pub(crate) fn keep(mirror: &Path, commit: &str) -> Result<(), String> {
    git(
        Some(mirror),
        &["update-ref", &format!("refs/nux/{commit}"), commit],
        None,
    )
    .map(|_| ())
}

/// Creates the repository `target` with `commit` checked out, copying only
/// that commit's objects from the mirror. No network access.
pub(crate) fn check_out(mirror: &Path, commit: &str, target: &Path) -> Result<(), String> {
    git(None, &["init", "--quiet"], Some(target))?;
    let mirror = mirror.to_string_lossy();
    git(
        Some(target),
        &[
            "fetch",
            "--quiet",
            "--no-tags",
            "--depth",
            "1",
            &mirror,
            &format!("refs/nux/{commit}"),
        ],
        None,
    )?;
    git(
        Some(target),
        &["checkout", "--quiet", "--detach", commit],
        None,
    )?;
    let head = git(Some(target), &["rev-parse", "HEAD"], None)?;
    if head != commit {
        return Err(format!(
            "the mirror `{mirror}` gave {head} instead of {commit}"
        ));
    }
    Ok(())
}

/// Runs `git [-C dir] <args> [path]`; its trimmed stdout, or its stderr as
/// the error.
fn git(dir: Option<&Path>, args: &[&str], path: Option<&Path>) -> Result<String, String> {
    let mut command = Command::new("git");
    if let Some(dir) = dir {
        command.arg("-C").arg(dir);
    }
    command.args(args);
    if let Some(path) = path {
        command.arg(path);
    }
    let output = command
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("cannot run `git`: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "`git {}` failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Puts `file` (already verified to have `sha256`) in the download cache.
/// Best effort: a failure only means the next download is not cached.
pub(crate) fn store_download(workspace: &Workspace, file: &Path, sha256: &str) {
    let target = workspace.cached_download(sha256);
    if target.is_file() {
        return;
    }
    let Some(dir) = target.parent() else { return };
    let tmp = target.with_extension(format!("tmp-{}", std::process::id()));
    let stored = std::fs::create_dir_all(dir)
        .and_then(|()| {
            std::fs::hard_link(file, &tmp).or_else(|_| std::fs::copy(file, &tmp).map(|_| ()))
        })
        .and_then(|()| std::fs::rename(&tmp, &target));
    if stored.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

/// Copies the cached download with `sha256` to `to`, if the cache has it
/// intact. A damaged entry is removed.
pub(crate) fn take_download(workspace: &Workspace, sha256: &str, to: &Path) -> bool {
    let cached = workspace.cached_download(sha256);
    match crate::fetch::sha256_file(&cached) {
        Ok(actual) if actual == sha256 => {}
        Ok(_) => {
            let _ = std::fs::remove_file(&cached);
            return false;
        }
        Err(_) => return false,
    }
    std::fs::hard_link(&cached, to)
        .or_else(|_| std::fs::copy(&cached, to).map(|_| ()))
        .is_ok()
}

/// SHA-256 of `text`, hex encoded.
fn hash(text: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Makes everything under `path` writable by its owner, so it can be
/// removed: Go makes its module cache read-only.
pub(crate) fn make_writable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let Ok(metadata) = path.symlink_metadata() else {
        return;
    };
    if metadata.file_type().is_symlink() {
        return;
    }
    let mut permissions = metadata.permissions();
    permissions.set_mode(permissions.mode() | 0o200 | if metadata.is_dir() { 0o100 } else { 0 });
    let _ = std::fs::set_permissions(path, permissions);
    if metadata.is_dir()
        && let Ok(entries) = std::fs::read_dir(path)
    {
        for entry in entries.filter_map(|e| e.ok()) {
            make_writable(&entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mirrors_are_named_after_the_repository_and_its_url() {
        let workspace = Workspace::new("/nux");
        let mirror = workspace.git_mirror("https://github.com/madler/zlib.git");
        let name = mirror.file_name().unwrap().to_string_lossy().into_owned();
        assert!(
            name.starts_with("zlib-")
                && name.ends_with(".git")
                && name.len() == "zlib-".len() + 16 + 4,
            "{name}"
        );
        assert_eq!(mirror.parent().unwrap(), Path::new("/nux/cache/git"));
        assert_ne!(
            mirror,
            workspace.git_mirror("https://example.com/other/zlib.git")
        );
        assert_eq!(
            mirror,
            workspace.git_mirror("https://github.com/madler/zlib.git")
        );
        let odd = workspace.git_mirror("git@host:team/we ird/");
        assert!(
            odd.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("we_ird-"),
            "{}",
            odd.display()
        );
    }
}
