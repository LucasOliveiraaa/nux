//! The version graph: one node per package version, one edge per declared
//! dependency, pointing at the version that dependency selects.

use nux_eval::Dependency;

use crate::{DependencyKind, PackageVersionId, ResolveError, Resolver, Version};

/// A dependency of one package version.
#[derive(Debug, Clone)]
pub struct Edge {
    pub name: String,
    pub requirement: String,
    pub kind: DependencyKind,
    /// The selected version, or why none could be selected. Errors are kept
    /// rather than raised so that a broken definition only fails resolution
    /// when something actually requires it.
    pub target: Result<PackageVersionId, ResolveError>,
}

struct Node {
    name: String,
    version: Version,
    edges: Vec<Edge>,
}

pub struct VersionGraph {
    /// Indexed by [`PackageVersionId`].
    nodes: Vec<Node>,
}

impl VersionGraph {
    pub fn build(resolver: &Resolver) -> Self {
        let nodes = (0..resolver.len())
            .map(|i| {
                let (package, version) = resolver.get(PackageVersionId(i as u32));
                let mut edges: Vec<Edge> = version
                    .dependencies
                    .iter()
                    .map(|dependency| {
                        let (name, requirement, kind) = match dependency {
                            Dependency::Build { name, version } => (name, version, DependencyKind::Build),
                            Dependency::Runtime { name, version } => (name, version, DependencyKind::Runtime),
                        };
                        Edge {
                            name: name.clone(),
                            requirement: requirement.clone(),
                            kind,
                            target: resolver.select(name, requirement),
                        }
                    })
                    .collect();
                // Dependencies come from a `HashSet`; sort for determinism.
                edges.sort_by(|a, b| (&a.name, &a.requirement, a.kind).cmp(&(&b.name, &b.requirement, b.kind)));
                Node {
                    name: package.name.clone(),
                    version: version.parsed.clone(),
                    edges,
                }
            })
            .collect();
        Self { nodes }
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn edges(&self, id: PackageVersionId) -> &[Edge] {
        &self.nodes[id.index()].edges
    }

    /// The name and version of `id`; also the key that orders build steps.
    pub fn key(&self, id: PackageVersionId) -> (&str, &Version) {
        let node = &self.nodes[id.index()];
        (&node.name, &node.version)
    }

    pub fn name_version(&self, id: PackageVersionId) -> (String, String) {
        let (name, version) = self.key(id);
        (name.to_string(), version.to_string())
    }

    /// The versions `id` depends on, in edge order, skipping unresolved
    /// edges and repeats (a version can be both a build and runtime target).
    pub fn targets(&self, id: PackageVersionId) -> impl Iterator<Item = PackageVersionId> + '_ {
        let edges = self.edges(id);
        edges.iter().enumerate().filter_map(move |(i, edge)| {
            let target = *edge.target.as_ref().ok()?;
            let seen = edges[..i].iter().any(|e| e.target.as_ref() == Ok(&target));
            (!seen).then_some(target)
        })
    }
}
