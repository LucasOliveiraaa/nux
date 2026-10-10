//! Environments: a `Nux.toml` file listing the packages to build and the
//! repositories their definitions come from.
//!
//! ```toml
//! [environment]
//! name = "school"           # defaults to the directory name
//!
//! [packages]
//! cmake = "4.2.0"           # any requirement `add_dependency` accepts
//! nvim = "latest"
//!
//! [[repository]]            # searched in order, relative to this file
//! path = "./repository"
//! ```
//!
//! Commands use `./Nux.toml` (or `-e <path>`), or the global environment in
//! the [configuration directory](crate::config) when there is none or with
//! `-g`. `nux install` and `nux remove` edit `[packages]`, keeping the rest
//! of the file as it is.

use std::path::{Path, PathBuf};

use nux_resolver::Request;
use toml::{Table, Value};

use crate::config::Config;

pub const FILE_NAME: &str = "Nux.toml";

/// Written by `nux install --global` when there is no global environment.
pub const GLOBAL_TEMPLATE: &str = r#"# The global environment: used by commands run outside of an environment,
# or with `--global`. `nux install -g` and `nux remove -g` edit `[packages]`.
[environment]
name = "global"

# Packages to build and the version to select: "latest", "1.0.0", "^1", ">=1, <2"...
[packages]

# Repositories configured in config.toml are searched after these.
"#;

/// Which environment a command uses: `-e <path>` or `-g`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Selection {
    pub path: Option<PathBuf>,
    pub global: bool,
}

impl Selection {
    /// The environment file to use, and whether it is the global one: with
    /// `-g`, the global environment; otherwise `-e <path>` or `./Nux.toml`,
    /// falling back to the global environment when `-e` was not given,
    /// there is no `./Nux.toml` and a global environment exists.
    pub fn locate(&self, config: &Config) -> Result<(PathBuf, bool), String> {
        let global = config.global_environment();
        if self.global {
            return global.map(|path| (path, true)).ok_or_else(|| {
                "cannot locate the global environment: none of NUX_CONFIG_DIR, XDG_CONFIG_HOME or HOME is set".into()
            });
        }
        let local = Environment::locate(self.path.as_deref());
        if self.path.is_none()
            && !local.exists()
            && let Some(global) = global.filter(|g| g.is_file())
        {
            return Ok((global, true));
        }
        Ok((local, false))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Environment {
    pub name: String,
    /// The environment file.
    pub path: PathBuf,
    /// Package definition directories, in priority order.
    pub repositories: Vec<PathBuf>,
    pub packages: Vec<Request>,
    /// The global environment, from the configuration directory.
    pub global: bool,
}

impl Environment {
    /// The file to load for `-e <path>`: `path` itself, or `path/Nux.toml`
    /// when it is a directory. Without `-e`, `./Nux.toml`.
    pub fn locate(path: Option<&Path>) -> PathBuf {
        match path {
            Some(path) if path.is_dir() => path.join(FILE_NAME),
            Some(path) => path.to_path_buf(),
            None => PathBuf::from(FILE_NAME),
        }
    }

    /// Loads the environment `selection` points at, with the configured
    /// repositories.
    pub fn select(selection: &Selection, config: &Config) -> Result<Self, String> {
        let (path, global) = selection.locate(config)?;
        if !path.exists() {
            if global {
                return Err(format!(
                    "there is no global environment `{}`; `nux install --global <package>` creates it",
                    path.display()
                ));
            }
            if selection.path.is_none() {
                return Err(format!(
                    "no `{FILE_NAME}` in the current directory and no global environment; \
                     create one with `nux init` or `nux install --global <package>`"
                ));
            }
        }
        Self::load_with(&path, global, config)
    }

    /// Loads `path` on its own, without configured repositories.
    pub fn load(path: &Path) -> Result<Self, String> {
        Self::load_with(path, false, &Config::default())
    }

    pub fn load_with(path: &Path, global: bool, config: &Config) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("failed to read environment `{}`: {e}", path.display()))?;
        Self::from_text(&text, path, global, config)
    }

    /// Parses `text` as the environment file `path`, then adds the
    /// repositories of `config` after its own.
    pub fn from_text(text: &str, path: &Path, global: bool, config: &Config) -> Result<Self, String> {
        let invalid = |e: String| format!("invalid environment `{}`: {e}", path.display());
        let mut env = Self::parse(text, path, global).map_err(invalid)?;
        for repository in &config.repositories {
            if !env.repositories.contains(repository) {
                env.repositories.push(repository.clone());
            }
        }
        if env.repositories.is_empty() {
            let mut message = "no repositories configured; add a `[[repository]]` with a `path`".to_string();
            if let Some(file) = config.file() {
                message.push_str(&format!(", here or in `{}`", file.display()));
            }
            return Err(invalid(message));
        }
        Ok(env)
    }

    /// The directory relative paths in the environment are resolved against.
    pub fn root(&self) -> &Path {
        self.path.parent().unwrap_or(Path::new(""))
    }

    fn parse(text: &str, path: &Path, global: bool) -> Result<Self, String> {
        let mut table: Table = text.parse().map_err(|e: toml::de::Error| e.message().to_string())?;
        let root = path.parent().unwrap_or(Path::new(""));

        let mut environment = take_table(&mut table, "environment")?;
        let name = match environment.remove("name") {
            Some(Value::String(name)) if !name.trim().is_empty() => name,
            Some(Value::String(_)) => return Err("`environment.name` must not be empty".into()),
            Some(other) => return Err(format!("`environment.name` must be a string, found {}", other.type_str())),
            None if global => "global".into(),
            None => default_name(root),
        };
        no_unknown_keys(&environment, "environment.")?;

        let repositories = take_repositories(&mut table, root)?;

        let packages = take_table(&mut table, "packages")?
            .into_iter()
            .map(|(name, requirement)| match requirement {
                Value::String(requirement) => Ok(Request::new(name, requirement)),
                other => Err(format!("the version of `{name}` must be a string, found {}", other.type_str())),
            })
            .collect::<Result<_, _>>()?;

        no_unknown_keys(&table, "")?;
        Ok(Self {
            name,
            path: path.to_path_buf(),
            repositories,
            packages,
            global,
        })
    }
}

/// `text` (an environment file) with `requests` set in `[packages]`.
/// Comments and layout are kept.
pub fn with_packages(text: &str, requests: &[Request]) -> Result<String, String> {
    let mut document: toml_edit::DocumentMut = text.parse().map_err(|e: toml_edit::TomlError| e.message().to_string())?;
    let packages = document
        .entry("packages")
        .or_insert(toml_edit::table())
        .as_table_like_mut()
        .ok_or("`packages` must be a table")?;
    for request in requests {
        let mut value = toml_edit::Value::from(&request.version);
        // A replaced requirement keeps its spacing and trailing comment.
        if let Some(old) = packages.get(&request.name).and_then(|item| item.as_value()) {
            *value.decor_mut() = old.decor().clone();
        }
        packages.insert(&request.name, toml_edit::Item::Value(value));
    }
    Ok(document.to_string())
}

/// `text` (an environment file) without `names` in `[packages]`.
pub fn without_packages(text: &str, names: &[String]) -> Result<String, String> {
    let mut document: toml_edit::DocumentMut = text.parse().map_err(|e: toml_edit::TomlError| e.message().to_string())?;
    if let Some(packages) = document.get_mut("packages").and_then(|p| p.as_table_like_mut()) {
        for name in names {
            packages.remove(name);
        }
    }
    Ok(document.to_string())
}

/// The `[[repository]]` entries of `table`, relative to `root`.
pub fn take_repositories(table: &mut Table, root: &Path) -> Result<Vec<PathBuf>, String> {
    match table.remove("repository") {
        None => Ok(Vec::new()),
        Some(Value::Array(items)) => items
            .into_iter()
            .map(|item| match item {
                Value::Table(mut repository) => {
                    let path = match repository.remove("path") {
                        Some(Value::String(dir)) => expand(&dir, root),
                        Some(other) => {
                            return Err(format!("`repository.path` must be a string, found {}", other.type_str()));
                        }
                        None => return Err("every `[[repository]]` needs a `path`".into()),
                    };
                    no_unknown_keys(&repository, "repository.")?;
                    Ok(path)
                }
                other => Err(format!("`repository` entries must be tables, found {}", other.type_str())),
            })
            .collect(),
        Some(other) => Err(format!(
            "`repository` must be an array of tables (`[[repository]]`), found {}",
            other.type_str()
        )),
    }
}

/// `path` relative to `root`; a leading `~/` stands for `$HOME`.
pub fn expand(path: &str, root: &Path) -> PathBuf {
    let home = std::env::var_os("HOME").filter(|h| !h.is_empty()).map(PathBuf::from);
    match (path.strip_prefix("~/").or((path == "~").then_some("")), home) {
        (Some(rest), Some(home)) => home.join(rest),
        _ => root.join(path),
    }
}

pub fn take_table(table: &mut Table, key: &str) -> Result<Table, String> {
    match table.remove(key) {
        None => Ok(Table::new()),
        Some(Value::Table(inner)) => Ok(inner),
        Some(other) => Err(format!("`{key}` must be a table, found {}", other.type_str())),
    }
}

pub fn no_unknown_keys(table: &Table, prefix: &str) -> Result<(), String> {
    match table.keys().next() {
        Some(key) => Err(format!("unknown key `{prefix}{key}`")),
        None => Ok(()),
    }
}

/// The name of the directory holding the environment file.
fn default_name(root: &Path) -> String {
    let dir = if root.as_os_str().is_empty() { std::env::current_dir().ok() } else { std::fs::canonicalize(root).ok() };
    dir.as_deref()
        .and_then(Path::file_name)
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "default".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<Environment, String> {
        Environment::from_text(text, Path::new("project/Nux.toml"), false, &Config::default()).map_err(|e| e.trim_start_matches("invalid environment `project/Nux.toml`: ").to_string())
    }

    #[test]
    fn parses_packages_and_repositories_in_order() {
        let env = parse(
            r#"
            [environment]
            name = "school"
            [packages]
            gcc = "latest"
            cmake = ">=4, <5"
            [[repository]]
            path = "local"
            [[repository]]
            path = "/abs"
            "#,
        )
        .unwrap();
        assert_eq!(env.name, "school");
        assert_eq!(env.repositories, [PathBuf::from("project/local"), PathBuf::from("/abs")]);
        assert_eq!(env.packages, [Request::new("cmake", ">=4, <5"), Request::new("gcc", "latest")]);
    }

    #[test]
    fn rejects_malformed_environments() {
        let repo = "\n[[repository]]\npath = \"r\"\n";
        let err = |text: &str| parse(text).unwrap_err();
        assert_eq!(err(""), "no repositories configured; add a `[[repository]]` with a `path`");
        assert_eq!(err("[[repository]]\nurl = \"x\""), "every `[[repository]]` needs a `path`");
        assert_eq!(err("[[repository]]\npath = 1"), "`repository.path` must be a string, found integer");
        assert_eq!(err("[[repository]]\npath = \"r\"\nbranch = \"x\""), "unknown key `repository.branch`");
        assert_eq!(err("[repository]\npath = \"r\""), "`repository` must be an array of tables (`[[repository]]`), found table");
        assert_eq!(err(&format!("[packages]\ngcc = 12{repo}")), "the version of `gcc` must be a string, found integer");
        assert_eq!(err(&format!("packages = 1{repo}")), "`packages` must be a table, found integer");
        assert_eq!(err(&format!("[environment]\nname = \"\"{repo}")), "`environment.name` must not be empty");
        assert_eq!(err(&format!("[environment]\nnam = \"x\"{repo}")), "unknown key `environment.nam`");
        assert_eq!(err(&format!("pakages = {{}}{repo}")), "unknown key `pakages`");
        assert!(parse("[packages\n").is_err());
    }

    #[test]
    fn locates_environment_file() {
        assert_eq!(Environment::locate(None), PathBuf::from("Nux.toml"));
        let dir = std::env::temp_dir();
        assert_eq!(Environment::locate(Some(&dir)), dir.join("Nux.toml"));
        assert_eq!(Environment::locate(Some(Path::new("dev.toml"))), PathBuf::from("dev.toml"));
    }
}
