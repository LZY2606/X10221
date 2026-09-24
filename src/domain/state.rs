//! 事件重放得到的聚合状态，以及派生结果（补偿 / 变换 / 细胞群）计算。
//!
//! 设计要点：
//! * 来源与解释分开；事件不可变，任何状态都能重放恢复。
//! * 近奇异矩阵：保留候选与判定状态，未经人工裁定不产出补偿结论。
//! * 群体百分比的分母钉住父群结果身份（含其依赖版本）。
//! * 上游修订只让受影响结果过期；旧结果不被覆盖。

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::domain::math::{asinh_transform, lu_decompose, point_in_polygon, pseudoinverse, LuReport};
use crate::domain::model::{
    CompensationMatrix, Event, GateLink, GatePolygon, RawEvents, SpilloverMeta, TransformParams,
};

/// 近奇异时保留的合法候选（不自动采纳）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InverseCandidate {
    pub id: String,
    pub kind: String,
    pub label: String,
    /// 候选逆矩阵（行优先）。
    pub inverse: Vec<Vec<f64>>,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MatrixStatus {
    pub report: Option<LuReport>,
    pub near_singular: bool,
    pub detection_event_id: Option<String>,
    pub candidates: Vec<InverseCandidate>,
    /// 人工接受的候选 id（来自 Adjudicate 事件）。
    pub accepted_candidate_id: Option<String>,
    pub accepted_reason_code: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AdjudicationRec {
    pub event_id: String,
    pub target: String,
    pub accepted: bool,
    pub reason_code: String,
    pub note: String,
    pub timestamp: String,
}

#[derive(Debug, Clone, Default)]
pub struct State {
    pub branch: String,
    pub head_seq: i64,
    /// 槽位版本：source / matrix / transform / gate:<id> / link:<id> / boundary:<gate>
    pub slot_versions: BTreeMap<String, i64>,
    pub raw: Option<RawEvents>,
    pub raw_history: Vec<(i64, String)>,
    pub spill: Option<SpilloverMeta>,
    pub matrix: Option<CompensationMatrix>,
    pub matrix_status: Option<MatrixStatus>,
    pub transform: Option<TransformParams>,
    pub gates: BTreeMap<String, GatePolygon>,
    pub links: BTreeMap<String, GateLink>,
    pub boundary: BTreeMap<(String, usize), bool>,
    pub boundary_seq: BTreeMap<String, i64>,
    pub adjudications: Vec<AdjudicationRec>,
    /// Undo 的事件 id（重放时这些事件视为不存在）。
    pub undone: BTreeSet<String>,
    /// 每个槽位最后一次修改的事件 id（溯源）。
    pub slot_events: BTreeMap<String, String>,
    pub events: Vec<Event>,
}

impl State {
    pub fn empty(branch: &str) -> Self {
        State { branch: branch.to_string(), ..Default::default() }
    }

    /// 从事件流重放。事件必须按 seq 升序；branch_open 之外都属于同一分支。
    pub fn replay(branch: &str, events: &[Event]) -> State {
        let mut st = State::empty(branch);
        for ev in events {
            apply_event(&mut st, ev.clone());
        }
        st
    }

    pub fn version(&self) -> i64 {
        self.head_seq
    }

    pub fn cofactor_for(&self, channel: &str) -> Option<f64> {
        let t = self.transform.as_ref()?;
        t.per_channel_cofactor
            .iter()
            .find(|(c, _)| c == channel)
            .map(|(_, v)| *v)
            .or(Some(t.default_cofactor))
    }
}

pub fn apply_event(st: &mut State, ev: Event) {
    if matches!(ev, Event::Undo { .. }) {
        if let Event::Undo { undone_event_id, .. } = &ev {
            st.undone.insert(undone_event_id.clone());
        }
        st.head_seq = ev.seq();
        st.events.push(ev);
        return;
    }
    if st.undone.contains(ev.id()) {
        st.head_seq = ev.seq();
        return;
    }
    match &ev {
        Event::Import { fcs_json, spill_json, seq, .. } => {
            let _affected = match &ev {
                Event::Import { affected_channels, .. } => affected_channels.clone(),
                _ => vec![],
            };
            let mut raw: RawEvents = serde_json::from_str(fcs_json).expect("fcs json");
            raw.id = format!("fcs:{}", &raw.source_sha[..12]);
            st.raw = Some(raw);
            st.spill = spill_json
                .as_ref()
                .map(|j| serde_json::from_str::<SpilloverMeta>(j).expect("spill json"));
            bump(st, "source", *seq, ev.id());
            st.raw_history.push((*seq, ev.id().to_string()));
        }
        Event::CompensationSet { matrix_json, seq, .. } => {
            let m: CompensationMatrix = serde_json::from_str(matrix_json).expect("matrix json");
            st.matrix_status = Some(analyze_matrix(&m));
            st.matrix = Some(m);
            bump(st, "matrix", *seq, ev.id());
        }
        Event::NearSingularDetected { matrix_id, candidates_json, affected_objects: _, seq, .. } => {
            if let Some(status) = st.matrix_status.as_mut() {
                status.detection_event_id = Some(ev.id().to_string());
                status.candidates = serde_json::from_str(candidates_json).expect("candidates json");
                let _ = matrix_id;
            }
        }
        Event::TransformSet { transform_json, seq, .. } => {
            let t: TransformParams = serde_json::from_str(transform_json).expect("transform json");
            st.transform = Some(t);
            bump(st, "transform", *seq, ev.id());
        }
        Event::GateUpsert { gate_json, link_json, seq, .. } => {
            let g: GatePolygon = serde_json::from_str(gate_json).expect("gate json");
            let l: GateLink = serde_json::from_str(link_json).expect("link json");
            bump(st, &format!("gate:{}", g.id), *seq, ev.id());
            bump(st, &format!("link:{}", l.id), *seq, ev.id());
            st.gates.insert(g.id.clone(), g);
            st.links.insert(l.id.clone(), l);
        }
        Event::GateRename { link_json, seq, .. } => {
            let l: GateLink = serde_json::from_str(link_json).expect("link json");
            bump(st, &format!("link:{}", l.id), *seq, ev.id());
            st.links.insert(l.id.clone(), l);
        }
        Event::BoundaryDecision { gate_id, decisions_json, seq, .. } => {
            let d: BTreeMap<usize, bool> = serde_json::from_str(decisions_json).expect("decisions");
            for (k, v) in d {
                st.boundary.insert((gate_id.clone(), k), v);
            }
            bump(st, &format!("boundary:{gate_id}"), *seq, ev.id());
        }
        Event::Adjudicate { target, accepted, reason_code, note, timestamp, .. } => {
            if accepted
                && reason_code.starts_with("accept_candidate:")
                && let Some(status) = st.matrix_status.as_mut()
            {
                let cid = reason_code.trim_start_matches("accept_candidate:").to_string();
                status.accepted_candidate_id = Some(cid);
                status.accepted_reason_code = Some(reason_code.clone());
            }
            st.adjudications.push(AdjudicationRec {
                event_id: ev.id().to_string(),
                target: target.clone(),
                accepted: *accepted,
                reason_code: reason_code.clone(),
                note: note.clone(),
                timestamp: timestamp.clone(),
            });
        }
        Event::BranchOpen { .. } => {}
        Event::Undo { .. } => unreachable!(),
    }
    st.head_seq = ev.seq();
    st.events.push(ev);
}

fn bump(st: &mut State, slot: &str, seq: i64, event_id: &str) {
    st.slot_versions.insert(slot.to_string(), seq);
    st.slot_events.insert(slot.to_string(), event_id.to_string());
}

pub fn analyze_matrix(m: &CompensationMatrix) -> MatrixStatus {
    let report = lu_decompose(&m.spill).map(|(_, _, r)| r);
    let near = report.as_ref().map(|r| r.rcond < m.near_singular_threshold).unwrap_or(true);
    let mut candidates = Vec::new();
    if near {
        candidates.push(InverseCandidate {
            id: format!("{}:cand:pseudoinverse", m.id),
            kind: "pseudoinverse".into(),
            label: "SVD/岭回归伪逆（rcond 低于阈值时的显式候选，不自动采纳）".into(),
            inverse: pseudoinverse(&m.spill),
            note: "最小二乘意义下的逆；近奇异输入下结果对噪声敏感".into(),
        });
        candidates.push(InverseCandidate {
            id: format!("{}:cand:identity", m.id),
            kind: "identity".into(),
            label: "保持未补偿（单位矩阵）".into(),
            inverse: identity(m.spill.len()),
            note: "不做泄漏扣除，等待单染对照重采".into(),
        });
    }
    MatrixStatus {
        report,
        near_singular: near,
        detection_event_id: None,
        candidates,
        accepted_candidate_id: None,
        accepted_reason_code: None,
    }
}

fn identity(n: usize) -> Vec<Vec<f64>> {
    (0..n).map(|i| (0..n).map(|j| if i == j { 1.0 } else { 0.0 }).collect()).collect()
}
