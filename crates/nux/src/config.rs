//! The global configuration directory: `$NUX_CONFIG_DIR`, otherwise
//! `$XDG_CONFIG_HOME/nux`, otherwise `~/.config/nux`.
//!
//! ```text
//! ~/.config/nux/
//! ├── config.toml     settings, and repositories every environment searches
//! ├── Nux.toml        the global environment
//! └── Nux.lock
//! ```
//!
//! ```toml
//! [nux]
//! home = "~/.nux"           # the workspace; $NUX_HOME takes precedence
//! jobs = 4                  # .nux files evaluated at the same time
//! parallel_builds = 4       # packages built at the same time
//!
//! [[repository]]            # searched after an environment's own
//! path = "~/recipes"        # relative to this directory, or absolute
//!
//! [sandbox]                 # how build commands are isolated
//! mode = "auto"             # "auto", "required" or "off"
//! network = true
//! shared_caches = false     # share cargo's and go's caches between builds
//! ```
//!
//! Everything is optional, including the directory itself.

use std::path::{Path, PathBuf};

use nux_build::{SandboxMode, SandboxPolicy, Workspace};
use toml::{Table, Value};

use crate::environment::{self, expand, no_unknown_keys, take_repositories, take_table};
use crate::repository;

pub const FILE_NAME: &str = "config.toml";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// The configuration directory; `None` when it cannot be located.
    pub dir: Option<PathBuf>,
    /// The workspace, unless `$NUX_HOME` is set.
    pub home: Option<PathBuf>,
    /// Repositories every environment searches after its own.
    pub repositories: Vec<PathBuf>,
    /// How many `.nux` files are evaluated at the same time.
    pub jobs: usize,
    /// How many packages are built at the same time.
    pub parallel_builds: usize,
    pub sandbox: SandboxPolicy,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            dir: None,
            home: None,
            repositories: Vec::new(),
            jobs: repository::DEFAULT_JOBS,
            parallel_builds: nux_build::DEFAULT_PARALLEL_BUILDS,
            sandbox: SandboxPolicy::default(),
        }
    }
}

impl Config {
    /// Loads the configuration of the directory the environment points at.
    pub fn from_env() -> Result<Self, String> {
        let var = |name| std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from);
        let dir = var("NUX_CONFIG_DIR")
            .or_else(|| var("XDG_CONFIG_HOME").map(|d| d.join("nux")))
            .or_else(|| var("HOME").map(|h| h.join(".config").join("nux")));
        match dir {
            Some(dir) => Self::load(&dir),
            None => Ok(Self::default()),
        }
    }

    /// Reads `dir/config.toml`; without one, nothing is configured.
    pub fn load(dir: &Path) -> Result<Self, String> {
        let mut config = Self { dir: Some(dir.to_path_buf()), ..Self::default() };
        let path = dir.join(FILE_NAME);
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(config),
            Err(e) => return Err(format!("failed to read configuration `{}`: {e}", path.display())),
        };
        config.parse(&text, dir).map_err(|e| format!("invalid configuration `{}`: {e}", path.display()))?;
        Ok(config)
    }

    fn parse(&mut self, text: &str, dir: &Path) -> Result<(), String> {
        let mut table: Table = text.parse().map_err(|e: toml::de::Error| e.message().to_string())?;
        let mut nux = take_table(&mut table, "nux")?;
        self.home = match nux.remove("home") {
            None => None,
            Some(Value::String(home)) if !home.trim().is_empty() => Some(expand(&home, dir)),
            Some(Value::String(_)) => return Err("`nux.home` must not be empty".into()),
            Some(other) => return Err(format!("`nux.home` must be a string, found {}", other.type_str())),
        };
        self.jobs = match nux.remove("jobs") {
            None => repository::DEFAULT_JOBS,
            Some(Value::Integer(jobs)) if jobs >= 1 => usize::try_from(jobs).unwrap_or(usize::MAX),
            Some(Value::Integer(_)) => return Err("`nux.jobs` must be at least 1".into()),
            Some(other) => return Err(format!("`nux.jobs` must be an integer, found {}", other.type_str())),
        };
        self.parallel_builds = match nux.remove("parallel_builds") {
            None => nux_build::DEFAULT_PARALLEL_BUILDS,
            Some(Value::Integer(builds)) if builds >= 1 => usize::try_from(builds).unwrap_or(usize::MAX),
            Some(Value::Integer(_)) => return Err("`nux.parallel_builds` must be at least 1".into()),
            Some(other) => return Err(format!("`nux.parallel_builds` must be an integer, found {}", other.type_str())),
        };
        no_unknown_keys(&nux, "nux.")?;
        self.repositories = take_repositories(&mut table, dir)?;

        let mut sandbox = take_table(&mut table, "sandbox")?;
        self.sandbox.mode = match sandbox.remove("mode") {
            None => SandboxMode::Auto,
            Some(Value::String(mode)) => match mode.as_str() {
                "auto" => SandboxMode::Auto,
                "required" => SandboxMode::Required,
                "off" => SandboxMode::Off,
                other => return Err(format!("`sandbox.mode` must be \"auto\", \"required\" or \"off\", not \"{other}\"")),
            },
            Some(other) => return Err(format!("`sandbox.mode` must be a string, found {}", other.type_str())),
        };
        self.sandbox.network = match sandbox.remove("network") {
            None => true,
            Some(Value::Boolean(network)) => network,
            Some(other) => return Err(format!("`sandbox.network` must be a boolean, found {}", other.type_str())),
        };
        self.sandbox.shared_caches = match sandbox.remove("shared_caches") {
            None => false,
            Some(Value::Boolean(shared)) => shared,
            Some(other) => return Err(format!("`sandbox.shared_caches` must be a boolean, found {}", other.type_str())),
        };
        no_unknown_keys(&sandbox, "sandbox.")?;
        no_unknown_keys(&table, "")
    }

    /// `config.toml`, if the directory is known.
    pub fn file(&self) -> Option<PathBuf> {
        self.dir.as_ref().map(|d| d.join(FILE_NAME))
    }

    /// The global environment's `Nux.toml`, if the directory is known.
    pub fn global_environment(&self) -> Option<PathBuf> {
        self.dir.as_ref().map(|d| d.join(environment::FILE_NAME))
    }

    /// `$NUX_HOME`, then `nux.home`, then `~/.nux`.
    pub fn workspace(&self) -> Result<Workspace, String> {
        let from_env = std::env::var_os("NUX_HOME").is_some_and(|h| !h.is_empty());
        match &self.home {
            Some(home) if !from_env => Ok(Workspace::new(home)),
            _ => Workspace::from_env().map_err(|e| e.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str, config: Option<&str>) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nux-config-tests-{}", std::process::id())).join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        if let Some(config) = config {
            std::fs::write(dir.join(FILE_NAME), config).unwrap();
        }
        dir
    }

    #[test]
    fn everything_is_optional() {
        let dir = dir("empty", None);
        let config = Config::load(&dir).unwrap();
        assert_eq!(config, Config { dir: Some(dir.clone()), ..Config::default() });
        assert_eq!(config.sandbox, SandboxPolicy { mode: SandboxMode::Auto, network: true, shared_caches: false });
        assert_eq!(config.jobs, 4);
        assert_eq!(config.parallel_builds, 4);
        assert_eq!(config.global_environment(), Some(dir.join("Nux.toml")));
        assert_eq!(Config::default().global_environment(), None);
    }

    #[test]
    fn reads_home_and_repositories() {
        let dir = dir("full", Some("[nux]\nhome = \"workspace\"\njobs = 16\nparallel_builds = 2\n\n[[repository]]\npath = \"recipes\"\n\n[[repository]]\npath = \"/abs\"\n\n[sandbox]\nmode = \"required\"\nnetwork = false\nshared_caches = true\n"));
        let config = Config::load(&dir).unwrap();
        assert_eq!(config.sandbox, SandboxPolicy { mode: SandboxMode::Required, network: false, shared_caches: true });
        assert_eq!(config.jobs, 16);
        assert_eq!(config.parallel_builds, 2);
        assert_eq!(config.home, Some(dir.join("workspace")));
        assert_eq!(config.repositories, [dir.join("recipes"), PathBuf::from("/abs")]);
    }

    #[test]
    fn rejects_malformed_configurations() {
        let err = |text: &str| {
            let dir = dir("bad", Some(text));
            Config::load(&dir).unwrap_err().split_once("`: ").unwrap().1.to_string()
        };
        assert_eq!(err("[nux]\nhome = 1"), "`nux.home` must be a string, found integer");
        assert_eq!(err("[nux]\nhome = \"\""), "`nux.home` must not be empty");
        assert_eq!(err("[nux]\nhom = \"x\""), "unknown key `nux.hom`");
        assert_eq!(err("[nux]\njobs = 0"), "`nux.jobs` must be at least 1");
        assert_eq!(err("[nux]\nparallel_builds = 0"), "`nux.parallel_builds` must be at least 1");
        assert_eq!(err("[nux]\njobs = \"4\""), "`nux.jobs` must be an integer, found string");
        assert_eq!(err("[packages]\njq = \"1\""), "unknown key `packages`");
        assert_eq!(err("[[repository]]\nurl = \"x\""), "every `[[repository]]` needs a `path`");
        assert_eq!(err("[sandbox]\nmode = \"strict\""), "`sandbox.mode` must be \"auto\", \"required\" or \"off\", not \"strict\"");
        assert_eq!(err("[sandbox]\nnetwork = \"no\""), "`sandbox.network` must be a boolean, found string");
        assert_eq!(err("[sandbox]\nnet = false"), "unknown key `sandbox.net`");
    }
}
