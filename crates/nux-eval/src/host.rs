//! Facts about the host machine that recipes can query: which executables
//! are already installed, and at which version.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{LazyLock, Mutex};

use regex::Regex;

/// The host executables visible to recipes.
///
/// Executables under an excluded directory (the Nux home) are not the
/// host's: they were built by Nux and must not satisfy their own recipes.
pub struct Host {
    search_path: Vec<PathBuf>,
    exclude: Vec<PathBuf>,
    /// `host_version` runs a process; each executable is asked once.
    versions: Mutex<HashMap<PathBuf, Option<String>>>,
}

impl Host {
    pub fn new(search_path: Vec<PathBuf>, exclude: Vec<PathBuf>) -> Self {
        let exclude = exclude.iter().map(|p| std::fs::canonicalize(p).unwrap_or_else(|_| p.clone())).collect();
        Self {
            search_path,
            exclude,
            versions: Mutex::new(HashMap::new()),
        }
    }

    /// Searches `$PATH`, ignoring everything under `exclude`.
    pub fn from_env(exclude: Vec<PathBuf>) -> Self {
        let search_path = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
        Self::new(search_path, exclude)
    }

    /// A host without executables, for reproducible evaluation.
    pub fn empty() -> Self {
        Self::new(Vec::new(), Vec::new())
    }

    /// The first executable named `name` on the search path.
    pub fn find(&self, name: &str) -> Option<PathBuf> {
        if name.is_empty() || name.contains('/') {
            return None;
        }
        self.search_path.iter().map(|dir| dir.join(name)).find(|path| {
            let Ok(real) = std::fs::canonicalize(path) else { return false };
            is_executable(&real) && !self.exclude.iter().any(|ex| real.starts_with(ex))
        })
    }

    /// The version `name --version` reports (`flag` replaces `--version`):
    /// the first `N.N[.N...]` in its output.
    pub fn version(&self, name: &str, flag: &str) -> Option<String> {
        let path = self.find(name)?;
        let key = path.join(flag);
        let mut versions = self.versions.lock().unwrap();
        versions.entry(key).or_insert_with(|| detect_version(&path, flag)).clone()
    }
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

static VERSION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\d+(\.\d+)+").unwrap());

fn detect_version(path: &Path, flag: &str) -> Option<String> {
    let output = Command::new(path)
        .arg(flag)
        .stdin(Stdio::null())
        .output()
        .ok()?;
    // Some tools print their version to stderr.
    let text = String::from_utf8_lossy(&output.stdout).into_owned() + &String::from_utf8_lossy(&output.stderr);
    text.lines().find_map(|line| VERSION.find(line)).map(|m| m.as_str().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn fake_bin(dir: &Path, name: &str, version_output: &str) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\necho '{version_output}'\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nux-host-tests-{}", std::process::id())).join(name);
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn finds_executables_in_order_and_detects_versions() {
        let dir = temp("find");
        fake_bin(&dir.join("a"), "gcc", "gcc (Debian 14.2.0-19) 14.2.0");
        fake_bin(&dir.join("b"), "gcc", "gcc 9.9.9");
        std::fs::write(dir.join("a/notexec"), "").unwrap();
        let host = Host::new(vec![dir.join("missing"), dir.join("a"), dir.join("b")], vec![]);

        assert_eq!(host.find("gcc"), Some(dir.join("a/gcc")));
        assert_eq!(host.version("gcc", "--version").as_deref(), Some("14.2.0"));
        assert_eq!(host.find("notexec"), None);
        assert_eq!(host.find("../a/gcc"), None);
        assert_eq!(host.version("nothing", "--version"), None);
    }

    #[test]
    fn excluded_directories_are_not_the_host() {
        let dir = temp("exclude");
        fake_bin(&dir.join("nux/packages/gcc/1/install/bin"), "gcc", "1.0");
        std::fs::create_dir_all(dir.join("nux/bin")).unwrap();
        std::os::unix::fs::symlink(dir.join("nux/packages/gcc/1/install/bin/gcc"), dir.join("nux/bin/gcc")).unwrap();
        fake_bin(&dir.join("usr"), "gcc", "12.1");

        let host = Host::new(vec![dir.join("nux/bin"), dir.join("usr")], vec![dir.join("nux")]);
        assert_eq!(host.find("gcc"), Some(dir.join("usr/gcc")));
        assert_eq!(host.version("gcc", "--version").as_deref(), Some("12.1"));
        assert_eq!(Host::empty().find("sh"), None);
    }
}
