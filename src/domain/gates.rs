//! 门多边形、父子谱系与“改名后引用解析”的多候选。
//!
//! 门身份（gate_id）是内容寻址的稳定身份；谱系边的父引用先以
//! `parent_ref`（门身份或导入时看到的显示名）保存。当一次“门嵌套
//! 改名”导致名字无法唯一解析时，解析层保留 *全部合法候选*，不做
//! 静默选择。

use serde::{Deserialize, Serialize};

use super::geometry::Polygon;

pub const HIERARCHY_RULE: &str = "gate.parent_reference.resolve_by_id_then_name.v1";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PolygonGate {
    pub gate_id: String,
    pub name: String,
    /// x/y 通道与坐标基准（原始/补偿后/变换后）。
    pub x_channel: String,
    pub y_channel: String,
    pub basis: String,
    pub polygon: Polygon,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GateHierarchy {
    pub hierarchy_id: String,
    /// 有序边：节点名在版本内保持稳定顺序（导出顺序即此顺序）。
    pub nodes: Vec<GateNode>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GateNode {
    /// 该节点对应的门（可能是 ID 或名字，导入时原样保留）。
    pub gate_ref: String,
    /// 父引用：优先 gate_id，也可能是显示名。
    pub parent_ref: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ResolvedHierarchy {
    pub edges: Vec<ResolvedEdge>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ResolvedEdge {
    pub child_id: String,
    /// 父唯一时为一个候选；改名产生歧义时为全部合法候选。
    pub parent_candidates: Vec<ParentCandidate>,
    pub status: ResolutionStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ParentCandidate {
    pub gate_id: String,
    pub name: String,
    pub why: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ResolutionStatus {
    Unique,
    Ambiguous,
    Unresolved,
}

/// 用一组门解析谱系。返回每个子节点的父候选集合。
pub fn resolve_hierarchy(h: &GateHierarchy, gates: &[PolygonGate]) -> ResolvedHierarchy {
    let mut edges = Vec::new();
    for node in &h.nodes {
        let child_id = resolve_one(&node.gate_ref, gates)
            .into_iter()
            .map(|g| g.gate_id)
            .next()
            .unwrap_or_else(|| node.gate_ref.clone());

        let parent = match &node.parent_ref {
            None => {
                edges.push(ResolvedEdge {
                    child_id,
                    parent_candidates: vec![],
                    status: ResolutionStatus::Unique,
                });
                continue;
            }
            Some(p) => p,
        };

        // 1) ID 精确命中 => 唯一。
        if let Some(g) = gates.iter().find(|g| g.gate_id == *parent) {
            edges.push(ResolvedEdge {
                child_id,
                parent_candidates: vec![ParentCandidate {
                    gate_id: g.gate_id.clone(),
                    name: g.name.clone(),
                    why: "id".to_string(),
                }],
                status: ResolutionStatus::Unique,
            });
            continue;
        }

        // 2) 名字匹配：唯一 => 唯一；多个 => 全部合法候选（歧义）。
        let matches: Vec<&PolygonGate> = gates.iter().filter(|g| g.name == *parent).collect();
        match matches.len() {
            0 => edges.push(ResolvedEdge {
                child_id,
                parent_candidates: vec![],
                status: ResolutionStatus::Unresolved,
            }),
            1 => edges.push(ResolvedEdge {
                child_id,
                parent_candidates: vec![ParentCandidate {
                    gate_id: matches[0].gate_id.clone(),
                    name: matches[0].name.clone(),
                    why: "unique-name".to_string(),
                }],
                status: ResolutionStatus::Unique,
            }),
            _ => edges.push(ResolvedEdge {
                child_id,
                parent_candidates: matches
                    .iter()
                    .map(|g| ParentCandidate {
                        gate_id: g.gate_id.clone(),
                        name: g.name.clone(),
                        why: "duplicate-name-after-rename".to_string(),
                    })
                    .collect(),
                status: ResolutionStatus::Ambiguous,
            }),
        }
    }
    ResolvedHierarchy { edges }
}

fn resolve_one(reference: &str, gates: &[PolygonGate]) -> Vec<PolygonGate> {
    if let Some(g) = gates.iter().find(|g| g.gate_id == reference) {
        return vec![g.clone()];
    }
    gates
        .iter()
        .filter(|g| g.name == reference)
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::geometry::Point;

    fn gate(id: &str, name: &str) -> PolygonGate {
        PolygonGate {
            gate_id: id.to_string(),
            name: name.to_string(),
            x_channel: "x".to_string(),
            y_channel: "y".to_string(),
            basis: "transformed".to_string(),
            polygon: Polygon {
                basis: "transformed".to_string(),
                vertices: vec![
                    Point { x: 0.0, y: 0.0 },
                    Point { x: 1.0, y: 0.0 },
                    Point { x: 0.0, y: 1.0 },
                ],
            },
        }
    }

    #[test]
    fn id_resolves_unique() {
        let gates = vec![gate("g_a", "Lymphocytes"), gate("g_b", "Monocytes")];
        let h = GateHierarchy {
            hierarchy_id: "h".to_string(),
            nodes: vec![
                GateNode { gate_ref: "g_a".to_string(), parent_ref: None },
                GateNode {
                    gate_ref: "g_b".to_string(),
                    parent_ref: Some("g_a".to_string()),
                },
            ],
        };
        let r = resolve_hierarchy(&h, &gates);
        assert_eq!(r.edges[1].status, ResolutionStatus::Unique);
        assert_eq!(r.edges[1].parent_candidates[0].gate_id, "g_a");
    }

    #[test]
    fn rename_creates_two_legal_candidates() {
        // 两个门被改成同名（门嵌套改名场景），子节点的父名产生歧义。
        let gates = vec![gate("g_a", "CD4"), gate("g_b2", "CD4")];
        let h = GateHierarchy {
            hierarchy_id: "h".to_string(),
            nodes: vec![GateNode {
                gate_ref: "g_child".to_string(),
                parent_ref: Some("CD4".to_string()),
            }],
        };
        let r = resolve_hierarchy(&h, &gates);
        assert_eq!(r.edges[0].status, ResolutionStatus::Ambiguous);
        assert_eq!(r.edges[0].parent_candidates.len(), 2);
    }
}
