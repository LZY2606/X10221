//! 事件重放：把 append-only 事件折叠成某一时刻的完整视图。
//!
//! 不可变原则：任何修订都只追加；群体旧结果保留，过期仅打标记（stale=true），
//! 绝不覆盖。近奇异补偿在显式裁定前阻塞派生门。

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::compute::{
    asinh_transform, invert_matrix, point_in_polygon, regularized_inverse, MatrixInverse,
    Membership, RIDGE,
};
use crate::domain::*;
use crate::util::stable_id;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum PopStatus {
    Ok,
    Ambiguous { candidate_set_id: String },
    Blocked { reason: String },
}

/// 一次派生群体（门 × 其钉住的全部输入版本 × 父群版本 × 边界裁定）。
#[derive(Debug, Clone, Serialize)]
pub struct DerivedPop {
    pub pop_id: String,
    pub gate_id: String,
    pub gate_version_id: String,
    pub gate_name: String,
    pub parent_pop_id: Option<String>,
    pub parent_gate_id: Option<String>,
    pub parent_gate_version_id: Option<String>,
    /// 分母：根门为事件总数；其余钉住父群当次分子快照。
    pub denominator: Option<i64>,
    pub numerator: Option<i64>,
    pub percent: Option<f64>,
    pub status: PopStatus,
    pub stale: bool,
    pub stale_reasons: Vec<String>,
    pub created_seq: i64,
    pub created_by_event: String,
    pub dataset_version: String,
    pub compensation_version: Option<String>,
    pub transform_version: Option<String>,
    pub numerator_event_ids: Vec<String>,
    pub ambiguous_event_ids: Vec<String>,
    pub candidate_set_id: Option<String>,
    pub diagnostics: Vec<String>,
    pub basis: String,
    pub x_unit: String,
    pub y_unit: String,
    /// 使用了有争议的（显式正则化）求逆。
    pub contested: bool,
}

/// 机器可读判定依据。
#[derive(Debug, Clone, Serialize)]
pub struct Judgment {
    pub rule_id: String,
    pub severity: String,
    pub subject: String,
    pub message: String,
    pub at_seq: Option<i64>,
    pub evidence: serde_json::Value,
}

#[derive(Debug, Default, Clone)]
pub struct State {
    pub dataset_head: Option<String>,
    pub datasets: BTreeMap<String, DatasetVersion>,
    pub spillover_head: Option<String>,
    pub spillover_metas: BTreeMap<String, SpilloverMeta>,
    pub compensation_head: Option<String>,
    pub compensations: BTreeMap<String, CompensationMatrix>,
    pub transform_head: Option<String>,
    pub transforms: BTreeMap<String, TransformParams>,
    pub gate_heads: BTreeMap<String, String>,
    pub gates: BTreeMap<String, BTreeMap<String, GateVersion>>,
    pub relations_head: Option<String>,
    pub relations: BTreeMap<String, GateRelationVersion>,
    pub candidate_sets: BTreeMap<String, CandidateSet>,
    pub retracted: BTreeSet<String>,
}

impl State {
    pub fn gate_head(&self, gate_id: &str) -> Option<&GateVersion> {
        let v = self.gate_heads.get(gate_id)?;
        self.gates.get(gate_id).and_then(|m| m.get(v))
    }

    fn apply_bundle(&mut self, bundle: &RevisionBundle) {
        if let Some(d) = &bundle.dataset {
            self.datasets.insert(d.version_id.clone(), d.clone());
            self.dataset_head = Some(d.version_id.clone());
        }
        if let Some(s) = &bundle.spillover_meta {
            self.spillover_metas.insert(s.version_id.clone(), s.clone());
            self.spillover_head = Some(s.version_id.clone());
        }
        if let Some(c) = &bundle.compensation {
            self.compensations.insert(c.version_id.clone(), c.clone());
            self.compensation_head = Some(c.version_id.clone());
        }
        if let Some(t) = &bundle.transform {
            self.transforms.insert(t.version_id.clone(), t.clone());
            self.transform_head = Some(t.version_id.clone());
        }
        for g in &bundle.gates {
            self.gate_heads.insert(g.gate_id.clone(), g.version_id.clone());
            self.gates
                .entry(g.gate_id.clone())
                .or_default()
                .insert(g.version_id.clone(), g.clone());
        }
        if let Some(r) = &bundle.relations {
            self.relations.insert(r.version_id.clone(), r.clone());
            self.relations_head = Some(r.version_id.clone());
        }
    }
}

pub struct Replay {
    pub state: State,
    pub pops: Vec<DerivedPop>,
    pub judgments: Vec<Judgment>,
    pub last_seq: i64,
}

pub fn replay(events: &[BranchEvent], up_to_seq: Option<i64>) -> Replay {
    let mut state = State::default();
    let mut inv_cache: BTreeMap<String, Result<MatrixInverse, String>> = BTreeMap::new();
    let mut pops: Vec<DerivedPop> = Vec::new();

    // 先扫描撤回目标：撤回恢复前一分支状态（目标事件及其结论不参与折叠）。
    for ev in events {
        if let Some(limit) = up_to_seq {
            if ev.seq > limit {
                break;
            }
        }
        if let EventPayload::Retracted { target_event_id, .. } = &ev.payload {
            state.retracted.insert(target_event_id.clone());
        }
    }

    for ev in events {
        if let Some(limit) = up_to_seq {
            if ev.seq > limit {
                break;
            }
        }
        if state.retracted.contains(&ev.event_id) {
            continue;
        }
        match &ev.payload {
            EventPayload::Retracted { .. } => {
                continue;
            }
            EventPayload::Imported { bundle, .. }
            | EventPayload::ParameterRevised { bundle, .. }
            | EventPayload::SourcePatched { bundle, .. }
            | EventPayload::GateRevised { bundle, .. }
            | EventPayload::GateRenamed { bundle }
            | EventPayload::Recomputed { bundle, .. } => {
                state.apply_bundle(bundle);
                register_material_candidates(&mut state, bundle, ev);
            }
            EventPayload::Decision {
                candidate_set_id,
                accepted_candidate_id,
                rejected,
                rationale,
            } => {
                if let Some(set) = state.candidate_sets.get_mut(candidate_set_id) {
                    set.accepted = Some(accepted_candidate_id.clone());
                    set.rejected = rejected.clone();
                    set.decided_by_event = Some(ev.event_id.clone());
                    set.rationale = Some(rationale.clone());
                }
                // 拒绝近奇异补偿版本：头部回退到其 supersedes（保留前版为当前）。
                if let Some(set) = state.candidate_sets.get(candidate_set_id) {
                    if set.kind == "near-singular-compensation" {
                        let rejected_now = matches!(accepted_candidate_id, None)
                            || accepted_candidate_id
                                .as_ref()
                                .is_some_and(|a| a.ends_with(":reject"));
                        if rejected_now {
                            if let Some(cv) = set.subject.strip_prefix("compensation:") {
                                if let Some(prev) = state
                                    .compensations
                                    .get(cv)
                                    .and_then(|c| c.supersedes.clone())
                                {
                                    state.compensation_head = Some(prev);
                                }
                            }
                        }
                    }
                }
            }
        }

        let requested = requested_gates(&ev.payload, &state);
        derive_requested(&mut state, &mut inv_cache, &mut pops, &requested, ev);
    }

    mark_staleness(&state, &mut pops);
    let judgments = build_judgments(&state, &pops, events);
    let last_seq = up_to_seq
        .or_else(|| events.iter().map(|e| e.seq).max())
        .unwrap_or(0);
    Replay { state, pops, judgments, last_seq }
}

/// 材料变更时登记候选集（近奇异补偿；门边界歧义在门版本出现时建立）。
fn register_material_candidates(state: &mut State, bundle: &RevisionBundle, ev: &BranchEvent) {
    if let Some(comp) = &bundle.compensation {
        let det = crate::compute::determinant(&comp.matrix);
        if det.abs() < crate::compute::SINGULAR_DET_THRESHOLD {
            let cs_id = stable_id("CS-COMP", &comp.version_id);
            if !state.candidate_sets.contains_key(&cs_id) {
                state.candidate_sets.insert(
                    cs_id.clone(),
                    CandidateSet {
                        candidate_set_id: cs_id,
                        created_by_event: ev.event_id.clone(),
                        kind: "near-singular-compensation".to_string(),
                        subject: format!("compensation:{}", comp.version_id),
                        candidates: vec![
                            Candidate {
                                candidate_id: format!("{comp_v}:reject", comp_v = comp.version_id),
                                label: "拒绝该补偿版本（保留前版，不求逆）".to_string(),
                                detail: format!(
                                    "|det|={det:.3e} 低于阈值 {t:.0e}：不做隐式求逆，门继续钉前版矩阵",
                                    t = crate::compute::SINGULAR_DET_THRESHOLD
                                ),
                            },
                            Candidate {
                                candidate_id: format!("{comp_v}:regularize", comp_v = comp.version_id),
                                label: "显式正则化后求逆（标记为有争议人工修正）".to_string(),
                                detail: format!("S' = S + {}·I，结果 contested=true", RIDGE),
                            },
                        ],
                        accepted: None,
                        rejected: Vec::new(),
                        decided_by_event: None,
                        rationale: None,
                    },
                );
            }
        }
    }
    for gate in &bundle.gates {
        // 边界候选集在派生时发现边界点再建立（此处无法评估），见 derive_one_gate。
        let _ = gate;
    }
}

fn requested_gates(payload: &EventPayload, state: &State) -> Vec<String> {
    let all = || state.gate_heads.keys().cloned().collect::<Vec<_>>();
    match payload {
        EventPayload::Imported { .. } => all(),
        EventPayload::ParameterRevised { bundle, .. }
        | EventPayload::GateRevised { bundle, .. }
        | EventPayload::GateRenamed { bundle }
        | EventPayload::Recomputed { bundle, .. } => bundle.gates.iter().map(|g| g.gate_id.clone()).collect(),
        EventPayload::SourcePatched { .. } => Vec::new(),
        EventPayload::Decision { candidate_set_id, .. } => state
            .candidate_sets
            .get(candidate_set_id)
            .map(|s| {
                // subject: `<gate_id>|...`（边界）或 `compensation:<ver>`（补偿）。
                let head = s.subject.split('|').next().unwrap_or("");
                if state.gate_heads.contains_key(head) {
                    // 裁定解决父群歧义后，下游门需要按新父群版本重新钉分母。
                    let mut out = vec![head.to_string()];
                    out.extend(descendants_of(state, head));
                    out
                } else {
                    Vec::new()
                }
            })
            .unwrap_or_default(),
        EventPayload::Retracted { .. } => Vec::new(),
    }
}

fn derive_requested(
    state: &mut State,
    inv_cache: &mut BTreeMap<String, Result<MatrixInverse, String>>,
    pops: &mut Vec<DerivedPop>,
    requested: &[String],
    ev: &BranchEvent,
) {
    let relations = state.relations_head.as_ref().and_then(|v| state.relations.get(v).cloned());
    let order = topological_order(state, relations.as_ref());
    let wanted: BTreeSet<String> = requested.iter().cloned().collect();
    for gate_id in order {
        if !wanted.contains(&gate_id) {
            continue;
        }
        let gate = match state.gate_head(&gate_id) {
            Some(g) => g.clone(),
            None => continue,
        };
        let pop = derive_one_gate(state, inv_cache, pops, &gate, ev);
        pops.push(pop);
    }
}

fn descendants_of(state: &State, gate_id: &str) -> Vec<String> {
    let relations = state.relations_head.as_ref().and_then(|v| state.relations.get(v));
    let Some(rel) = relations else { return Vec::new() };
    let mut out = Vec::new();
    let mut frontier = vec![gate_id.to_string()];
    while let Some(g) = frontier.pop() {
        for (child, parent) in &rel.edges {
            if parent == &g && !out.contains(child) {
                out.push(child.clone());
                frontier.push(child.clone());
            }
        }
    }
    out
}

fn topological_order(state: &State, relations: Option<&GateRelationVersion>) -> Vec<String> {
    let edges: BTreeMap<String, String> = relations
        .map(|r| r.edges.iter().cloned().collect())
        .unwrap_or_default();
    let mut order = Vec::new();
    let mut visiting = BTreeSet::new();
    fn visit(
        g: String,
        edges: &BTreeMap<String, String>,
        order: &mut Vec<String>,
        visiting: &mut BTreeSet<String>,
        done: &mut BTreeSet<String>,
    ) {
        if done.contains(&g) || !visiting.insert(g.clone()) {
            return;
        }
        if let Some(parent) = edges.get(&g) {
            visit(parent.clone(), edges, order, visiting, done);
        }
        visiting.remove(&g);
        done.insert(g.clone());
        order.push(g);
    }
    let mut done = BTreeSet::new();
    for g in state.gate_heads.keys().cloned() {
        visit(g, &edges, &mut order, &mut visiting, &mut done);
    }
    order
}

fn latest_pop_for<'a>(
    pops: &'a [DerivedPop],
    gate_id: &str,
    created_before: i64,
) -> Option<&'a DerivedPop> {
    pops.iter()
        .filter(|p| p.gate_id == gate_id && p.created_seq <= created_before)
        .max_by_key(|p| p.created_seq)
}

#[derive(Debug, Clone)]
pub(crate) struct EventPoint {
    event_id: String,
    x: Option<f64>,
    y: Option<f64>,
    raw_x: String,
    raw_y: String,
    diagnostics: Vec<String>,
    membership: Membership,
}

fn derive_one_gate(
    state: &mut State,
    inv_cache: &mut BTreeMap<String, Result<MatrixInverse, String>>,
    pops: &mut Vec<DerivedPop>,
    gate: &GateVersion,
    ev: &BranchEvent,
) -> DerivedPop {
    let relations = state.relations_head.as_ref().and_then(|v| state.relations.get(v).cloned());
    let parent_gate_id = relations
        .and_then(|r| r.edges.iter().find(|(c, _)| c == &gate.gate_id).map(|(_, p)| p.clone()));

    let (parent_pop_id, parent_gate_version_id, denominator) = match &parent_gate_id {
        Some(pg) => match latest_pop_for(pops, pg, ev.seq) {
            Some(pp) => (Some(pp.pop_id.clone()), Some(pp.gate_version_id.clone()), pp.numerator),
            None => (None, None, None),
        },
        None => {
            let total = state
                .datasets
                .get(&gate.space.dataset_version)
                .map(|d| d.events.len() as i64);
            (None, None, total)
        }
    };

    let mut base = DerivedPop {
        pop_id: String::new(),
        gate_id: gate.gate_id.clone(),
        gate_version_id: gate.version_id.clone(),
        gate_name: gate.name.clone(),
        parent_pop_id: parent_pop_id.clone(),
        parent_gate_id: parent_gate_id.clone(),
        parent_gate_version_id: parent_gate_version_id.clone(),
        denominator,
        numerator: None,
        percent: None,
        status: PopStatus::Ok,
        stale: false,
        stale_reasons: Vec::new(),
        created_seq: ev.seq,
        created_by_event: ev.event_id.clone(),
        dataset_version: gate.space.dataset_version.clone(),
        compensation_version: gate.space.compensation_version.clone(),
        transform_version: gate.space.transform_version.clone(),
        numerator_event_ids: Vec::new(),
        ambiguous_event_ids: Vec::new(),
        candidate_set_id: None,
        diagnostics: Vec::new(),
        basis: gate.space.basis.clone(),
        x_unit: gate.space.x_unit.clone(),
        y_unit: gate.space.y_unit.clone(),
        contested: false,
    };

    if matches!(gate.kind, GateKind::Root) {
        let ids: Vec<String> = state
            .datasets
            .get(&gate.space.dataset_version)
            .map(|d| d.events.iter().map(|e| e.event_id.clone()).collect())
            .unwrap_or_default();
        let n = ids.len() as i64;
        return finish_pop(base, ids, Vec::new(), n, PopStatus::Ok, None);
    }

    if parent_gate_id.is_some() && parent_pop_id.is_none() {
        return blocked_pop(base, "parent population has no resolvable version at this seq".to_string());
    }

    let parent_status = parent_pop_id
        .as_ref()
        .and_then(|pid| pops.iter().find(|p| &p.pop_id == pid))
        .map(|p| p.status.clone());
    match parent_status {
        Some(PopStatus::Blocked { reason }) => {
            return blocked_pop(base, format!("parent population blocked: {reason}"));
        }
        Some(PopStatus::Ambiguous { .. }) => {
            return blocked_pop(base, "parent population has unresolved boundary candidates".to_string());
        }
        _ => {}
    }

    // 近奇异补偿：绝不默默求逆。
    let mut contested_inverse = false;
    if let Some(comp_vid) = gate.space.compensation_version.clone() {
        let comp = state.compensations.get(&comp_vid).cloned();
        let comp = match comp {
            Some(c) => c,
            None => return blocked_pop(base, format!("missing compensation version {comp_vid}")),
        };
        if !inv_cache.contains_key(&comp_vid) {
            inv_cache.insert(comp_vid.clone(), invert_matrix(&comp.matrix));
        }
        if let Err(err) = inv_cache.get(&comp_vid).unwrap() {
            let cs_id = stable_id("CS-COMP", &comp.version_id);
            let decision = state
                .candidate_sets
                .get(&cs_id)
                .and_then(|s| s.accepted.clone())
                .flatten();
            match decision.as_deref() {
                Some(d) if d.ends_with(":regularize") => {
                    if !inv_cache.contains_key(&format!("{comp_vid}#regularized")) {
                        inv_cache.insert(
                            format!("{comp_vid}#regularized"),
                            Ok(regularized_inverse(&comp.matrix, RIDGE)),
                        );
                    }
                    contested_inverse = true;
                }
                Some(other) => {
                    return blocked_pop(
                        base,
                        format!("accepted candidate {other} does not permit numeric compensation"),
                    );
                }
                None => {
                    let rejected = state
                        .candidate_sets
                        .get(&cs_id)
                        .and_then(|s| s.accepted.clone())
                        .is_some_and(|a| a.is_none());
                    if rejected {
                        return blocked_pop(
                            base,
                            format!("{err}; near-singular compensation explicitly rejected"),
                        );
                    }
                    return blocked_pop(
                        base,
                        format!("{err}; awaiting reviewer decision (no silent inverse)"),
                    );
                }
            }
        }
    }

    let inv_key = if contested_inverse {
        gate.space
            .compensation_version
            .as_ref()
            .map(|v| format!("{v}#regularized"))
    } else {
        gate.space.compensation_version.clone()
    };
    let points = evaluate_gate(state, inv_cache, inv_key.as_deref(), gate);

    let parent_members: BTreeSet<String> = parent_pop_id
        .as_ref()
        .and_then(|pid| pops.iter().find(|p| &p.pop_id == pid))
        .map(|p| p.numerator_event_ids.iter().cloned().collect())
        .unwrap_or_default();

    let mut in_ids = Vec::new();
    let mut boundary_ids = Vec::new();
    let mut diagnostics = Vec::new();
    for pt in points {
        if parent_gate_id.is_some() && !parent_members.contains(&pt.event_id) {
            continue;
        }
        diagnostics.extend(pt.diagnostics);
        match pt.membership {
            Membership::In => in_ids.push(pt.event_id),
            Membership::Out => {}
            Membership::BoundaryAmbiguous => boundary_ids.push(pt.event_id),
        }
    }
    base.diagnostics = diagnostics;

    if !boundary_ids.is_empty() {
        let cs_id = stable_id("CS-BND", &format!("{}|{}", gate.gate_id, gate.version_id));
        ensure_boundary_candidates(state, gate, &boundary_ids, &cs_id, ev);
        let accepted = state
            .candidate_sets
            .get(&cs_id)
            .and_then(|s| s.accepted.clone())
            .flatten();
        let rejected_all = state
            .candidate_sets
            .get(&cs_id)
            .and_then(|s| s.accepted.clone())
            .is_some_and(|a| a.is_none());
        match accepted.as_deref() {
            Some(a) if a.ends_with(":include") => {
                in_ids.extend(boundary_ids.clone());
            }
            Some(_) => {} // exclude
            None => {
                let status = PopStatus::Ambiguous { candidate_set_id: cs_id.clone() };
                return finish_pop(base, in_ids, boundary_ids, 0, status, Some(cs_id));
            }
        }
        let _ = rejected_all;
    }

    let n = in_ids.len() as i64;
    let mut pop = finish_pop(base, in_ids, Vec::new(), n, PopStatus::Ok, None);
    pop.contested = contested_inverse;
    pop
}

fn ensure_boundary_candidates(
    state: &mut State,
    gate: &GateVersion,
    boundary_ids: &[String],
    cs_id: &str,
    ev: &BranchEvent,
) {
    if state.candidate_sets.contains_key(cs_id) {
        return;
    }
    state.candidate_sets.insert(
        cs_id.to_string(),
        CandidateSet {
            candidate_set_id: cs_id.to_string(),
            created_by_event: ev.event_id.clone(),
            kind: "boundary-membership".to_string(),
            subject: format!("{}|{}", gate.gate_id, gate.version_id),
            candidates: vec![
                Candidate {
                    candidate_id: format!("{}:include", gate.version_id),
                    label: format!("边界点计入「{}」（闭区间）", gate.name),
                    detail: format!("边界事件: {}", boundary_ids.join(", ")),
                },
                Candidate {
                    candidate_id: format!("{}:exclude", gate.version_id),
                    label: format!("边界点排除出「{}」（开区间）", gate.name),
                    detail: format!("边界事件: {}", boundary_ids.join(", ")),
                },
            ],
            accepted: None,
            rejected: Vec::new(),
            decided_by_event: None,
            rationale: None,
        },
    );
}

fn blocked_pop(mut base: DerivedPop, reason: String) -> DerivedPop {
    base.status = PopStatus::Blocked { reason };
    base.pop_id = pop_key(&base, "blocked");
    base
}

fn finish_pop(
    mut base: DerivedPop,
    in_ids: Vec<String>,
    ambiguous_ids: Vec<String>,
    numerator: i64,
    status: PopStatus,
    cs_id: Option<String>,
) -> DerivedPop {
    base.numerator_event_ids = in_ids;
    base.ambiguous_event_ids = ambiguous_ids;
    base.numerator = Some(numerator);
    base.percent = base
        .denominator
        .filter(|d| *d > 0)
        .map(|d| (numerator as f64) * 100.0 / d as f64);
    base.status = status;
    base.candidate_set_id = cs_id.clone();
    let resolution = match &base.status {
        PopStatus::Ambiguous { .. } => cs_id.unwrap_or_else(|| "pending".to_string()),
        PopStatus::Blocked { .. } => "blocked".to_string(),
        PopStatus::Ok if base.contested => "resolved-contested".to_string(),
        PopStatus::Ok => "resolved".to_string(),
    };
    base.pop_id = pop_key(&base, &resolution);
    base
}

/// 稳定群体身份：门版本 + 数据/补偿/变换版本 + 父群版本 + 裁定结果。
fn pop_key(p: &DerivedPop, resolution: &str) -> String {
    let key = format!(
        "{}|{}|{}|{}|{}|{}|{}",
        p.gate_id,
        p.gate_version_id,
        p.dataset_version,
        p.compensation_version.clone().unwrap_or_else(|| "-".into()),
        p.transform_version.clone().unwrap_or_else(|| "-".into()),
        p.parent_pop_id.clone().unwrap_or_else(|| "-".into()),
        resolution,
    );
    stable_id("POP", &key)
}

/// 计算门坐标空间中每个事件的坐标与成员判定。
pub(crate) fn evaluate_gate(
    state: &State,
    inv_cache: &BTreeMap<String, Result<MatrixInverse, String>>,
    inv_key: Option<&str>,
    gate: &GateVersion,
) -> Vec<EventPoint> {
    let dataset = match state.datasets.get(&gate.space.dataset_version) {
        Some(d) => d,
        None => return Vec::new(),
    };
    let cofactor = gate
        .space
        .transform_version
        .as_ref()
        .and_then(|tv| state.transforms.get(tv))
        .map(|t| t.cofactor);
    let inverse = inv_key
        .and_then(|k| inv_cache.get(k))
        .and_then(|r| r.as_ref().ok());
    let comp = gate
        .space
        .compensation_version
        .as_ref()
        .and_then(|cv| state.compensations.get(cv));

    dataset
        .events
        .iter()
        .map(|event| {
            let mut diags = Vec::new();
            let x_raw = channel_raw(event, &gate.space.x_channel);
            let y_raw = channel_raw(event, &gate.space.y_channel);
            let mut x = coordinate(
                &gate.space.x_channel, event, comp, inverse, cofactor, &mut diags,
            );
            let mut y = coordinate(
                &gate.space.y_channel, event, comp, inverse, cofactor, &mut diags,
            );
            if gate.space.basis == "asinh-compensated" {
                x = x.map(|v| asinh_transform(v, cofactor.unwrap_or(150.0)));
                y = y.map(|v| asinh_transform(v, cofactor.unwrap_or(150.0)));
            }
            let membership = match (x, y) {
                (Some(xv), Some(yv)) => {
                    let (inside, on_boundary) =
                        point_in_polygon(&Point { x: xv, y: yv }, &gate.polygon);
                    if on_boundary {
                        match gate.boundary {
                            BoundaryPolicy::Inclusive => Membership::In,
                            BoundaryPolicy::Exclusive => Membership::Out,
                            BoundaryPolicy::Ambiguous => Membership::BoundaryAmbiguous,
                        }
                    } else if inside {
                        Membership::In
                    } else {
                        Membership::Out
                    }
                }
                _ => Membership::Out,
            };
            EventPoint {
                event_id: event.event_id.clone(),
                x,
                y,
                raw_x: x_raw,
                raw_y: y_raw,
                diagnostics: diags,
                membership,
            }
        })
        .collect()
}

fn channel_raw(event: &FcsEvent, channel: &str) -> String {
    event
        .values
        .iter()
        .find(|v| v.channel == channel)
        .map(|v| v.raw.clone())
        .unwrap_or_default()
}

/// 解析单通道坐标：原始 ADC，或补偿后的线性值。
fn coordinate(
    channel: &str,
    event: &FcsEvent,
    comp: Option<&CompensationMatrix>,
    inverse: Option<&MatrixInverse>,
    _cofactor: Option<f64>,
    diags: &mut Vec<String>,
) -> Option<f64> {
    let rv = event.values.iter().find(|v| v.channel == channel)?;
    diags.extend(rv.diagnostics.iter().cloned());
    let value = rv.numeric?;
    match (comp, inverse) {
        (Some(c), Some(inv)) => {
            // y_comp = S^-1 * y_raw（荧光通道子空间）。
            let idx = c.channels.iter().position(|ch| ch == channel)?;
            // comp = S^-1 · raw：comp[idx] = Σ_k inv[idx][k] · raw[k]
            let mut v = 0.0;
            for (k, raw_ch) in c.channels.iter().enumerate() {
                let raw_rv = event
                    .values
                    .iter()
                    .find(|x| &x.channel == raw_ch)?;
                let raw_val = raw_rv.numeric.unwrap_or(0.0);
                v += inv.inverse[idx][k] * raw_val;
            }
            if v < 0.0 {
                diags.push(format!("{channel}: over-compensation produced {v:.2} (compensated-linear), kept"));
            }
            Some(v)
        }
        _ => Some(value),
    }
}

fn superseded_chain(state: &State, vid: &str) -> Vec<String> {
    // 给定版本 id，返回同一材料链上被它取代的全部旧版本 id。
    let mut chain = Vec::new();
    for (head, d) in &state.datasets {
        if d.supersedes.is_some() && collect_chain_ds(state, head).contains(&vid.to_string()) {
            if head != &vid {
                chain.push(head.clone());
            }
        }
    }
    chain
}

fn collect_chain_ds(state: &State, head: &str) -> Vec<String> {
    let mut out = vec![head.to_string()];
    let mut cur = head.to_string();
    while let Some(d) = state.datasets.get(&cur) {
        match &d.supersedes {
            Some(old) => {
                out.push(old.clone());
                cur = old.clone();
            }
            None => break,
        }
    }
    out
}

/// 过期传播：修订只追加；当前指针之外的派生结果标 stale，绝不覆盖。
fn mark_staleness(state: &State, pops: &mut [DerivedPop]) {
    let relations = state
        .relations_head
        .as_ref()
        .and_then(|v| state.relations.get(v))
        .cloned()
        .unwrap_or(GateRelationVersion {
            version_id: String::new(),
            supersedes: None,
            edges: Vec::new(),
            note: String::new(),
        });
    let parent_of: BTreeMap<String, String> =
        relations.edges.iter().cloned().collect();

    for i in 0..pops.len() {
        let mut reasons = Vec::new();
        let p = &pops[i];

        // 1) 门版本已不是当前头部（门被修订/改名改名不改版本线以外的内容）。
        if state.gate_heads.get(&p.gate_id).map(|v| v != &p.gate_version_id).unwrap_or(true) {
            reasons.push(format!("gate-version-superseded:{}", p.gate_version_id));
        }

        // 2) 数据集版本被局部源数据改动取代（按数据版本链）。
        if state.dataset_head.as_deref() != Some(p.dataset_version.as_str()) {
            let head = state.dataset_head.clone().unwrap_or_default();
            if collect_chain_ds(state, &head).contains(&p.dataset_version) {
                reasons.push(format!("dataset-superseded-by:{}", head));
            }
        }

        // 3) 钉住的补偿版本被参数修订取代。
        if let Some(cv) = &p.compensation_version {
            if state.compensation_head.as_deref() != Some(cv.as_str()) {
                let head = state.compensation_head.clone().unwrap_or_default();
                if comp_chain_contains(state, &head, cv) {
                    reasons.push(format!("compensation-superseded-by:{}", head));
                }
            }
        }

        // 4) 钉住的变换版本被参数修订取代。
        if let Some(tv) = &p.transform_version {
            if state.transform_head.as_deref() != Some(tv.as_str()) {
                let head = state.transform_head.clone().unwrap_or_default();
                if transform_chain_contains(state, &head, tv) {
                    reasons.push(format!("transform-superseded-by:{}", head));
                }
            }
        }

        // 5) 父群存在更新的派生（分母钉版：上游修订后本结果过期）。
        if let Some(pg) = &p.parent_gate_id {
            if let Some(parent_head_v) = state.gate_heads.get(pg) {
                if Some(parent_head_v.as_str()) != p.parent_gate_version_id.as_deref() {
                    reasons.push(format!("parent-gate-revised:{}", pg));
                }
            }
            let newer_parent = pops
                .iter()
                .filter(|q| {
                    q.gate_id == *pg
                        && q.created_seq > p.created_seq
                        && matches!(q.status, PopStatus::Ok)
                })
                .min_by_key(|q| q.created_seq);
            if let Some(np) = newer_parent {
                reasons.push(format!("parent-pop-updated:{}", np.pop_id));
            }
        }

        let _ = superseded_chain;
        let _ = &parent_of;
        if !reasons.is_empty() {
            pops[i].stale = true;
            pops[i].stale_reasons = reasons;
        }
    }
}

fn comp_chain_contains(state: &State, head: &str, old: &str) -> bool {
    let mut cur = head.to_string();
    loop {
        if cur == old {
            return true;
        }
        match state.compensations.get(&cur).and_then(|c| c.supersedes.clone()) {
            Some(prev) => cur = prev,
            None => return false,
        }
    }
}

fn transform_chain_contains(state: &State, head: &str, old: &str) -> bool {
    let mut cur = head.to_string();
    loop {
        if cur == old {
            return true;
        }
        match state.transforms.get(&cur).and_then(|t| t.supersedes.clone()) {
            Some(prev) => cur = prev,
            None => return false,
        }
    }
}

fn build_judgments(
    state: &State,
    pops: &[DerivedPop],
    events: &[BranchEvent],
) -> Vec<Judgment> {
    let mut out = Vec::new();

    for comp in state.compensations.values() {
        let det = crate::compute::determinant(&comp.matrix);
        if det.abs() < crate::compute::SINGULAR_DET_THRESHOLD {
            let cs_id = stable_id("CS-COMP", &comp.version_id);
            let set = state.candidate_sets.get(&cs_id);
            let (severity, message) = match set.and_then(|s| s.accepted.clone()) {
                Some(Some(c)) if c.ends_with(":regularize") => (
                    "decided-contested",
                    "近奇异补偿矩阵：已由审阅者显式选择正则化求逆（contested=true，非默默求逆）",
                ),
                Some(Some(_)) => ("decided", "近奇异补偿候选被选定"),
                Some(None) => ("decided-rejected", "近奇异补偿矩阵被拒绝，派生门阻塞且保留"),
                None => ("blocked", "近奇异补偿矩阵不能被默默求逆，等待人工裁定"),
            };
            out.push(Judgment {
                rule_id: "comp.near_singular_no_silent_inverse".to_string(),
                severity: severity.to_string(),
                subject: format!("compensation:{}", comp.version_id),
                message: message.to_string(),
                at_seq: events
                    .iter()
                    .find(|e| matches!(&e.payload,
                        EventPayload::ParameterRevised { bundle, .. }
                            if bundle.compensation.as_ref().map(|c| c.version_id == comp.version_id).unwrap_or(false)))
                    .map(|e| e.seq),
                evidence: serde_json::json!({
                    "determinant": det,
                    "threshold": crate::compute::SINGULAR_DET_THRESHOLD,
                    "candidate_set_id": cs_id,
                    "matrix_unit": comp.unit,
                    "matrix_basis": comp.basis,
                }),
            });
        }
    }

    for set in state.candidate_sets.values() {
        if set.kind == "boundary-membership" {
            let unresolved = set.accepted.is_none();
            out.push(Judgment {
                rule_id: "gate.boundary_candidates_preserved".to_string(),
                severity: if unresolved { "ambiguous" } else { "decided" }.to_string(),
                subject: set.subject.clone(),
                message: if unresolved {
                    "边界点存在多个合法成员候选，全部保留等待裁定".to_string()
                } else {
                    "边界点候选已裁定".to_string()
                },
                at_seq: set.decided_by_event.as_ref().and_then(|_| {
                    events.iter().map(|e| e.seq).max()
                }),
                evidence: serde_json::to_value(set).unwrap_or(serde_json::json!({})),
            });
        }
    }

    for p in pops {
        if p.stale {
            out.push(Judgment {
                rule_id: "lineage.upstream_revision_marks_stale".to_string(),
                severity: "stale".to_string(),
                subject: format!("pop:{}", p.pop_id),
                message: "上游门/输入版本已修订：该下游结果标记过期但保留，未被覆盖".to_string(),
                at_seq: Some(p.created_seq),
                evidence: serde_json::json!({
                    "pop_id": p.pop_id,
                    "gate_id": p.gate_id,
                    "stale_reasons": p.stale_reasons,
                }),
            });
        }
        if p.percent.is_some() {
            out.push(Judgment {
                rule_id: "pct.denominator_pinned_to_parent_version".to_string(),
                severity: "ok".to_string(),
                subject: format!("pop:{}", p.pop_id),
                message: "百分比分母钉住父群版本快照".to_string(),
                at_seq: Some(p.created_seq),
                evidence: serde_json::json!({
                    "pop_id": p.pop_id,
                    "parent_pop_id": p.parent_pop_id,
                    "parent_gate_version_id": p.parent_gate_version_id,
                    "denominator": p.denominator,
                    "numerator": p.numerator,
                }),
            });
        }
    }

    out
}

/// 审阅页投影：门树 + 散点 + 补偿前后对照 + 分母来源 + 候选集。
pub fn project_view(replay: &Replay) -> serde_json::Value {
    let state = &replay.state;
    let relations = state
        .relations_head
        .as_ref()
        .and_then(|v| state.relations.get(v))
        .cloned()
        .unwrap_or(GateRelationVersion {
            version_id: String::new(),
            supersedes: None,
            edges: Vec::new(),
            note: String::new(),
        });
    let parent_of: BTreeMap<String, String> = relations.edges.iter().cloned().collect();

    let mut gate_tree = Vec::new();
    for (gate_id, head_v) in &state.gate_heads {
        let gate = state.gates.get(gate_id).and_then(|m| m.get(head_v));
        if let Some(g) = gate {
            // 该门当前有效的最新派生（未过期优先）。
            let live = replay
                .pops
                .iter()
                .filter(|p| &p.gate_id == gate_id && !p.stale)
                .max_by_key(|p| p.created_seq)
                .cloned();
            let any_latest = replay
                .pops
                .iter()
                .filter(|p| &p.gate_id == gate_id)
                .max_by_key(|p| p.created_seq)
                .cloned();
            let points = gate_scatter_points(state, g);
            gate_tree.push(serde_json::json!({
                "gate_id": gate_id,
                "gate_version_id": head_v,
                "name": g.name,
                "kind": g.kind,
                "space": g.space,
                "boundary_policy": g.boundary,
                "polygon": g.polygon,
                "parent_gate_id": parent_of.get(gate_id),
                "live_pop": live,
                "latest_pop": any_latest,
                "points": points,
            }));
        }
    }

    let compensation_compare = compensation_compare_rows(state);

    serde_json::json!({
        "seq": replay.last_seq,
        "heads": {
            "dataset": state.dataset_head,
            "spillover_meta": state.spillover_head,
            "compensation": state.compensation_head,
            "transform": state.transform_head,
            "relations": state.relations_head,
            "gates": state.gate_heads,
        },
        "gate_tree": gate_tree,
        "compensation_compare": compensation_compare,
        "pops": replay.pops,
        "candidate_sets": state.candidate_sets.values().collect::<Vec<_>>(),
        "judgments": replay.judgments,
        "retracted_events": state.retracted.iter().collect::<Vec<_>>(),
    })
}

/// 为审阅散点收集某门坐标空间中的事件点（补偿前后由门空间基准体现）。
pub fn gate_scatter_points(state: &State, gate: &GateVersion) -> Vec<serde_json::Value> {
    let mut cache: BTreeMap<String, Result<MatrixInverse, String>> = BTreeMap::new();
    if let Some(cv) = &gate.space.compensation_version {
        if let Some(comp) = state.compensations.get(cv) {
            cache.insert(cv.clone(), invert_matrix(&comp.matrix));
            let cs_id = stable_id("CS-COMP", cv);
            let accepted_reg = state
                .candidate_sets
                .get(&cs_id)
                .and_then(|s| s.accepted.clone())
                .flatten()
                .is_some_and(|a| a.ends_with(":regularize"));
            if accepted_reg {
                cache.insert(
                    format!("{cv}#regularized"),
                    Ok(regularized_inverse(&comp.matrix, RIDGE)),
                );
            }
        }
    }
    let key = if cache.contains_key(&format!(
        "{}#regularized",
        gate.space.compensation_version.clone().unwrap_or_default()
    )) {
        gate.space.compensation_version.as_ref().map(|v| format!("{v}#regularized"))
    } else {
        gate.space.compensation_version.clone()
    };
    let cofactor = gate
        .space
        .transform_version
        .as_ref()
        .and_then(|tv| state.transforms.get(tv))
        .map(|t| t.cofactor);
    evaluate_gate(state, &cache, key.as_deref(), gate)
        .into_iter()
        .map(|p| {
            let (mut x, mut y) = (p.x, p.y);
            if gate.space.basis == "asinh-compensated" {
                let c = cofactor.unwrap_or(150.0);
                x = x.map(|v| asinh_transform(v, c));
                y = y.map(|v| asinh_transform(v, c));
            }
            serde_json::json!({
                "event_id": p.event_id,
                "x": x, "y": y,
                "raw_x": p.raw_x,
                "raw_y": p.raw_y,
                "diagnostics": p.diagnostics,
            })
        })
        .collect()
}

/// 每个荧光事件在当前头部补偿矩阵下的补偿前/后差异（带单位）。
fn compensation_compare_rows(state: &State) -> Vec<serde_json::Value> {
    let comp_vid = match &state.compensation_head {
        Some(v) => v.clone(),
        None => return Vec::new(),
    };
    let comp = match state.compensations.get(&comp_vid) {
        Some(c) => c.clone(),
        None => return Vec::new(),
    };
    let ds_vid = match &state.dataset_head {
        Some(v) => v.clone(),
        None => return Vec::new(),
    };
    let dataset = match state.datasets.get(&ds_vid) {
        Some(d) => d,
        None => return Vec::new(),
    };
    let inv = match invert_matrix(&comp.matrix) {
        Ok(i) => i,
        Err(msg) => {
            return vec![serde_json::json!({
                "compensation_version": comp_vid,
                "invertible": false,
                "reason": msg,
            })];
        }
    };
    let mut rows = Vec::new();
    for e in &dataset.events {
        let mut row = serde_json::json!({
            "event_id": e.event_id,
            "dataset_version": ds_vid,
            "compensation_version": comp_vid,
            "unit_raw": "ADC-count",
            "unit_comp": "ADC-count(compensated-linear)",
        });
        let mut channels = serde_json::json!({});
        for (idx, ch) in comp.channels.iter().enumerate() {
            let raw = e.values.iter().find(|v| &v.channel == ch).and_then(|v| v.numeric);
            let mut compv: Option<f64> = None;
            if raw.is_some() {
                let mut v = 0.0;
                for (k, ch_k) in comp.channels.iter().enumerate() {
                    let rv = e
                        .values
                        .iter()
                        .find(|x| &x.channel == ch_k)
                        .and_then(|x| x.numeric)
                        .unwrap_or(0.0);
                    v += inv.inverse[idx][k] * rv;
                }
                compv = Some(v);
            }
            channels[ch] = serde_json::json!({
                "raw": raw,
                "compensated": compv,
                "delta": raw.and_then(|r| compv.map(|c| c - r)),
            });
        }
        row["channels"] = channels;
        rows.push(row);
    }
    rows
}
