//! Turns evaluated package definitions into an ordered build plan.
//!
//! Resolution runs in four separate steps:
//!
//! 1. [`Resolver::build_graph`] links every package version to the versions
//!    its dependencies select ([`graph`]).
//! 2. [`prune`] keeps only the versions reachable from the requested ones.
//! 3. [`order::find_cycle`] rejects dependency cycles.
//! 4. [`order::build_order`] sorts what is left so dependencies come first.
//!
//! [`Resolver::resolve`] runs all four and returns [`ResolvedPackage`]s.

use std::collections::{HashMap, HashSet};

use nux_eval::{Dependency, EvalResult, HostBinary, Library, Source, VersionDef};

pub mod graph;
pub mod order;
pub mod prune;
mod version;

pub use graph::{Edge, VersionGraph};
pub use version::{Version, VersionReq};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PackageVersionId(u32);

impl PackageVersionId {
    fn index(self) -> usize {
        self.0 as usize
    }
}

pub struct PackageVersion {
    pub id: PackageVersionId,
    pub version: String,
    pub source: Option<Source>,
    pub dependencies: HashSet<Dependency>,
    pub commands: Vec<String>,
    pub bin: Option<String>,
    /// Libraries registered by `export_lib`.
    pub libraries: Vec<Library>,
    /// Set when the host already provides this version.
    pub host: Option<HostBinary>,
    parsed: Version,
}

pub struct Package {
    pub name: String,
    pub versions: Vec<PackageVersion>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum DependencyKind {
    Build,
    Runtime,
}

/// A package the environment asks for, with a requirement in the same
/// syntax as `add_dependency` (`latest`, `4.2.0`, `>=1, <2`, ...).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub name: String,
    pub version: String,
}

impl Request {
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
        }
    }
}

/// A dependency of a [`ResolvedPackage`], pinned to one version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedDependency {
    pub id: PackageVersionId,
    pub name: String,
    pub version: String,
    /// The requirement as written in the definition.
    pub requirement: String,
    pub kind: DependencyKind,
}

/// One entry of the build plan.
#[derive(Debug, Clone)]
pub struct ResolvedPackage {
    pub id: PackageVersionId,
    pub name: String,
    pub version: String,
    pub source: Option<Source>,
    pub commands: Vec<String>,
    /// The executable registered by `set_bin`, relative to the install prefix.
    pub bin: Option<String>,
    /// Libraries registered by `export_lib`.
    pub libraries: Vec<Library>,
    /// Set when the host already provides this version.
    pub host: Option<HostBinary>,
    /// Sorted by name, then requirement; each one appears earlier in the plan.
    pub dependencies: Vec<ResolvedDependency>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveError {
    /// The same version of a package was registered twice.
    DuplicateVersion { name: String, version: String },
    /// A declared version string cannot be parsed.
    InvalidVersion { name: String, version: String, reason: String },
    /// A requirement string cannot be parsed.
    InvalidRequirement { name: String, requirement: String, reason: String },
    /// No package with this name is defined.
    UnknownPackage { name: String },
    /// The package exists, but none of its versions satisfy the requirement.
    NoMatchingVersion { name: String, requirement: String, available: Vec<String> },
    /// `error` happened while resolving a dependency of `name` `version`.
    InDependency { name: String, version: String, error: Box<ResolveError> },
    /// Each entry depends on the next; the last one is the first again.
    Cycle(Vec<(String, String)>),
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResolveError::DuplicateVersion { name, version } => {
                write!(f, "version `{version}` of `{name}` is defined more than once")
            }
            ResolveError::InvalidVersion { name, version, reason } => {
                write!(f, "invalid version `{version}` of `{name}`: {reason}")
            }
            ResolveError::InvalidRequirement { name, requirement, reason } => {
                write!(f, "invalid version requirement `{requirement}` for `{name}`: {reason}")
            }
            ResolveError::UnknownPackage { name } => write!(f, "package `{name}` is not defined"),
            ResolveError::NoMatchingVersion { name, requirement, available } => write!(
                f,
                "no version of `{name}` satisfies `{requirement}` (available: {})",
                available.join(", ")
            ),
            ResolveError::InDependency { name, version, error } => {
                write!(f, "in dependencies of `{name}` {version}: {error}")
            }
            ResolveError::Cycle(path) => {
                f.write_str("dependency cycle: ")?;
                for (i, (name, version)) in path.iter().enumerate() {
                    if i > 0 {
                        f.write_str(" -> ")?;
                    }
                    write!(f, "{name} {version}")?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for ResolveError {}

/// The set of known package definitions.
pub struct Resolver {
    packages: Vec<Package>,
    by_name: HashMap<String, usize>,
    /// `(package index, version index)` for each id.
    locations: Vec<(usize, usize)>,
    /// Versions to keep choosing while they still match (from `Nux.lock`).
    preferred: HashSet<PackageVersionId>,
}

impl Resolver {
    pub fn new() -> Self {
        Self {
            packages: Vec::new(),
            by_name: HashMap::new(),
            locations: Vec::new(),
            preferred: HashSet::new(),
        }
    }

    /// Registers every version of an evaluated program.
    pub fn add_package(&mut self, result: EvalResult) -> Result<Vec<PackageVersionId>, ResolveError> {
        result
            .versions
            .into_iter()
            .map(|def| self.add_version(result.name.clone(), def))
            .collect()
    }

    pub fn add_version(&mut self, name: String, def: VersionDef) -> Result<PackageVersionId, ResolveError> {
        let parsed = Version::parse(&def.version).map_err(|reason| ResolveError::InvalidVersion {
            name: name.clone(),
            version: def.version.clone(),
            reason,
        })?;

        let package_index = match self.by_name.get(&name) {
            Some(&index) => index,
            None => {
                self.packages.push(Package {
                    name: name.clone(),
                    versions: Vec::new(),
                });
                self.by_name.insert(name.clone(), self.packages.len() - 1);
                self.packages.len() - 1
            }
        };
        let package = &mut self.packages[package_index];
        if package.versions.iter().any(|v| v.version == def.version) {
            return Err(ResolveError::DuplicateVersion {
                name,
                version: def.version,
            });
        }

        let id = PackageVersionId(self.locations.len() as u32);
        self.locations.push((package_index, package.versions.len()));
        package.versions.push(PackageVersion {
            id,
            version: def.version,
            source: def.source,
            dependencies: def.dependencies,
            commands: def.commands,
            bin: def.bin,
            libraries: def.libraries,
            host: def.host,
            parsed,
        });
        Ok(id)
    }

    pub fn packages(&self) -> &[Package] {
        &self.packages
    }

    pub fn package(&self, name: &str) -> Option<&Package> {
        self.by_name.get(name).map(|&i| &self.packages[i])
    }

    /// The package name and version behind `id`.
    pub fn get(&self, id: PackageVersionId) -> (&Package, &PackageVersion) {
        let (package, version) = self.locations[id.index()];
        let package = &self.packages[package];
        (package, &package.versions[version])
    }

    pub(crate) fn len(&self) -> usize {
        self.locations.len()
    }

    /// Makes `name` `version` win over other matching versions, so that a
    /// locked resolution stays the same when newer versions appear. Unknown
    /// versions are ignored. Returns whether the version exists.
    pub fn prefer(&mut self, name: &str, version: &str) -> bool {
        let id = self.package(name).and_then(|p| p.versions.iter().find(|v| v.version == version)).map(|v| v.id);
        if let Some(id) = id {
            self.preferred.insert(id);
        }
        id.is_some()
    }

    /// Picks the version of `name` to use for `requirement`: among the
    /// versions that satisfy it, a [preferred](Self::prefer) one wins, then
    /// one the host provides (it needs no build), then the highest.
    pub fn select(&self, name: &str, requirement: &str) -> Result<PackageVersionId, ResolveError> {
        let req = VersionReq::parse(requirement).map_err(|reason| ResolveError::InvalidRequirement {
            name: name.to_string(),
            requirement: requirement.to_string(),
            reason,
        })?;
        let package = self.package(name).ok_or_else(|| ResolveError::UnknownPackage {
            name: name.to_string(),
        })?;
        package
            .versions
            .iter()
            .filter(|v| req.matches(&v.parsed))
            .max_by_key(|v| (self.preferred.contains(&v.id), v.host.is_some(), &v.parsed))
            .map(|v| v.id)
            .ok_or_else(|| {
                let mut available: Vec<_> = package.versions.iter().map(|v| &v.parsed).collect();
                available.sort();
                ResolveError::NoMatchingVersion {
                    name: name.to_string(),
                    requirement: requirement.to_string(),
                    available: available.into_iter().map(|v| v.to_string()).collect(),
                }
            })
    }

    pub fn build_graph(&self) -> VersionGraph {
        VersionGraph::build(self)
    }

    /// Selects a version for every request and returns everything that must
    /// be built for them, dependencies first, each version once.
    pub fn resolve(&self, requests: &[Request]) -> Result<Vec<ResolvedPackage>, ResolveError> {
        let roots = requests
            .iter()
            .map(|r| self.select(&r.name, &r.version))
            .collect::<Result<Vec<_>, _>>()?;

        let graph = self.build_graph();
        let required = prune::prune(&graph, &roots)?;
        if let Some(cycle) = order::find_cycle(&graph, &required) {
            return Err(ResolveError::Cycle(
                cycle.into_iter().map(|id| graph.name_version(id)).collect(),
            ));
        }
        let order = order::build_order(&graph, &required);
        Ok(order.into_iter().map(|id| self.resolved(&graph, id)).collect())
    }

    fn resolved(&self, graph: &VersionGraph, id: PackageVersionId) -> ResolvedPackage {
        let (package, version) = self.get(id);
        let dependencies = graph
            .edges(id)
            .iter()
            .map(|edge| {
                let target = *edge.target.as_ref().expect("pruning rejects unresolved edges");
                let (name, version) = graph.name_version(target);
                ResolvedDependency {
                    id: target,
                    name,
                    version,
                    requirement: edge.requirement.clone(),
                    kind: edge.kind,
                }
            })
            .collect();
        ResolvedPackage {
            id,
            name: package.name.clone(),
            version: version.version.clone(),
            source: version.source.clone(),
            commands: version.commands.clone(),
            bin: version.bin.clone(),
            libraries: version.libraries.clone(),
            host: version.host.clone(),
            dependencies,
        }
    }
}

impl Default for Resolver {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
