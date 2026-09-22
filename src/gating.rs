//! 门控引擎：原始值 -> 补偿 -> 变换 -> 多边形成员 -> 群体百分比。
//!
//! 纯函数：给定钉住的输入版本集合，产出不可变门结果。
//! 规则 RULE.PCT-01：门的百分比分母必须钉住父群版本（parent_result_snapshot），
//! 绝不使用“当前最新”父群重新计算。
//! RULE.UNC-01：无法转换坐标的事件留在原处（dropped_unconvertible），不删除、不为画图丢弃。

use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::comp;
use crate::geom;
use crate::types::{
    CompMatrix, Dataset, DenominatorRef, Gate, GateResult, Lineage, RawValue, TransformParams,
};

pub const RULE_PCT: &str = "RULE.PCT-01";
pub const RULE_UNC: &str = "RULE.UNC-01";
pub const RULE_NS: &str = "RULE.NS-01";
pub const RULE_STALE: &str = "RULE.STALE-01";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompResolution {
    pub matrix_id: String,
    /// 采纳的候选 id（INV.direct / INV.ridge-small / INV.ridge-large）。
    pub candidate_id: String,
    /// 实际使用的逆矩阵。
    pub inverse: Vec<Vec<f64>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineInputs {
    pub dataset: Dataset,
    pub comp: Option<CompResolution>,
    pub matrix: Option<CompMatrix>,
    pub transform: TransformParams,
    pub gates: BTreeMap<String, Gate>,
    pub lineage: Lineage,
    /// 本组结果的内容快照标识，用于钉住分母引用。
    pub snapshot_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompPoint {
    pub event_id: String,
    pub ordinal: usize,
    /// 每个通道一个补偿后数值；无法补偿/转换的通道为 null 并带诊断。
    pub compensated: Vec<Option<f64>>,
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EngineOutput {
    /// 事件在补偿通道上的结果（用于补偿前后差异）。
    pub comp_points: Vec<CompPoint>,
    pub results: Vec<GateResult>,
}

fn numeric(v: &RawValue) -> Option<f64> {
    v.numeric.filter(|n| n.is_finite())
}

fn asinh(x: f64) -> f64 {
    ((x) + (x * x + 1.0).sqrt()).ln()
}

impl EngineInputs {
    fn channel_index(&self, id: &str) -> Option<usize> {
        self.dataset.channels.iter().position(|c| c.id == id)
    }

    fn cofactor(&self, channel: &str) -> Option<f64> {
        self.transform
            .per_channel
            .iter()
            .find(|t| t.channel == channel)
            .and_then(|t| t.cofactor)
    }

    /// 计算每个事件每通道的“派生数值”：
    /// spillover 通道先补偿；其余通道保持原始 ADC。
    /// 诊断原样保留；无法转换 -> None。
    pub fn derive(&self) -> Vec<Vec<Option<f64>>> {
        let nch = self.dataset.channels.len();
        let comp_channels: Vec<String> = self
            .matrix
            .as_ref()
            .map(|m| m.channels.clone())
            .unwrap_or_default();
        let comp_idx: Vec<Option<usize>> = comp_channels
            .iter()
            .map(|c| self.channel_index(c))
            .collect();
        let mut out = Vec::with_capacity(self.dataset.events.len());
        for ev in &self.dataset.events {
            // 先用原始数值填充。
            let mut row: Vec<Option<f64>> = ev.values.iter().map(numeric).collect();
            if let Some(res) = &self.comp {
                if comp_idx.iter().all(|i| i.is_some()) {
                    let raw: Vec<f64> = comp_idx
                        .iter()
                        .map(|i| numeric(&ev.values[i.unwrap()]).unwrap_or(0.0))
                        .collect();
                    let all_finite = comp_idx
                        .iter()
                        .all(|i| numeric(&ev.values[i.unwrap()]).is_some());
                    if all_finite {
                        let comped = comp::apply_inverse(&res.inverse, &raw);
                        for (k, idx_opt) in comp_idx.iter().enumerate() {
                            row[idx_opt.unwrap()] = Some(comped[k]);
                        }
                    } else {
                        for idx_opt in &comp_idx {
                            row[idx_opt.unwrap()] = None;
                        }
                    }
                }
            }
            let _ = nch;
            out.push(row);
        }
        out
    }

    fn coordinate(&self, derived: &[Option<f64>], channel: &str) -> Option<f64> {
        let idx = self.channel_index(channel)?;
        let v = derived[idx]?;
        match self.cofactor(channel) {
            Some(c) if c > 0.0 => Some(asinh(v / c)),
            _ => Some(v),
        }
    }

    pub fn evaluate(&self) -> EngineOutput {
        let derived = self.derive();
        let nch = self.dataset.channels.len();
        let mut comp_points = Vec::new();
        for (ev, row) in self.dataset.events.iter().zip(derived.iter()) {
            let diags: Vec<String> = ev
                .values
                .iter()
                .filter(|v| v.diagnostic != "none")
                .map(|v| v.diagnostic.clone())
                .collect();
            comp_points.push(CompPoint {
                event_id: ev.event_id.clone(),
                ordinal: ev.ordinal,
                compensated: row.clone(),
                diagnostics: diags,
            });
        }
        let _ = nch;

        // 拓扑顺序由 lineage.edges 顺序保证（父先于子）。
        let mut members: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut results: Vec<GateResult> = Vec::new();
        let all_ids: HashSet<String> =
            self.dataset.events.iter().map(|e| e.event_id.clone()).collect();
        for edge in &self.lineage.edges {
            let gate = match self.gates.get(&edge.child) {
                Some(g) => g,
                None => continue,
            };
            let parent_set: Option<&HashSet<String>> = None;
            let parent_members: Vec<String> = if edge.parent == "ROOT" {
                self.dataset.events.iter().map(|e| e.event_id.clone()).collect()
            } else {
                members.get(&edge.parent).cloned().unwrap_or_default()
            };
            let parent_hash: HashSet<String> = parent_members.iter().cloned().collect();
            let _ = parent_set;

            let denominator = parent_members.len();
            let denominator_ref = if edge.parent == "ROOT" {
                DenominatorRef {
                    kind: "dataset".into(),
                    dataset_id: self.dataset.dataset_id.clone(),
                    parent_gate_id: None,
                    parent_result_snapshot: None,
                    parent_event_count: all_ids.len(),
                }
            } else {
                DenominatorRef {
                    kind: "gate_result".into(),
                    dataset_id: self.dataset.dataset_id.clone(),
                    parent_gate_id: Some(edge.parent.clone()),
                    parent_result_snapshot: Some(self.snapshot_id.clone()),
                    parent_event_count: denominator,
                }
            };

            let mut included = Vec::new();
            let mut excluded = Vec::new();
            let mut dropped = Vec::new();
            let mut gate_member_ids = Vec::new();
            for ev in &self.dataset.events {
                if !parent_hash.contains(&ev.event_id) {
                    continue;
                }
                let didx = ev.ordinal;
                let x = self.coordinate(&derived[didx], &gate.x_channel);
                let y = self.coordinate(&derived[didx], &gate.y_channel);
                let (x, y) = match (x, y) {
                    (Some(x), Some(y)) => (x, y),
                    _ => {
                        dropped.push(ev.event_id.clone());
                        continue;
                    }
                };
                match geom::classify(&gate.vertices, geom::Point::new(x, y)) {
                    geom::Boundary::Inside => gate_member_ids.push(ev.event_id.clone()),
                    geom::Boundary::IncludedEdge => {
                        included.push(ev.event_id.clone());
                        gate_member_ids.push(ev.event_id.clone());
                    }
                    geom::Boundary::ExcludedEdge => excluded.push(ev.event_id.clone()),
                    geom::Boundary::Outside => {}
                }
            }
            gate_member_ids.sort();
            included.sort();
            excluded.sort();
            dropped.sort();
            let numerator = gate_member_ids.len();
            let percent = if denominator > 0 {
                100.0 * numerator as f64 / denominator as f64
            } else {
                0.0
            };
            let result = GateResult {
                gate_id: gate.gate_id.clone(),
                label: gate.label.clone(),
                parent: edge.parent.clone(),
                population: edge.population.clone(),
                member_event_ids: gate_member_ids.clone(),
                included_boundary: included,
                excluded_boundary: excluded,
                dropped_unconvertible: dropped,
                numerator,
                denominator,
                denominator_ref,
                percent,
                percent_unit: "percent_of_pinned_parent".into(),
            };
            members.insert(gate.gate_id.clone(), gate_member_ids);
            results.push(result);
        }
        EngineOutput { comp_points, results }
    }
}
