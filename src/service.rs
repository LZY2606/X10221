//! 领域服务：补偿候选生成、群体计算、依赖失效传播、复核裁定。
//!
//! 规则（被端到端与单元测试钉住）：
//! 1. 近奇异补偿矩阵不能被默默求逆：严格奇异 -> 零候选 + 诊断；近奇异 -> 保留多个 ridge 候选，等人工裁定。
//! 2. 门的百分比分母必须钉住父群版本（PopulationResult.denominator）。
//! 3. 修订上游门后下游结果标记过期而不覆盖：旧 PopulationResult 保留，仅追加 StaleMarks。

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{bail, Result};
use sha2::{Digest, Sha256};

use crate::canonical::hex_encode;
use crate::domain::matrix::{classify, MatrixDiag};
use crate::domain::{
    CompensationCandidate, CompensationProposal, DenominatorRef, GateVersion, Point,
    PopulationResult, SpilloverSpec, TransformVersion,
};
use crate::events::{
    event_identity, Event, EventPayload, ReviewAction, StaleMarkPayload, SubjectKind,
};
use crate::projection::State;
use crate::store::Db;

pub struct Clock {
    pub now: i64,
}

impl Clock {
    pub fn new(start: i64) -> Self {
        Clock { now: start }
    }
    pub fn tick(&mut self) -> i64 {
        let t = self.now;
        self.now += 1;
        t
    }
}

fn short_id(prefix: &str, material: &str) -> String {
    let mut h = Sha256::new();
    h.update(material.as_bytes());
    format!("{}-{}", prefix, &hex_encode(&h.finalize())[..12])
}

/// 从 spillover 生成补偿提案。绝不默默求逆：
/// - 严格奇异：candidates 为空，diagnostics 说明无法决定；
/// - 近奇异：精确逆 + 多个 ridge 解全部保留为合法候选，等待人工选择；
/// - 良态：单一精确逆候选。
pub fn propose_compensation(spec: &SpilloverSpec) -> CompensationProposal {
    let basis = spec.matrix.clone();
    let cond = basis.condition_estimate();
    let mut diagnostics = Vec::new();
    let class = classify(&basis);
    let candidates = match class {
        MatrixDiag::Singular => {
            diagnostics.push(format!(
                "spillover 矩阵严格奇异（条件数估计 {}），不存在唯一补偿矩阵；保留全部材料，不做求逆",
                if cond.is_finite() { format!("{:.3e}", cond) } else { "∞".into() }
            ));
            Vec::new()
        }
        MatrixDiag::NearSingular => {
            diagnostics.push(format!(
                "spillover 矩阵近奇异（条件数估计 {:.3e} > 1e8）：精确逆不被默默采用； ridge λ ∈ {{1e-2,1e-1,1}} 的全部合法解保留为候选",
                cond
            ));
            let mut cands = Vec::new();
            for lambda in [1e-2f64, 1e-1, 1.0] {
                let m = basis.ridge_solution(lambda);
                let residual = basis.roundtrip_residual(&m);
                let id = short_id("cc", &format!(
                    "{}\n{}\n{}", spec.spillover_id, lambda, serde_json::to_string(&m.rows).unwrap()
                ));
                cands.push(CompensationCandidate {
                    candidate_id: id,
                    method: "ridge".into(),
                    matrix: m,
                    ridge_lambda: Some(lambda),
                    residual,
                    condition_estimate: cond,
                });
            }
            cands
        }
        MatrixDiag::Invertible => {
            let inv = basis.inverse().expect("良态矩阵可逆");
            let residual = basis.roundtrip_residual(&inv);
            let id = short_id("cc", &format!(
                "{}\nexact\n{}", spec.spillover_id, serde_json::to_string(&inv.rows).unwrap()
            ));
            vec![CompensationCandidate {
                candidate_id: id,
                method: "exact".into(),
                matrix: inv,
                ridge_lambda: None,
                residual,
                condition_estimate: cond,
            }]
        }
    };
    let compensation_id = format!(
        "comp-{}",
        spec.spillover_id.strip_prefix("spill-").unwrap_or(&spec.spillover_id)
    );
    CompensationProposal {
        compensation_id,
        version: 1,
        spillover_id: spec.spillover_id.clone(),
        spillover_version: spec.version,
        basis,
        candidates,
        diagnostics,
    }
}

/// 某事件在给定状态、给定门版本下的二维坐标（带坐标基准）。
pub fn gate_coordinates(state: &State, gate: &GateVersion, event_values: &BTreeMap<String, f64>)
    -> Result<Point, Vec<String>> {
    let mut errs = Vec::new();
    let raw = |ch: &str| event_values.get(ch).copied().ok_or_else(|| format!("缺少通道 {}", ch));
    let mut project = |ch: &str| -> Result<f64, String> {
        let raw_v = raw(ch)?;
        if gate.basis == "raw" {
            return Ok(raw_v);
        }
        if let Some(comp_id) = gate.basis.strip_prefix("compensated:") {
            let comp = state.compensations.get(comp_id)
                .ok_or_else(|| format!("缺少补偿 {}", comp_id))?;
            let decision = state.decisions.values()
                .find(|d| d.active && d.subject_kind == SubjectKind::Compensation
                    && d.subject_id == comp_id
                    && d.subject_version == comp.version
                    && d.action == ReviewAction::Accept)
                .ok_or_else(|| format!("补偿 {} 尚未被接受，不能用于计算群体", comp_id))?;
            let chosen = decision.candidate_id.clone()
                .or_else(|| (comp.candidates.len() == 1)
                    .then(|| comp.candidates[0].candidate_id.clone()))
                .ok_or_else(|| format!("补偿 {} 有多个合法候选，必须人工选择后才能计算群体", comp_id))?;
            let candidate = comp.candidates.iter()
                .find(|c| c.candidate_id == chosen)
                .ok_or_else(|| format!("补偿 {} 的候选 {} 不存在", comp_id, chosen))?;
            let labels = &candidate.matrix.labels;
            let v: Vec<f64> = labels.iter().map(|l| event_values.get(l).copied()
                .ok_or_else(|| format!("缺少通道 {}", l))).collect::<Result<_,_>>()?;
            let out = candidate.matrix.apply(&v);
            let idx = labels.iter().position(|l| l == ch)
                .ok_or_else(|| format!("通道 {} 不在补偿矩阵中", ch))?;
            let val = out[idx];
            return if gate.basis.starts_with("transformed:") {
                Err("transformed 基准必须直接引用 transform".into())
            } else { Ok(val) };
        }
        if let Some(tid) = gate.basis.strip_prefix("transformed:") {
            let tv = state.transforms.get(tid)
                .ok_or_else(|| format!("缺少变换 {}", tid))?;
            if tv.channel_id != ch {
                return Err(format!("变换 {} 属于通道 {}，不能用于 {}", tid, tv.channel_id, ch));
            }
            return tv.spec.apply(raw_v).map_err(|e| format!("变换失败: {}", e));
        }
        Err(format!("未知坐标基准 {}", gate.basis))
    };
    let x = project(&gate.x_channel).map_err(|e| { errs.push(e); });
    let y = project(&gate.y_channel).map_err(|e| { errs.push(e); });
    match (x.ok(), y.ok()) {
        (Some(x), Some(y)) if errs.is_empty() => Ok(Point { x, y }),
        _ => Err(errs),
    }
}

/// 在某个门版本上计算群体结果；分母钉住父群版本。
pub fn compute_population(
    state: &State,
    gate: &GateVersion,
    edge_version: u32,
) -> Result<PopulationResult> {
    let dataset = state.dataset.as_ref().ok_or_else(|| anyhow::anyhow!("尚未导入数据集"))?;
    let edges: BTreeMap<String, &crate::domain::TreeEdgeVersion> =
        state.edges.values().map(|e| (e.child_gate.clone(), e)).collect();
    let edge = edges.get(&gate.gate_id).copied()
        .ok_or_else(|| anyhow::anyhow!("门 {} 缺少父子关系版本", gate.gate_id))?;
    if edge.version != edge_version {
        bail!("门 {} 的关系版本不匹配（期望 {}，当前 {}）", gate.gate_id, edge.version, edge_version);
    }

    // 分母：根群钉数据集版本；子群钉父群结果版本。
    let (denominator, denom_count, eligible_events): (DenominatorRef, usize, Vec<(String, BTreeMap<String, f64>)>) =
        match &edge.parent_gate {
            None => (
                DenominatorRef::Dataset { dataset_id: dataset.dataset_id.clone(), version: dataset.version },
                dataset.events.len(),
                collect_raw(dataset, &[]),
            ),
            Some(parent_gate) => {
                let parent = state.populations.get(parent_gate)
                    .ok_or_else(|| anyhow::anyhow!("父群 {} 尚无结果，必须先计算父群", parent_gate))?;
                if parent.stale {
                    bail!("父群 {}/v{} 已过期，必须先重算父群；不允许在过期分母上出新结论", parent_gate, parent.result.version);
                }
                let member_set: BTreeSet<&str> =
                    parent.result.member_event_ids.iter().map(String::as_str).collect();
                (
                    DenominatorRef::Population {
                        population_id: parent.result.population_id.clone(),
                        version: parent.result.version,
                    },
                    parent.result.member_event_ids.len(),
                    collect_raw(dataset, &member_set),
                )
            }
        };

    let mut member_event_ids = Vec::new();
    let mut diagnostics = BTreeMap::new();
    for (event_id, values) in &eligible_events {
        match gate_coordinates(state, gate, values) {
            Ok(point) => {
                if gate.geometry.contains(point) {
                    member_event_ids.push(event_id.clone());
                }
            }
            Err(errs) => {
                diagnostics.insert(event_id.clone(), errs);
            }
        }
    }
    member_event_ids.sort();
    let numerator = member_event_ids.len();
    let percent = if denom_count == 0 {
        0.0
    } else {
        100.0 * numerator as f64 / denom_count as f64
    };
    let mut diag_lines: Vec<String> = diagnostics.into_iter()
        .map(|(e, errs)| format!("事件 {} 未参与门判定: {}", e, errs.join("; ")))
        .collect();
    diag_lines.sort();

    let mut transform_versions = Vec::new();
    if let Some(tid) = gate.basis.strip_prefix("transformed:") {
        if let Some(tv) = state.transforms.get(tid) {
            transform_versions.push((tv.channel_id.clone(), tv.version));
        }
    }
    let (compensation_id, compensation_version) =
        if let Some(cid) = gate.basis.strip_prefix("compensated:") {
            state.compensations.get(cid).map(|c| (Some(c.compensation_id.clone()), Some(c.version))).unwrap_or((None, None))
        } else { (None, None) };

    Ok(PopulationResult {
        population_id: format!("pop-{}", gate.gate_id.strip_prefix("gate-").unwrap_or(&gate.gate_id)),
        version: 1,
        gate_id: gate.gate_id.clone(),
        gate_version: gate.version,
        edge_version,
        denominator,
        member_event_ids,
        numerator,
        denominator_count: denom_count,
        percent,
        compensation_id,
        compensation_version,
        transform_versions,
        diagnostics: diag_lines,
    })
}

fn collect_raw(dataset: &crate::domain::ChannelDataset, only: &BTreeSet<&str>)
    -> Vec<(String, BTreeMap<String, f64>)> {
    dataset.events.iter().filter_map(|row| {
        if !only.is_empty() && !only.contains(row.event_id.as_str()) {
            return None;
        }
        let mut map = BTreeMap::new();
        let mut bad = Vec::new();
        for v in &row.values {
            match v.value {
                Some(x) => { map.insert(v.channel_id.clone(), x); }
                None => bad.push(v.channel_id.clone()),
            }
        }
        if bad.is_empty() { Some((row.event_id.clone(), map)) } else { None }
    }).collect()
}

/// 依赖失效：某直接来源（通道数据 / 补偿 / 变换 / 门几何）修订后，
/// 计算被标记过期的精确群体集合 —— 沿门树向下传播，不全局替换。
pub fn stale_scope(state: &State, affected_gates: &BTreeSet<String>) -> Vec<StaleMarkPayload> {
    let edges: BTreeMap<String, Option<String>> = state.edges.values()
        .map(|e| (e.child_gate.clone(), e.parent_gate.clone())).collect();
    let mut downstream: BTreeSet<String> = BTreeSet::new();
    let mut stack: Vec<String> = affected_gates.iter().cloned().collect();
    while let Some(g) = stack.pop() {
        if downstream.insert(g.clone()) {
            for (child, parent) in &edges {
                if parent.as_deref() == Some(g.as_str()) {
                    stack.push(child.clone());
                }
            }
        }
    }
    let mut marks = Vec::new();
    for gate_id in downstream {
        if let Some(p) = state.populations.get(&format!("pop-{}", gate_id.strip_prefix("gate-").unwrap_or(&gate_id))) {
            marks.push(StaleMarkPayload {
                population_id: p.result.population_id.clone(),
                version: p.result.version,
                reason: format!("上游来源修订影响门 {}（几何/数据/参数版本已变化），旧结果保留但标记过期", gate_id),
                source_kind: "upstream-revision".into(),
                source_id: gate_id.clone(),
                source_version: 0,
            });
        }
    }
    marks.sort_by(|a, b| a.population_id.cmp(&b.population_id));
    marks
}

/// 通道原始值修订（dataset v2）影响哪些门：任何使用这些通道的门。
pub fn gates_using_channels(state: &State, channels: &BTreeSet<String>) -> BTreeSet<String> {
    state.gates.values()
        .filter(|g| channels.contains(&g.x_channel) || channels.contains(&g.y_channel))
        .map(|g| g.gate_id.clone())
        .collect()
}

/// 变换修订影响哪些门：引用该 transform 的门。
pub fn gates_using_transform(state: &State, transform_id: &str) -> BTreeSet<String> {
    let wanted = format!("transformed:{}", transform_id);
    state.gates.values().filter(|g| g.basis == wanted).map(|g| g.gate_id.clone()).collect()
}

/// 补偿修订影响哪些门：坐标基准引用该补偿的门。
pub fn gates_using_compensation(state: &State, compensation_id: &str) -> BTreeSet<String> {
    let wanted = format!("compensated:{}", compensation_id);
    state.gates.values().filter(|g| g.basis == wanted).map(|g| g.gate_id.clone()).collect()
}

/// 应用层：把一次审阅动作编排为若干追加事件（含过期标记），统一走乐观并发。
pub struct App<'a> {
    pub db: &'a Db,
    pub clock: Clock,
}

impl<'a> App<'a> {
    pub fn new(db: &'a Db, clock_start: i64) -> Self {
        App { db, clock: Clock::new(clock_start) }
    }

    pub fn fork(&self, parent_branch: &str, new_branch_id: &str, title: &str) -> Result<Event> {
        let head = self.db.branch_head(parent_branch)?
            .ok_or_else(|| anyhow::anyhow!("父分支不存在"))?;
        self.db.create_branch(new_branch_id, title, Some(parent_branch), Some(&head.1), self.clock_now())?;
        Ok(self.db.event_by_id(&head.1)?.unwrap())
    }

    fn clock_now(&self) -> i64 { self.clock.now }

    pub fn commit(&mut self, branch_id: &str, expected_head: Option<String>,
                  payload: EventPayload, actor: &str) -> Result<Event> {
        let at = self.clock.tick();
        self.db.commit(branch_id, expected_head, payload, actor, at)
    }

    /// 导入原始材料：原始字节 + 登记事件，失败整体回滚。
    pub fn import_blob(&mut self, branch_id: &str, expected_head: Option<String>,
                       media_type: &str, bytes: &[u8], actor: &str) -> Result<(Event, String)> {
        let at = self.clock.tick();
        let blob_id = self.db.put_blob(media_type, bytes, at)?;
        let payload = EventPayload::BlobImported {
            blob_id: blob_id.clone(), media_type: media_type.to_string(), byte_length: bytes.len(),
        };
        let ev = self.db.commit(branch_id, expected_head, payload, actor, at)?;
        Ok((ev, blob_id))
    }

    /// 修订门几何：旧群体结果保留，下游按依赖标过期（只影响局部）。
    pub fn revise_gate(&mut self, branch_id: &str, expected_head: Option<String>,
                       gate: GateVersion, actor: &str) -> Result<(Event, Option<Event>)> {
        let state = self.state(branch_id)?;
        let old = state.gates.get(&gate.gate_id).cloned();
        let geometry_changed = old.as_ref().map(|o| o.geometry != gate.geometry
            || o.basis != gate.basis || o.x_channel != gate.x_channel || o.y_channel != gate.y_channel)
            .unwrap_or(true);
        let mut gate = gate;
        if let Some(old) = &old {
            gate.version = old.version + 1;
        }
        let ev = self.commit(branch_id, expected_head, EventPayload::GateSet { gate }, actor)?;
        if geometry_changed {
            let state = self.state(branch_id)?;
            let mut affected = BTreeSet::new();
            affected.insert(ev_payload_gate_id(&ev.payload).to_string());
            let marks = stale_scope(&state, &affected);
            if !marks.is_empty() {
                let stale_ev = self.commit(branch_id, Some(ev.id.clone()),
                    EventPayload::StaleMarks { marks }, actor)?;
                return Ok((ev, Some(stale_ev)));
            }
        }
        Ok((ev, None))
    }

    pub fn revise_transform(&mut self, branch_id: &str, expected_head: Option<String>,
                            transform: TransformVersion, actor: &str) -> Result<(Event, Option<Event>)> {
        let state = self.state(branch_id)?;
        let mut transform = transform;
        if let Some(old) = state.transforms.get(&transform.transform_id) {
            transform.version = old.version + 1;
        }
        let ev = self.commit(branch_id, expected_head, EventPayload::TransformSet { transform }, actor)?;
        let state = self.state(branch_id)?;
        let tid = ev_payload_transform_id(&ev.payload).to_string();
        let affected = gates_using_transform(&state, &tid);
        let marks = stale_scope(&state, &affected);
        if marks.is_empty() { return Ok((ev, None)); }
        let stale_ev = self.commit(branch_id, Some(ev.id.clone()),
            EventPayload::StaleMarks { marks }, actor)?;
        Ok((ev, Some(stale_ev)))
    }

    pub fn register_spillover(&mut self, branch_id: &str, expected_head: Option<String>,
                              spec: SpilloverSpec, actor: &str) -> Result<(Event, Event)> {
        let proposal = propose_compensation(&spec);
        let at = self.clock.tick();
        let ev1 = self.db.commit(branch_id, expected_head,
            EventPayload::SpilloverRegistered { spec }, actor, at)?;
        let at2 = self.clock.tick();
        let ev2 = self.db.commit(branch_id, Some(ev1.id.clone()),
            EventPayload::CompensationProposed { proposal }, actor, at2)?;
        Ok((ev1, ev2))
    }

    /// 复核裁定。接受近奇异补偿时必须指定 candidate_id；拒绝/调整则下游保持待复核。
    pub fn decide(&mut self, branch_id: &str, expected_head: Option<String>,
                  decision_id: &str, subject_kind: SubjectKind, subject_id: &str,
                  subject_version: u32, action: ReviewAction,
                  candidate_id: Option<String>, note: &str, actor: &str) -> Result<Event> {
        let state = self.state(branch_id)?;
        if subject_kind == SubjectKind::Compensation && action == ReviewAction::Accept {
            let comp = state.compensations.get(subject_id)
                .ok_or_else(|| anyhow::anyhow!("补偿 {} 不存在", subject_id))?;
            if comp.version != subject_version {
                bail!("裁定对象版本过期");
            }
            if comp.candidates.is_empty() {
                bail!("该补偿无任何合法候选（严格奇异），不能接受；请重新采集单染对照");
            }
            if comp.candidates.len() > 1 {
                let cid = candidate_id.as_deref()
                    .ok_or_else(|| anyhow::anyhow!("存在多个合法候选，接受时必须显式选择 candidate_id"))?;
                if !comp.candidates.iter().any(|c| c.candidate_id == cid) {
                    bail!("candidate_id {} 不是该补偿的合法候选", cid);
                }
            }
        }
        self.commit(branch_id, expected_head, EventPayload::ReviewDecision {
            decision_id: decision_id.to_string(), subject_kind,
            subject_id: subject_id.to_string(), subject_version, action, candidate_id,
            note: note.to_string(),
        }, actor)
    }

    pub fn revert_decision(&mut self, branch_id: &str, expected_head: Option<String>,
                           decision_id: &str, note: &str, actor: &str) -> Result<Event> {
        self.commit(branch_id, expected_head, EventPayload::DecisionReverted {
            decision_id: decision_id.to_string(), note: note.to_string(),
        }, actor)
    }

    pub fn state(&self, branch_id: &str) -> Result<State> {
        let events = crate::projection::branch_events(self.db, branch_id, None)?;
        Ok(crate::projection::replay(events))
    }

    pub fn state_at(&self, branch_id: &str, seq: i64) -> Result<State> {
        let events = crate::projection::branch_events(self.db, branch_id, Some(seq))?;
        Ok(crate::projection::replay(events))
    }
}

fn ev_payload_gate_id(p: &EventPayload) -> &str {
    match p {
        EventPayload::GateSet { gate } => &gate.gate_id,
        _ => "unknown",
    }
}

fn ev_payload_transform_id(p: &EventPayload) -> &str {
    match p {
        EventPayload::TransformSet { transform } => &transform.transform_id,
        _ => "unknown",
    }
}

/// 生成新的事件身份（供导入档案后的 marker 使用，不参与档案身份）。
pub fn fresh_event_id(branch_id: &str, seq: i64, parent_id: &Option<String>,
                      payload: &EventPayload, actor: &str) -> String {
    event_identity(branch_id, seq, parent_id, payload, actor)
}
