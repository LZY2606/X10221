//! Gate lineage is keyed by the stable gate_id. Renaming a node changes only
//! its display name: identities, edges and population percentages are stable.

use std::collections::{BTreeMap, BTreeSet};

use crate::engine::model::{GateNode, GateTree};

pub fn parent_map(tree: &GateTree) -> BTreeMap<String, Option<String>> {
    tree.nodes
        .iter()
        .map(|n| (n.gate_id.clone(), None))
        .collect::<BTreeMap<_, _>>()
        .into_iter()
        .map(|(g, _)| {
            let parent = tree
                .edges
                .iter()
                .find(|e| e.gate_id == g)
                .and_then(|e| e.parent.clone());
            (g, parent)
        })
        .collect()
}

pub fn name_map(nodes: &[GateNode]) -> BTreeMap<String, String> {
    nodes.iter().map(|n| (n.gate_id.clone(), n.name.clone())).collect()
}

pub fn descendants(tree: &GateTree, gate_id: &str) -> BTreeSet<String> {
    let parents = parent_map(tree);
    let mut out = BTreeSet::new();
    let mut stack = vec![gate_id.to_string()];
    while let Some(g) = stack.pop() {
        for (child, parent) in &parents {
            if parent.as_deref() == Some(g.as_str()) && !out.contains(child) {
                out.insert(child.clone());
                stack.push(child.clone());
            }
        }
    }
    out
}

/// Structural identity of a gate tree: ordered gate ids plus edges. Two trees
/// that differ only in node names have the same structural identity.
pub fn structural_hash(tree: &GateTree) -> String {
    let mut ids: Vec<String> = tree.nodes.iter().map(|n| n.gate_id.clone()).collect();
    ids.sort();
    let mut edges: Vec<String> = tree
        .edges
        .iter()
        .map(|e| format!("{}->{}", e.gate_id, e.parent.clone().unwrap_or_else(|| "ROOT".into())))
        .collect();
    edges.sort();
    crate::engine::ids::members_hash(
        &[
            format!("nodes:{}", ids.join(",")),
            format!("edges:{}", edges.join(",")),
        ]
        .into_iter()
        .chain(ids)
        .collect::<Vec<_>>(),
    )
}

pub fn renamed_nodes(old: &GateTree, new: &GateTree) -> Vec<(String, String, String)> {
    let old_names = name_map(&old.nodes);
    let mut out = Vec::new();
    for n in &new.nodes {
        if let Some(prev) = old_names.get(&n.gate_id) {
            if prev != &n.name {
                out.push((n.gate_id.clone(), prev.clone(), n.name.clone()));
            }
        }
    }
    out
}
