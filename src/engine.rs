//! 推导引擎：给定一版源材料引用，计算补偿、变换、门控与群体结果。
//! 引擎本身不写库、不改历史；所有结果带依赖版本引用，身份内容寻址。

use crate::comp::{compensate_row, CompAnalysis, CompCandidate, CompMatrix, CompVerdict};
use crate::event::AuditEvent;
use crate::gate::{GateDef, Membership, Point};
use crate::ids::derived_id;
use crate::lineage::{LineageDoc, LineageNode};
use crate::materials::{MaterialBody, MaterialEnvelope};
use crate::measure::RawMeasure;
use crate::population::{EventMembership, MembershipTag, PopulationResult};
use crate::transform::ChannelTransform;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone)]
pub struct ResolvedMaterials {
    pub channels: MaterialEnvelope,
    pub spillover: MaterialEnvelope,
    pub comp_matrix: MaterialEnvelope,
    pub transform: MaterialEnvelope,
    pub lineage: MaterialEnvelope,
    /// gate id -> 该门当前被引用的版本信封
    pub gates: BTreeMap<String, MaterialEnvelope>,
}

impl ResolvedMaterials {
    pub fn channel_doc(&self) -> &crate::materials::RawChannelDoc {
        match &self.channels.body {
            MaterialBody::Channels(c) => c,
            _ => unreachable!("channels envelope mismatch"),
        }
    }
    pub fn matrix(&self) -> &CompMatrix {
        match &self.comp_matrix.body {
            MaterialBody::CompMatrix(m) => m,
            _ => unreachable!(),
        }
    }
    pub fn transforms(&self) -> &[ChannelTransform] {
        match &self.transform.body {
            MaterialBody::Transform(t) => t,
            _ => unreachable!(),
        }
    }
    pub fn lineage_doc(&self) -> &LineageDoc {
        match &self.lineage.body {
            MaterialBody::Lineage(l) => l,
            _ => unreachable!(),
        }
    }
    pub fn gate_def(&self, gate_id: &str) -> Option<(&str, &GateDef)> {
        let env = self.gates.get(gate_id)?;
        match &env.body {
            MaterialBody::Gate(w) => Some((env.id.as_str(), &w.def)),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisWrapper {
    pub matrix_version_id: String,
    pub spillover_version_id: String,
    pub channels_version_id: String,
    pub analysis: CompAnalysis,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompensatedWrapper {
    pub analysis_derived_id: String,
    pub candidate_key: String,
    pub channel_order: Vec<String>,
    pub rows: Vec<crate::population::CompensatedEventRow>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PopWrapper {
    pub result: PopulationResult,
    pub deps: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Bundle {
    pub analysis_id: String,
    pub analysis_wrapper: AnalysisWrapper,
    pub compensated_id: Option<String>,
    pub compensated: Option<CompensatedWrapper>,
    pub pop_ids: BTreeMap<String, String>,
    pub pops: BTreeMap<String, PopWrapper>,
    pub verdict: CompVerdict,
}

/// 原始事件 -> 逐通道 RawMeasure（保留原始词法与诊断）。
pub fn raw_measures(mat: &ResolvedMaterials) -> Vec<Vec<RawMeasure>> {
    let doc = mat.channel_doc();
    let names: Vec<&str> = doc.fcs.channels.iter().map(|c| c.name.as_str()).collect();
    doc.fcs
        .events
        .iter()
        .map(|row| {
            row.iter()
                .enumerate()
                .map(|(i, raw)| RawMeasure::detector(raw.as_str(), names[i]))
                .collect()
        })
        .collect()
}

fn transform_for_basis<'a>(transforms: &'a [ChannelTransform], basis: &str) -> Option<&'a ChannelTransform> {
    transforms.iter().find(|t| t.output_basis == basis)
}

fn channel_in_basis(basis: &str) -> &str {
    basis.split("channel:").nth(1).unwrap_or(basis)
}

/// 运行整条推导链。selected_candidate=None 时：
/// - 可逆矩阵自动选 "inverse"；
/// - 近奇异/奇异矩阵阻断补偿，保留候选/判定，不产出补偿与群体数值。
pub fn evaluate(mat: &ResolvedMaterials, selected_candidate: Option<&str>) -> Bundle {
    let matrix = mat.matrix();
    let analysis = matrix.analyze();
    let wrapper = AnalysisWrapper {
        matrix_version_id: mat.comp_matrix.id.clone(),
        spillover_version_id: mat.spillover.id.clone(),
        channels_version_id: mat.channels.id.clone(),
        analysis: analysis.clone(),
    };
    let analysis_id = derived_id("comp_analysis", &wrapper);

    let candidate: Option<&CompCandidate> = match analysis.verdict {
        CompVerdict::Invertible => analysis.candidates.iter().find(|c| c.key == "inverse"),
        CompVerdict::NearSingular => {
            selected_candidate.and_then(|k| analysis.candidates.iter().find(|c| c.key == k))
        }
        CompVerdict::Singular => None,
    };

    let measures = raw_measures(mat);
    let mut pop_ids = BTreeMap::new();
    let mut pops = BTreeMap::new();

    let (compensated_id, compensated) = match candidate {
        Some(cand) => {
            let mut rows = Vec::with_capacity(measures.len());
            for (idx, row) in measures.iter().enumerate() {
                let mut diag: Vec<String> = Vec::new();
                let ordered: Vec<Option<f64>> = matrix
                    .channels
                    .iter()
                    .map(|ch| {
                        let pos = mat.channel_doc().fcs.channels.iter().position(|c| &c.name == ch);
                        match pos {
                            Some(p) => {
                                let m = &row[p];
                                diag.extend(m.diagnostics.iter().cloned());
                                m.value
                            }
                            None => {
                                diag.push("CHANNEL_NOT_PRESENT".into());
                                None
                            }
                        }
                    })
                    .collect();
                let values = compensate_row(&cand.inverse, &ordered);
                if values.is_none() {
                    diag.push("COMPENSATION_NOT_NUMERIC".into());
                }
                rows.push(crate::population::CompensatedEventRow {
                    event_index: idx as u32,
                    values,
                    basis: "channel-order:compensated".into(),
                    diagnostics: diag,
                });
            }
            let comp_wrap = CompensatedWrapper {
                analysis_derived_id: analysis_id.clone(),
                candidate_key: cand.key.clone(),
                channel_order: matrix.channels.clone(),
                rows,
            };
            let id = derived_id("compensated", &comp_wrap);
            build_pops(mat, &comp_wrap, &id, &mut pops, &mut pop_ids);
            (Some(id), Some(comp_wrap))
        }
        None => (None, None),
    };

    Bundle {
        analysis_id,
        analysis_wrapper: wrapper,
        compensated_id,
        compensated,
        pop_ids,
        pops,
        verdict: analysis.verdict,
    }
}

fn build_pops(
    mat: &ResolvedMaterials,
    comp: &CompensatedWrapper,
    comp_id: &str,
    pops: &mut BTreeMap<String, PopWrapper>,
    pop_ids: &mut BTreeMap<String, String>,
) {
    let lineage = mat.lineage_doc();
    let transforms = mat.transforms();
    let total = comp.rows.len() as u32;

    // 根群：补偿成功的事件（溢出/非数值不进入数值门控，诊断保留在行内）。
    let root_id = lineage
        .nodes
        .iter()
        .find_map(|n| matches!(n, LineageNode::Root { .. }).then(|| n.id().to_string()))
        .expect("lineage root");
    let root_memberships: Vec<EventMembership> = comp
        .rows
        .iter()
        .map(|r| EventMembership {
            event_index: r.event_index,
            membership: if r.values.is_some() {
                MembershipTag::Inside
            } else {
                MembershipTag::Invalid
            },
        })
        .collect();
    let root_result = PopulationResult::build(&root_id, root_memberships, total, "root:all-events".into());
    let root_wrap = PopWrapper {
        deps: vec![comp_id.to_string(), mat.channels.id.clone()],
        result: root_result,
    };
    let root_derived = derived_id("pop_root", &root_wrap);
    pops.insert(root_id.clone(), root_wrap);
    pop_ids.insert(root_id.clone(), root_derived);

    // 自上而下：父群分母在构建时钉住父群派生结果 id。
    let mut remaining: Vec<&String> = lineage.edges.iter().map(|e| &e.child).collect();
    let mut guard = 0;
    while !remaining.is_empty() {
        guard += 1;
        assert!(guard < 1000, "lineage cycle");
        let mut next = Vec::new();
        for child in remaining {
            let parents = lineage.parents(child);
            if !parents.iter().all(|p| pop_ids.contains_key(*p)) {
                next.push(child);
                continue;
            }
            let node = lineage.node(child).expect("node");
            let gate_ref = match node {
                LineageNode::Gate { gate_material_version_id, .. } => {
                    gate_material_version_id.clone()
                }
                LineageNode::Root { .. } => unreachable!(),
            };
            // 门当前定义（信封 id 可能已不同于节点钉住的版本——由 store 判过期）。
            let current_gate = lineage_gate_id(node);
            let (_, def) = mat.gate_def(current_gate).expect("gate def");

            let parent = parents[0].to_string();
            let parent_members: &PopulationResult = &pops.get(&parent).unwrap().result;
            let parent_derived = pop_ids.get(&parent).unwrap().clone();

            let parent_set: std::collections::HashSet<u32> =
                parent_members.member_events.iter().copied().collect();

            let xt = transform_for_basis(transforms, &def.x_basis);
            let yt = transform_for_basis(transforms, &def.y_basis);
            let xch = channel_in_basis(&def.x_basis).to_string();
            let ych = channel_in_basis(&def.y_basis).to_string();
            let xi = comp.channel_order.iter().position(|c| *c == xch);
            let yi = comp.channel_order.iter().position(|c| *c == ych);

            let mut memberships = Vec::new();
            for row in &comp.rows {
                let tag = if !parent_set.contains(&row.event_index) {
                    MembershipTag::Outside
                } else if row.values.is_none() {
                    MembershipTag::Invalid
                } else {
                    let vals = row.values.as_ref().unwrap();
                    let point = match (xi, yi, xt, yt) {
                        (Some(xi), Some(yi), Some(xt), Some(yt)) => {
                            let (x, dx) = xt.apply_with_diagnostics(vals[xi]);
                            let (y, dy) = yt.apply_with_diagnostics(vals[yi]);
                            let _ = (dx, dy);
                            Point { x, y }
                        }
                        _ => Point { x: f64::NAN, y: f64::NAN },
                    };
                    MembershipTag::from(def.membership(point))
                };
                memberships.push(EventMembership { event_index: row.event_index, membership: tag });
            }
            let result = PopulationResult::build(child, memberships, parent_members.numerator_count, parent_derived);
            let mut deps = vec![
                comp_id.to_string(),
                parent_derived,
                mat.transform.id.clone(),
                gate_ref,
            ];
            deps.sort();
            let wrap = PopWrapper { deps, result };
            let id = derived_id(&format!("pop_{child}"), &wrap);
            pops.insert(child.clone(), wrap);
            pop_ids.insert(child.clone(), id);
        }
        remaining = next;
    }
}

fn lineage_gate_id<'a>(node: &'a LineageNode) -> &'a str {
    match node {
        LineageNode::Gate { gate_material_version_id, .. } => {
            // 节点钉住的是版本 id，取其稳定门 id 部分（mat_gate_<id>_v.._hash）。
            // 信封按 gate id 存储；从版本 id 解析 gate id。
            gate_id_from_material(gate_material_version_id)
        }
        LineageNode::Root { .. } => "",
    }
}

/// 从 "mat_gate_<gateid>_v.." 还原 gate id（gate id 不含 "_v" 段）。
pub fn gate_id_from_material(version_id: &str) -> &str {
    let trimmed = version_id.strip_prefix("mat_gate_").unwrap_or(version_id);
    match trimmed.rfind("_v") {
        Some(pos) => &trimmed[..pos],
        None => trimmed,
    }
}

/// 事件快照中对派生结果的引用，供旧事件视图恢复。
pub fn snapshot_refs(bundle: &Bundle) -> serde_json::Value {
    serde_json::json!({
        "comp_analysis": bundle.analysis_id,
        "compensated": bundle.compensated_id,
        "populations": bundle.pop_ids,
        "verdict": format!("{:?}", bundle.verdict).to_ascii_lowercase(),
    })
}

/// 由事件的依赖快照恢复可见结论引用。
pub fn refs_from_snapshot(snapshot: &serde_json::Value) -> Option<serde_json::Value> {
    snapshot.get("derived").cloned()
}

#[allow(dead_code)]
pub fn membership_used(_e: &AuditEvent) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gate_id_roundtrip_from_material_version() {
        assert_eq!(gate_id_from_material("mat_gate_cd3_v02_ab12cd34ef56"), "cd3");
    }

    #[test]
    fn boundary_membership_is_not_inside() {
        let g = GateDef {
            x_basis: "x".into(),
            y_basis: "y".into(),
            shape: crate::gate::GateShape::Rectangle { x0: 0.0, x1: 1.0, y0: 0.0, y1: 1.0 },
            boundary_inclusive: false,
        };
        assert_eq!(g.membership(Point { x: 0.0, y: 0.5 }), Membership::Boundary);
    }
}
