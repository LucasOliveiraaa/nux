//! Cycle detection and build ordering over a pruned set of versions.

use std::collections::{BTreeSet, HashMap};

use crate::{PackageVersionId, VersionGraph};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mark {
    /// On the current DFS path.
    Active,
    Done,
}

/// Finds a dependency cycle among `nodes`, returned as the path that closes
/// it: `[a, b, a]` means `a` depends on `b`, which depends on `a`. A version
/// depending on itself is `[a, a]`. Edges leaving `nodes` are ignored.
pub fn find_cycle(graph: &VersionGraph, nodes: &BTreeSet<PackageVersionId>) -> Option<Vec<PackageVersionId>> {
    fn visit(
        graph: &VersionGraph,
        nodes: &BTreeSet<PackageVersionId>,
        id: PackageVersionId,
        marks: &mut HashMap<PackageVersionId, Mark>,
        path: &mut Vec<PackageVersionId>,
    ) -> Option<Vec<PackageVersionId>> {
        marks.insert(id, Mark::Active);
        path.push(id);
        for target in graph.targets(id).filter(|t| nodes.contains(t)) {
            match marks.get(&target) {
                Some(Mark::Active) => {
                    let start = path.iter().position(|&p| p == target).unwrap();
                    let mut cycle = path[start..].to_vec();
                    cycle.push(target);
                    return Some(cycle);
                }
                Some(Mark::Done) => {}
                None => {
                    if let Some(cycle) = visit(graph, nodes, target, marks, path) {
                        return Some(cycle);
                    }
                }
            }
        }
        path.pop();
        marks.insert(id, Mark::Done);
        None
    }

    let mut marks = HashMap::new();
    for &id in nodes {
        if !marks.contains_key(&id)
            && let Some(cycle) = visit(graph, nodes, id, &mut marks, &mut Vec::new())
        {
            return Some(cycle);
        }
    }
    None
}

/// Orders `nodes` so every version comes after the versions it depends on.
/// Among versions that are ready at the same time, the lowest name and then
/// version goes first, so the result does not depend on registration or
/// request order. `nodes` must be acyclic (see [`find_cycle`]).
pub fn build_order(graph: &VersionGraph, nodes: &BTreeSet<PackageVersionId>) -> Vec<PackageVersionId> {
    let mut pending: HashMap<PackageVersionId, usize> = HashMap::new();
    let mut dependents: HashMap<PackageVersionId, Vec<PackageVersionId>> = HashMap::new();
    for &id in nodes {
        let targets: Vec<_> = graph.targets(id).filter(|t| nodes.contains(t)).collect();
        pending.insert(id, targets.len());
        for target in targets {
            dependents.entry(target).or_default().push(id);
        }
    }

    let key = |id: PackageVersionId| {
        let (name, version) = graph.key(id);
        (name, version, id)
    };
    let mut ready: BTreeSet<_> = nodes.iter().filter(|id| pending[id] == 0).map(|&id| key(id)).collect();

    let mut order = Vec::with_capacity(nodes.len());
    while let Some((_, _, id)) = ready.pop_first() {
        order.push(id);
        for &dependent in dependents.get(&id).into_iter().flatten() {
            let count = pending.get_mut(&dependent).unwrap();
            *count -= 1;
            if *count == 0 {
                ready.insert(key(dependent));
            }
        }
    }
    debug_assert_eq!(order.len(), nodes.len(), "build_order called on a cyclic graph");
    order
}
