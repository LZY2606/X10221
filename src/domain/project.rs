//! 事件投影：从事件流重建某分支在某时刻的可见结论。
//!
//! 关键不变量：
//! - 群百分比分母钉住父群版本（事件中保存 DenominatorRef）；
//! - 上游门修订后，下游结果只加 [`StaleMark`]，旧 PopulationResult 原样保留；
//! - 近奇异补偿矩阵的全部合法候选保留到裁定事件发生前，不默默选择。

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::domain::events::{EventKind, EventRecord, Judgement};
use crate::domain::gate::PolygonGate;
use crate::domain::model::{
    Adjudication, ChannelSpec, CompensationMatrix, DenominatorRef, GateVersion, PopulationResult,
    SpilloverMeta, StaleMark, TransformParam,
};

pub const ROOT_GATE: &str = "gate#root";
pub const MAIN_BRANCH: &str = "main";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct BlobInfo {
    pub blob_id: String,
    pub kind: String,
    pub filename: String,
    pub byte_len: usize,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RawEventRow {
    pub event_id: String,
    /// 原始通道值（f64 承载 f32；NaN/Inf 允许出现）。
    pub values: BTreeMap<String, f64>,
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct BranchState {
    pub branch: String,
    pub head_seq: i64,
    pub head_event_id: String,
    pub head_hash: String,
    pub blobs: BTreeMap<String, BlobInfo>,
    pub channels: BTreeMap<String, ChannelSpec>,
    pub channel_order: Vec<String>,
    pub raw_events: BTreeMap<String, RawEventRow>,
    pub event_order: Vec<String>,
    pub spillovers: BTreeMap<String, SpilloverMeta>,
    pub spillover_order: Vec<String>,
    pub matrices: BTreeMap<String, CompensationMatrix>,
    pub matrix_order: Vec<String>,
    pub transforms: BTreeMap<String, TransformParam>,
    pub transform_order: Vec<String>,
    pub gates: BTreeMap<String, Vec<GateVersion>>,
    pub gate_order: Vec<String>,
    /// 每个门当前生效版本 rev（可能为某分支上的改名/修订）。
    pub gate_head_rev: BTreeMap<String, String>,
    pub populations: Vec<PopulationResult>,
    pub stale: Vec<StaleMark>,
    pub adjudications: Vec<Adjudication>,
    /// 被撤销裁定的 id（撤销事件产生）。
    pub undone: BTreeSet<String>,
    /// 父分支（草案分叉点）。
    pub forked_from: Option<(String, i64)>,
    /// 所有事件（该分支线性历史）。
    pub events: Vec<EventRecord>,
}

impl BranchState {
    pub fn new(branch: &str) -> Self {
        let mut s = BranchState::default();
        s.branch = branch.to_string();
        s
    }

    pub fn gate(&self, id: &str) -> Option<&PolygonGate> {
        let rev = self.gate_head_rev.get(id)?;
        self.gates
            .get(id)
            .and_then(|vs| vs.iter().find(|v| &v.rev == rev))
            .map(|v| &v.gate)
    }

    pub fn gate_version(&self, id: &str, rev: &str) -> Option<&GateVersion> {
        self.gates.get(id).and_then(|vs| vs.iter().find(|v| v.rev == rev))
    }

    /// 门的最新版本（追加顺序末尾）。
    pub fn latest_gate_version(&self, id: &str) -> Option<&GateVersion> {
        self.gates.get(id).and_then(|vs| vs.last())
    }

    /// 某门在当前分支最新门版本下是否已过期（被标记且无更新结果）。
    pub fn gate_stale_marks(&self, gate_id: &str) -> Vec<&StaleMark> {
        let latest = self.latest_gate_version(gate_id).map(|v| v.rev.as_str());
        self.stale
            .iter()
            .filter(|m| m.gate_id == gate_id && Some(m.stale_gate_rev.as_str()) == latest)
            .collect()
    }

    /// 门在当前分支最新门版本下的最新群结果。
    pub fn population_for_latest(&self, gate_id: &str) -> Option<&PopulationResult> {
        let rev = self.latest_gate_version(gate_id)?.rev.as_str();
        self.populations.iter().rev().find(|p| p.gate_id == gate_id && p.gate_rev == rev)
    }

    /// 当前可用补偿矩阵：必须已有显式选定候选（近奇异未裁定时不可用）。
    pub fn active_matrix(&self) -> Option<&CompensationMatrix> {
        for id in self.matrix_order.iter().rev() {
            if let Some(m) = self.matrices.get(id) {
                if m.status == "rejected" {
                    continue;
                }
                if m.chosen_candidate.is_some() && m.comp_matrix().is_some() {
                    return Some(m);
                }
            }
        }
        None
    }

    /// 最新矩阵版本（不论是否可用于补偿），供审阅页展示候选与诊断。
    pub fn latest_matrix(&self) -> Option<&CompensationMatrix> {
        self.matrix_order.iter().rev().find_map(|id| self.matrices.get(id))
    }

    /// 该门在谱系上的所有祖先（含自身），按从根到门顺序。
    pub fn ancestry(&self, gate_id: &str) -> Vec<String> {
        let mut chain = Vec::new();
        let mut cur = Some(gate_id.to_string());
        let mut guard = 0;
        while let Some(id) = cur {
            chain.push(id.clone());
            if id == ROOT_GATE {
                break;
            }
            cur = self.gate(&id).and_then(|g| g.parent_gate_id.clone());
            guard += 1;
            if guard > 64 {
                break;
            }
        }
        chain.reverse();
        chain
    }

    /// 受某上游门修订影响的后代门（失效范围）。
    pub fn descendants_of(&self, gate_id: &str) -> Vec<String> {
        let mut out = Vec::new();
        for id in &self.gate_order {
            let chain = self.ancestry(id);
            if chain.contains(&gate_id.to_string()) && id != gate_id {
                out.push(id.clone());
            }
        }
        out
    }
}

/// 把一个事件应用到分支投影上。
pub fn apply_event(state: &mut BranchState, ev: &EventRecord) {
    state.head_seq = ev.seq;
    state.head_event_id = ev.event_id.clone();
    state.head_hash = ev.hash.clone();
    state.events.push(ev.clone());

    let kind = match EventKind::parse(&ev.kind) {
        Some(k) => k,
        None => return,
    };
    let p = &ev.payload;
    match kind {
        EventKind::ImportCommitted => fold_import(state, ev, p),
        EventKind::CompensationRevised => fold_comp_revised(state, p),
        EventKind::CompensationAdjudicated => fold_comp_adjudication(state, p),
        EventKind::TransformRevised => fold_transform(state, p),
        EventKind::GateRevised => fold_gate_revised(state, p),
        EventKind::BoundaryAdjudicated => {}
        // 派生结果直接从载荷回放（保证历史结论不被新计算覆盖）
        EventKind::PopulationsDerived => fold_populations(state, p),
        EventKind::MarkedStale => fold_stale(state, p),
        EventKind::DraftForked => {
            if let (Some(b), Some(seq)) =
                (p.get("forked_branch").and_then(|v| v.as_str()), p.get("at_seq").and_then(|v| v.as_i64()))
            {
                state.forked_from = Some((b.to_string(), seq));
            }
        }
        EventKind::DecisionUndone => {
            if let Some(id) = p.get("adjudication_id").and_then(|v| v.as_str()) {
                state.undone.insert(id.to_string());
            }
        }
    }
}

fn fold_import(state: &mut BranchState, ev: &EventRecord, p: &Value) {
    if let Some(blobs) = p.get("blobs").and_then(|v| v.as_array()) {
        for b in blobs {
            if let Ok(info) = serde_json::from_value::<BlobInfo>(b.clone()) {
                state.blobs.insert(info.blob_id.clone(), info);
            }
        }
    }
    if let Some(channels) = p.get("channels").and_then(|v| v.as_array()) {
        state.channel_order.clear();
        state.channels.clear();
        for c in channels {
            if let Ok(ch) = serde_json::from_value::<ChannelSpec>(c.clone()) {
                state.channel_order.push(ch.name.clone());
                state.channels.insert(ch.name.clone(), ch);
            }
        }
    }
    if let Some(rows) = p.get("raw_events").and_then(|v| v.as_array()) {
        state.event_order.clear();
        state.raw_events.clear();
        for r in rows {
            if let Ok(row) = serde_json::from_value::<RawEventRow>(r.clone()) {
                state.event_order.push(row.event_id.clone());
                state.raw_events.insert(row.event_id.clone(), row);
            }
        }
    }
    if let Some(arr) = p.get("spillovers").and_then(|v| v.as_array()) {
        for v in arr {
            if let Ok(sm) = serde_json::from_value::<SpilloverMeta>(v.clone()) {
                if !state.spillover_order.contains(&sm.stable_id) {
                    state.spillover_order.push(sm.stable_id.clone());
                }
                state.spillovers.insert(sm.stable_id.clone(), sm);
            }
        }
    }
    if let Some(arr) = p.get("matrices").and_then(|v| v.as_array()) {
        for v in arr {
            if let Ok(m) = serde_json::from_value::<CompensationMatrix>(v.clone()) {
                if !state.matrix_order.contains(&m.stable_id) {
                    state.matrix_order.push(m.stable_id.clone());
                }
                state.matrices.insert(m.stable_id.clone(), m);
            }
        }
    }
    if let Some(arr) = p.get("transforms").and_then(|v| v.as_array()) {
        for v in arr {
            if let Ok(t) = serde_json::from_value::<TransformParam>(v.clone()) {
                if !state.transform_order.contains(&t.stable_id) {
                    state.transform_order.push(t.stable_id.clone());
                }
                state.transforms.insert(t.stable_id.clone(), t);
            }
        }
    }
    if let Some(arr) = p.get("gates").and_then(|v| v.as_array()) {
        for v in arr {
            if let Ok(gv) = serde_json::from_value::<GateVersion>(v.clone()) {
                let id = gv.gate.stable_id.clone();
                if !state.gate_order.contains(&id) {
                    state.gate_order.push(id.clone());
                }
                state.gate_head_rev.insert(id.clone(), gv.rev.clone());
                state.gates.entry(id).or_default().push(gv);
            }
        }
    }
    if let Some(arr) = p.get("populations").and_then(|v| v.as_array()) {
        for v in arr {
            if let Ok(r) = serde_json::from_value::<PopulationResult>(v.clone()) {
                state.populations.push(r);
            }
        }
    }
    if let Some(arr) = p.get("adjudications").and_then(|v| v.as_array()) {
        for v in arr {
            if let Ok(a) = serde_json::from_value::<Adjudication>(v.clone()) {
                state.adjudications.push(a);
            }
        }
    }
    let _ = ev;
}

fn fold_comp_revised(state: &mut BranchState, p: &Value) {
    if let Some(v) = p.get("spillover") {
        if let Ok(sm) = serde_json::from_value::<SpilloverMeta>(v.clone()) {
            if !state.spillover_order.contains(&sm.stable_id) {
                state.spillover_order.push(sm.stable_id.clone());
            }
            state.spillovers.insert(sm.stable_id.clone(), sm);
        }
    }
    if let Some(v) = p.get("matrix") {
        if let Ok(m) = serde_json::from_value::<CompensationMatrix>(v.clone()) {
            if !state.matrix_order.contains(&m.stable_id) {
                state.matrix_order.push(m.stable_id.clone());
            }
            state.matrices.insert(m.stable_id.clone(), m);
        }
    }
    fold_stale(state, p);
}

fn fold_comp_adjudication(state: &mut BranchState, p: &Value) {
    let matrix_id = p.get("matrix_id").and_then(|v| v.as_str()).unwrap_or("");
    let decision = p.get("decision").and_then(|v| v.as_str()).unwrap_or("");
    let candidate = p.get("candidate").and_then(|v| v.as_str());
    if let Some(m) = state.matrices.get_mut(matrix_id) {
        m.chosen_candidate = candidate.map(|s| s.to_string());
        if decision == "reject" {
            m.chosen_candidate = None;
            m.status = "rejected".into();
        } else if decision == "accept" {
            m.status = format!("adjudicated:{candidate}", candidate = candidate.unwrap_or("?"));
        }
    }
    if let Ok(a) = serde_json::from_value::<Adjudication>(
        serde_json::json!({
            "id": p.get("adjudication_id").cloned().unwrap_or(Value::Null),
            "kind": "compensation",
            "decision": decision,
            "target_id": matrix_id,
            "base_rev": p.get("base_rev").cloned().unwrap_or(Value::Null),
            "note": p.get("note").cloned().unwrap_or(Value::Null),
        }),
    ) {
        state.adjudications.push(a);
    }
}

fn fold_transform(state: &mut BranchState, p: &Value) {
    if let Some(v) = p.get("transform") {
        if let Ok(t) = serde_json::from_value::<TransformParam>(v.clone()) {
            if !state.transform_order.contains(&t.stable_id) {
                state.transform_order.push(t.stable_id.clone());
            }
            state.transforms.insert(t.stable_id.clone(), t);
        }
    }
}

fn fold_gate_revised(state: &mut BranchState, p: &Value) {
    if let Some(v) = p.get("gate") {
        if let Ok(gv) = serde_json::from_value::<GateVersion>(v.clone()) {
            let id = gv.gate.stable_id.clone();
            if !state.gate_order.contains(&id) {
                state.gate_order.push(id.clone());
            }
            state.gate_head_rev.insert(id.clone(), gv.rev.clone());
            state.gates.entry(id).or_default().push(gv);
        }
    }
}

fn fold_populations(state: &mut BranchState, p: &Value) {
    if let Some(arr) = p.get("populations").and_then(|v| v.as_array()) {
        for v in arr {
            if let Ok(r) = serde_json::from_value::<PopulationResult>(v.clone()) {
                state.populations.push(r);
            }
        }
    }
}

fn fold_stale(state: &mut BranchState, p: &Value) {
    if let Some(arr) = p.get("stale").and_then(|v| v.as_array()) {
        for v in arr {
            if let Ok(m) = serde_json::from_value::<StaleMark>(v.clone()) {
                if !state.stale.iter().any(|x| x == &m) {
                    state.stale.push(m);
                }
            }
        }
    }
}

/// 从一组事件重放为分支状态（事件必须已是该分支线性序列）。
pub fn replay(branch: &str, events: &[EventRecord]) -> BranchState {
    let mut s = BranchState::new(branch);
    for ev in events {
        apply_event(&mut s, ev);
    }
    s
}

/// 标准机器判定：近奇异不可默默求逆。
pub fn judgement_no_silent_inverse(matrix_id: &str, status: &str) -> Judgement {
    Judgement {
        rule_id: "rule.no-silent-inverse".into(),
        status: match status {
            "needs_decision" => "blocked_pending_adjudication".into(),
            "rejected" => "rejected".into(),
            _ => "satisfied".into(),
        },
        message: "近奇异补偿矩阵不能被默默求逆：保留全部合法候选，等待显式裁定".into(),
        affected: vec![matrix_id.to_string()],
    }
}

/// 标准机器判定：百分比分母钉住父群版本。
pub fn judgement_denominator_pinned(pop: &PopulationResult) -> Judgement {
    Judgement {
        rule_id: "rule.denominator-pinned-parent".into(),
        status: "satisfied".into(),
        message: format!(
            "群 {} 的分母钉住父群 {}@{}（{} 事件，成员集合 {}）",
            pop.gate_id, pop.denominator.parent_gate_id, pop.denominator.parent_gate_rev,
            pop.denominator.parent_count, pop.denominator.parent_membership_rev
        ),
        affected: vec![pop.gate_id.clone()],
    }
}

/// 标准机器判定：上游修订后下游标过期不覆盖。
pub fn judgement_stale_not_overwrite(marks: &[StaleMark]) -> Judgement {
    Judgement {
        rule_id: "rule.stale-not-overwrite".into(),
        status: "satisfied".into(),
        message: "上游门修订后，下游结果标记过期并保留旧值，不以新计算覆盖历史".into(),
        affected: marks.iter().map(|m| m.gate_id.clone()).collect(),
    }
}
