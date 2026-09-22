//! Dependency-driven staleness. A source revision invalidates exactly the
//! populations whose dependency chain reaches it; old conclusions are never
//! globally replaced, they are flagged stale with explicit old/new versions.

use std::collections::BTreeSet;

use crate::engine::model::{ArtifactContent, GateTree};
use crate::engine::lineage::descendants;

#[derive(Clone, Debug)]
pub struct RevisionImpact {
    pub artifact_kind: String,
    pub material_id: String,
    pub directly_changed: bool,
    pub affected_populations: BTreeSet<String>,
    pub rename_only: bool,
    pub reason: String,
}

/// Channels whose compensation depends on a spillover material change.
pub fn spillover_channels(content: &ArtifactContent) -> Vec<String> {
    match content {
        ArtifactContent::Spillover(meta) => meta.rows.iter().map(|r| r.stain.clone()).collect(),
        _ => Vec::new(),
    }
}

fn gate_channels(polygon: &crate::engine::model::GatePolygon) -> Vec<String> {
    vec![polygon.x_channel.clone(), polygon.y_channel.clone()]
}

/// Compute the set of affected population (gate) ids when `new_content`
/// replaces the previous version of material `material_id`.
pub fn impact(
    artifact_kind: &str,
    material_id: &str,
    old_content: Option<&ArtifactContent>,
    new_content: &ArtifactContent,
    tree: Option<&GateTree>,
    all_polygons: &[&crate::engine::model::GatePolygon],
) -> RevisionImpact {
    let mut affected = BTreeSet::new();
    let mut rename_only = false;
    let mut reason = format!("{artifact_kind} material `{material_id}` revised");

    match (artifact_kind, new_content) {
        ("raw_events", ArtifactContent::RawEvents(fcs)) => {
            if fcs.re_read_scope.is_empty() {
                for p in all_polygons {
                    affected.insert(p.gate_id.clone());
                }
                reason.push_str("; full re-read: all populations affected");
            } else {
                let scope: BTreeSet<&str> = fcs.re_read_scope.iter().map(String::as_str).collect();
                for p in all_polygons {
                    let chans = gate_channels(p);
                    if chans.iter().any(|c| scope.contains(c.as_str())) {
                        affected.insert(p.gate_id.clone());
                    }
                }
                reason.push_str(&format!(
                    "; scoped re-read of channels [{}]: only gates on those channels affected",
                    fcs.re_read_scope.join(", ")
                ));
            }
        }
        ("spillover", content) => {
            let chans: BTreeSet<String> = spillover_channels(content).into_iter().collect();
            for p in all_polygons {
                if gate_channels(p).iter().any(|c| chans.contains(c)) {
                    affected.insert(p.gate_id.clone());
                }
            }
            reason.push_str("; single-stain coefficient change scoped to stained channel");
        }
        ("comp_matrix", _) => {
            for p in all_polygons {
                if p.space.contains("compensated") {
                    affected.insert(p.gate_id.clone());
                }
            }
            reason.push_str("; compensated-space gates affected");
        }
        ("transform", ArtifactContent::Transform(set)) => {
            let chans: BTreeSet<&str> = set.params.iter().map(|p| p.channel.as_str()).collect();
            for p in all_polygons {
                if gate_channels(p).iter().any(|c| chans.contains(c.as_str())) {
                    affected.insert(p.gate_id.clone());
                }
            }
            reason.push_str("; transformed gates on changed channels affected");
        }
        ("gate_polygon", ArtifactContent::GatePolygon(poly)) => {
            affected.insert(poly.gate_id.clone());
            if let Some(tree) = tree {
                for d in descendants(tree, &poly.gate_id) {
                    affected.insert(d);
                }
            }
            reason = format!("polygon `{}` revised; gate and descendants affected", poly.gate_id);
        }
        ("gate_tree", ArtifactContent::GateTree(new_tree)) => {
            if let Some(ArtifactContent::GateTree(old_tree)) = old_content {
                let same_structure =
                    crate::engine::lineage::structural_hash(old_tree)
                        == crate::engine::lineage::structural_hash(new_tree);
                if same_structure {
                    rename_only = true;
                    reason = "gate tree renamed only; stable gate_ids, edges and percentages unchanged";
                } else {
                    for n in &new_tree.nodes {
                        affected.insert(n.gate_id.clone());
                    }
                    reason.push_str("; lineage structure changed");
                }
            }
        }
        _ => {}
    }

    RevisionImpact {
        artifact_kind: artifact_kind.to_string(),
        material_id: material_id.to_string(),
        directly_changed: true,
        affected_populations: affected,
        rename_only,
        reason,
    }
}
