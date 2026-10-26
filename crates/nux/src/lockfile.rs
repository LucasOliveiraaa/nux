//! `Nux.lock`: the exact resolution of an environment.
//!
//! It sits next to `Nux.toml` and records, for every package version the
//! environment needs, its dependencies (as exact versions) and the identity
//! of its source: the git commit or the SHA-256 of the downloaded file. With
//! it, a later `nux sync` keeps choosing the same versions while they still
//! satisfy the requirements, and only accepts sources with the same
//! identity.
//!
//! This is not the workspace lock (`$NUX_HOME/.lock`), which only keeps two
//! processes from modifying a workspace at the same time.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use nux_build::{BuildPlan, SourceIdentity, Workspace};
use nux_eval::{Source, SourceKind};
use nux_resolver::{ResolvedPackage, Resolver};
use toml::{Table, Value};

use crate::environment::Environment;

pub const FILE_NAME: &str = "Nux.lock";
const FORMAT_VERSION: i64 = 1;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Lock {
    /// Sorted by name, then version.
    pub packages: Vec<LockedPackage>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LockedPackage {
    pub name: String,
    pub version: String,
    pub requested: bool,
    /// `name version` of each dependency, sorted.
    pub dependencies: Vec<String>,
    /// The source declaration: `git+<url>[#<tag>]`, `url+<url>`, `local+<path>`.
    pub source: Option<String>,
    /// The git commit the source was cloned at.
    pub revision: Option<String>,
    /// `sha256:<hex>` of a downloaded file.
    pub checksum: Option<String>,
    /// The host executable, for versions the host provides.
    pub host: Option<String>,
}

impl LockedPackage {
    fn key(&self) -> (&str, &str) {
        (&self.name, &self.version)
    }

    fn identity(&self) -> SourceIdentity {
        SourceIdentity {
            revision: self.revision.clone(),
            sha256: self
                .checksum
                .as_ref()
                .and_then(|c| c.strip_prefix("sha256:"))
                .map(str::to_string),
        }
    }
}

/// How a recipe's source is written in the lock.
pub fn source_id(source: &Source) -> String {
    match &source.kind {
        SourceKind::Git(url) => match &source.tag {
            Some(tag) => format!("git+{url}#{tag}"),
            None => format!("git+{url}"),
        },
        SourceKind::Url(url) => format!("url+{url}"),
        SourceKind::Local(path) => format!("local+{path}"),
    }
}

impl Lock {
    pub fn path(env: &Environment) -> PathBuf {
        env.root().join(FILE_NAME)
    }

    /// Reads the lock at `path`; `None` when there is none.
    pub fn read(path: &Path) -> Result<Option<Self>, String> {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::parse(&text)
                .map(Some)
                .map_err(|e| format!("invalid `{}`: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("cannot read `{}`: {e}", path.display())),
        }
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let table: Table = text
            .parse()
            .map_err(|e: toml::de::Error| e.message().to_string())?;
        match table.get("version") {
            Some(Value::Integer(FORMAT_VERSION)) => {}
            Some(other) => return Err(format!("unsupported lock format version {other}")),
            None => return Err("missing `version`".into()),
        }
        let entries = match table.get("package") {
            None => return Ok(Self::default()),
            Some(Value::Array(entries)) => entries,
            Some(_) => return Err("`package` must be an array of tables".into()),
        };
        let mut packages = entries
            .iter()
            .map(|entry| {
                let Value::Table(entry) = entry else {
                    return Err("`package` entries must be tables".to_string());
                };
                let string = |key: &str| -> Result<Option<String>, String> {
                    match entry.get(key) {
                        None => Ok(None),
                        Some(Value::String(s)) => Ok(Some(s.clone())),
                        Some(_) => Err(format!("`{key}` must be a string")),
                    }
                };
                let dependencies = match entry.get("dependencies") {
                    None => Vec::new(),
                    Some(Value::Array(items)) => items
                        .iter()
                        .map(|i| {
                            i.as_str()
                                .map(str::to_string)
                                .ok_or("`dependencies` must hold strings")
                        })
                        .collect::<Result<_, _>>()?,
                    Some(_) => return Err("`dependencies` must be an array".into()),
                };
                Ok(LockedPackage {
                    name: string("name")?.ok_or("a package has no `name`")?,
                    version: string("version")?.ok_or("a package has no `version`")?,
                    requested: entry
                        .get("requested")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                    dependencies,
                    source: string("source")?,
                    revision: string("revision")?,
                    checksum: string("checksum")?,
                    host: string("host")?,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        packages.sort_by(|a, b| a.key().cmp(&b.key()));
        Ok(Self { packages })
    }

    /// The file's text: stable and sorted, so it diffs well.
    pub fn to_toml(&self) -> String {
        let quote = |s: &str| Value::String(s.to_string()).to_string();
        let mut out = String::from(
            "# Generated by `nux sync`: the exact versions and sources this environment\n\
             # resolved to. Commit it to reproduce them; do not edit it by hand.\n",
        );
        out.push_str(&format!("version = {FORMAT_VERSION}\n"));
        for package in &self.packages {
            out.push_str("\n[[package]]\n");
            out.push_str(&format!("name = {}\n", quote(&package.name)));
            out.push_str(&format!("version = {}\n", quote(&package.version)));
            if package.requested {
                out.push_str("requested = true\n");
            }
            for (key, value) in [
                ("source", &package.source),
                ("revision", &package.revision),
                ("checksum", &package.checksum),
                ("host", &package.host),
            ] {
                if let Some(value) = value {
                    out.push_str(&format!("{key} = {}\n", quote(value)));
                }
            }
            if !package.dependencies.is_empty() {
                out.push_str("dependencies = [\n");
                for dependency in &package.dependencies {
                    out.push_str(&format!("    {},\n", quote(dependency)));
                }
                out.push_str("]\n");
            }
        }
        out
    }

    /// Replaces the file at `path` atomically.
    pub fn write(&self, path: &Path) -> Result<(), String> {
        nux_build::write_atomic(path, self.to_toml().as_bytes()).map_err(|e| e.to_string())
    }

    /// Makes the resolver keep the locked versions while they match.
    pub fn prefer_in(&self, resolver: &mut Resolver) {
        for package in &self.packages {
            resolver.prefer(&package.name, &package.version);
        }
    }

    /// The source identities `packages` must have: those locked for the same
    /// version with the same source declaration. A changed declaration is
    /// a new source, so its old identity does not apply.
    pub fn pins(&self, packages: &[ResolvedPackage]) -> HashMap<(String, String), SourceIdentity> {
        packages
            .iter()
            .filter_map(|package| {
                let locked = self.get(&package.name, &package.version)?;
                let source = package.source.as_ref().map(source_id);
                (locked.source == source && package.host.is_none())
                    .then(|| {
                        (
                            (package.name.clone(), package.version.clone()),
                            locked.identity(),
                        )
                    })
                    .filter(|(_, identity)| !identity.is_empty())
            })
            .collect()
    }

    fn get(&self, name: &str, version: &str) -> Option<&LockedPackage> {
        self.packages.iter().find(|p| p.key() == (name, version))
    }

    /// The lock describing `plan`. Source identities come from `previous`
    /// when it locked the same source, otherwise from the workspace.
    pub fn from_plan(plan: &BuildPlan, workspace: &Workspace, previous: Option<&Lock>) -> Self {
        let mut packages: Vec<LockedPackage> = plan
            .steps
            .iter()
            .map(|step| {
                let package = &step.package;
                let source = package
                    .source
                    .as_ref()
                    .filter(|_| package.host.is_none())
                    .map(source_id);
                let locked = previous
                    .and_then(|lock| lock.get(&package.name, &package.version))
                    .filter(|locked| locked.source == source)
                    .map(LockedPackage::identity)
                    .filter(|identity| !identity.is_empty());
                let identity = match (&source, locked) {
                    (None, _) => SourceIdentity::default(),
                    (Some(_), Some(locked)) => locked,
                    (Some(_), None) => workspace
                        .source_identity(&package.name, &package.version)
                        .unwrap_or_default(),
                };
                let mut dependencies: Vec<String> = package
                    .dependencies
                    .iter()
                    .map(|d| format!("{} {}", d.name, d.version))
                    .collect();
                dependencies.sort();
                dependencies.dedup();
                LockedPackage {
                    name: package.name.clone(),
                    version: package.version.clone(),
                    requested: step.requested,
                    dependencies,
                    source,
                    revision: identity.revision,
                    checksum: identity.sha256.map(|sha| format!("sha256:{sha}")),
                    host: step.host().map(|path| path.display().to_string()),
                }
            })
            .collect();
        packages.sort_by(|a, b| a.key().cmp(&b.key()));
        Self { packages }
    }

    /// The first way `new` differs from `self`, worded for an error message,
    /// or `None` when they are the same.
    pub fn difference(&self, new: &Lock) -> Option<String> {
        let by_name = |lock: &Lock| {
            let mut map: BTreeMap<String, Vec<String>> = BTreeMap::new();
            for p in &lock.packages {
                map.entry(p.name.clone())
                    .or_default()
                    .push(p.version.clone());
            }
            map
        };
        let (old_names, new_names) = (by_name(self), by_name(new));
        for (name, new_versions) in &new_names {
            match old_names.get(name) {
                None => {
                    return Some(format!(
                        "`{name}` {} would be added",
                        new_versions.join(", ")
                    ));
                }
                Some(old_versions) if old_versions != new_versions => {
                    return Some(format!(
                        "`{name}` would change from {} to {}",
                        old_versions.join(", "),
                        new_versions.join(", ")
                    ));
                }
                Some(_) => {}
            }
        }
        if let Some((name, versions)) = old_names
            .iter()
            .find(|(name, _)| !new_names.contains_key(*name))
        {
            return Some(format!("`{name}` {} would be removed", versions.join(", ")));
        }
        for (old, new) in self.packages.iter().zip(&new.packages) {
            let what = if old.dependencies != new.dependencies {
                "dependencies"
            } else if old.source != new.source {
                "source"
            } else if old.revision != new.revision || old.checksum != new.checksum {
                "source revision or checksum"
            } else if old.host != new.host {
                "host executable"
            } else if old.requested != new.requested {
                "requested packages"
            } else {
                continue;
            };
            return Some(format!(
                "the {what} of `{}` {} would change",
                new.name, new.version
            ));
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn package(name: &str, version: &str) -> LockedPackage {
        LockedPackage {
            name: name.into(),
            version: version.into(),
            ..Default::default()
        }
    }

    fn sample() -> Lock {
        Lock {
            packages: vec![
                LockedPackage {
                    requested: true,
                    dependencies: vec!["gcc 14.2.0".into(), "zlib 1.3.1".into()],
                    source: Some("git+https://example.com/app.git#v1".into()),
                    revision: Some("0123abcd".into()),
                    ..package("app", "1.0")
                },
                LockedPackage {
                    host: Some("/usr/bin/gcc".into()),
                    ..package("gcc", "14.2.0")
                },
                LockedPackage {
                    source: Some("url+https://example.com/zlib.tar.gz".into()),
                    checksum: Some("sha256:abc".into()),
                    ..package("zlib", "1.3.1")
                },
            ],
        }
    }

    #[test]
    fn round_trips_through_its_text() {
        let lock = sample();
        let text = lock.to_toml();
        assert!(text.contains(
            "[[package]]\nname = \"app\"\nversion = \"1.0\"\nrequested = true\nsource = \"git+https://example.com/app.git#v1\"\nrevision = \"0123abcd\"\ndependencies = [\n    \"gcc 14.2.0\",\n    \"zlib 1.3.1\",\n]\n"
        ), "{text}");
        assert_eq!(Lock::parse(&text).unwrap(), lock);
        assert_eq!(Lock::parse("version = 1").unwrap(), Lock::default());
    }

    #[test]
    fn rejects_malformed_locks() {
        assert_eq!(Lock::parse("").unwrap_err(), "missing `version`");
        assert_eq!(
            Lock::parse("version = 2").unwrap_err(),
            "unsupported lock format version 2"
        );
        assert_eq!(
            Lock::parse("version = 1\n[[package]]\nversion = \"1\"").unwrap_err(),
            "a package has no `name`"
        );
        assert_eq!(
            Lock::parse(
                "version = 1\n[[package]]\nname = \"a\"\nversion = \"1\"\ndependencies = [1]"
            )
            .unwrap_err(),
            "`dependencies` must hold strings"
        );
    }

    #[test]
    fn describes_the_first_difference() {
        let old = sample();
        assert_eq!(old.difference(&old), None);

        let mut new = old.clone();
        new.packages[2].version = "1.3.2".into();
        assert_eq!(
            old.difference(&new).unwrap(),
            "`zlib` would change from 1.3.1 to 1.3.2"
        );

        let mut new = old.clone();
        new.packages.remove(1);
        assert_eq!(
            old.difference(&new).unwrap(),
            "`gcc` 14.2.0 would be removed"
        );
        assert_eq!(new.difference(&old).unwrap(), "`gcc` 14.2.0 would be added");

        let mut new = old.clone();
        new.packages[0].revision = Some("ffff".into());
        assert_eq!(
            old.difference(&new).unwrap(),
            "the source revision or checksum of `app` 1.0 would change"
        );

        let mut new = old.clone();
        new.packages[0].dependencies.pop();
        assert_eq!(
            old.difference(&new).unwrap(),
            "the dependencies of `app` 1.0 would change"
        );
    }

    #[test]
    fn source_ids() {
        let source = |kind, tag: Option<&str>| Source {
            kind,
            tag: tag.map(Into::into),
            proxy: Some("p".into()),
        };
        assert_eq!(
            source_id(&source(SourceKind::Git("u".into()), Some("v1"))),
            "git+u#v1"
        );
        assert_eq!(
            source_id(&source(SourceKind::Url("u".into()), None)),
            "url+u"
        );
        assert_eq!(
            source_id(&source(SourceKind::Local("src".into()), None)),
            "local+src"
        );
    }
}
