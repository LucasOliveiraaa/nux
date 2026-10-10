//! Restricting the graph to what the requested versions need.

use std::collections::BTreeSet;

use crate::{PackageVersionId, ResolveError, VersionGraph};

/// The versions reachable from `roots`, roots included. Fails on the first
/// reachable dependency that could not be resolved.
pub fn prune(graph: &VersionGraph, roots: &[PackageVersionId]) -> Result<BTreeSet<PackageVersionId>, ResolveError> {
    let mut required = BTreeSet::new();
    let mut stack: Vec<_> = roots.iter().rev().copied().collect();

    while let Some(id) = stack.pop() {
        if !required.insert(id) {
            continue;
        }
        for edge in graph.edges(id) {
            match &edge.target {
                Ok(target) => stack.push(*target),
                Err(error) => return Err(in_dependency(graph, id, error)),
            }
        }
    }
    Ok(required)
}

fn in_dependency(graph: &VersionGraph, id: PackageVersionId, error: &ResolveError) -> ResolveError {
    let (name, version) = graph.name_version(id);
    ResolveError::InDependency {
        name,
        version,
        error: Box::new(error.clone()),
    }
}
