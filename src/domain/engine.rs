//! 事件溯源折叠引擎：把一条分支可见的事件流折叠成当前谱系状态。
//! 纯函数、确定性；任何 seq 上的可见结论都可重放恢复。

use super::math::{
    apply_compensation, assess_matrix, point_in_polygon, MatrixVerdict, Point, TransformSpec,
};
use super::model::{DecisionOutcome, DomainEvent, Envelope, SourceRole, ALGORITHM_VERSION};
use crate::fcs::FcsFile;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Serialize)]
pub struct SourceEntry {
    pub source_id: String,
    pub role: SourceRole,
    pub filename: String,
    pub sha256: String,
    pub byte_len: usize,
    pub event_count: usize,
    pub channel_names: Vec<String>,
    pub diagnostic_count: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct SpilloverEntry {
    pub id: String,
    pub revision: u32,
    pub channel_order: Vec<String>,
    pub matrix: Vec<Vec<f64>>,
    pub unit: String,
    pub basis: String,
    pub derived_from_sources: Vec<String>,
    pub verdict: Option<MatrixVerdict>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GateEntry {
    pub gate_id: String,
    pub revision: u32,
    pub name: String,
    pub x_channel: String,
    pub y_channel: String,
    pub coordinate_basis: String,
    pub polygon: Vec<Point>,
    pub parent: Option<String>,
    /// 历次修订（保留旧几何身份，支持时间旅行）
    pub revisions: Vec<u32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DecisionEntry {
    pub decision_id: String,
    pub spillover_id: String,
    pub outcome: DecisionOutcome,
    pub candidate_id: Option<String>,
    pub rationale: String,
    pub reverted: bool,
    pub at_seq: u64,
    pub event_id: String,
}

/// 门群结果的“分母钉版”引用。
#[derive(Debug, Clone, Serialize)]
pub struct DenominatorRef {
    pub parent_gate: Option<String>,
    pub parent_pop_result_id: Option<String>,
    pub parent_count: usize,
    pub parent_revision_sig: String,
    pub root_source_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PopulationResult {
    pub pop_result_id: String,
    pub gate_id: String,
    pub gate_revision: u32,
    pub gate_name: String,
    pub parent_gate: Option<String>,
    /// 计算时钉住的完整依赖版本签名
    pub dependency_sig: String,
    pub spillover_revision: Option<u32>,
    pub transform_sig: String,
    pub denominator: DenominatorRef,
    pub count: usize,
    pub percent_of_parent: f64,
    pub percent_of_root: f64,
    pub root_count: usize,
    /// fresh / stale_pending_revision
    pub status: String,
    pub stale_reason: Option<String>,
    pub stale_since_seq: Option<u64>,
    pub computed_at_seq: u64,
    /// 门内/门外事件下标（在样本源中的位置）
    pub member_indices: Vec<usize>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CompensationView {
    pub spillover_id: String,
    pub revision: u32,
    pub channel_order: Vec<String>,
    pub verdict: MatrixVerdict,
    /// 当前生效决策（最近一次未撤销）
    pub active_decision: Option<DecisionEntry>,
    /// 全部历史决策（含被撤销/被否定），同时保留采纳与未采纳结果
    pub all_decisions: Vec<DecisionEntry>,
    pub effective_inverse: Option<Vec<Vec<f64>>>,
    pub blocked: bool,
    pub blocked_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ViewState {
    pub project_id: String,
    pub title: String,
    pub branch: String,
    pub head_seq: u64,
    pub algorithm_version: String,
    pub sources: BTreeMap<String, SourceEntry>,
    pub sample_source: Option<String>,
    pub spillovers: BTreeMap<String, SpilloverEntry>,
    pub transforms: BTreeMap<String, (u32, TransformSpec)>,
    pub gates: BTreeMap<String, GateEntry>,
    pub tree_revision: u32,
    pub decisions: Vec<DecisionEntry>,
    pub compensation: Option<CompensationView>,
    pub populations: BTreeMap<String, PopulationResult>,
    pub conflicts: Vec<ConflictView>,
    pub forks: Vec<ForkView>,
    pub reverted_event_ids: BTreeSet<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConflictView {
    pub conflict_id: String,
    pub branch: String,
    pub expected_head: u64,
    pub actual_head: u64,
    pub attempted_action: String,
    pub left_draft: String,
    pub right_draft: String,
    pub diff: Vec<String>,
    pub at_seq: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ForkView {
    pub new_branch: String,
    pub display_name: String,
    pub forked_from_seq: u64,
    pub at_seq: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScatterPoint {
    pub event_index: usize,
    pub x: f64,
    pub y: f64,
    pub x_raw: f64,
    pub y_raw: f64,
    pub x_ok: bool,
    pub y_ok: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScatterData {
    pub x_channel: String,
    pub y_channel: String,
    pub x_basis: String,
    pub y_basis: String,
    pub points: Vec<ScatterPoint>,
    pub untransformable: usize,
    pub source_id: String,
}

#[derive(Default, Clone)]
struct FoldState {
    project_id: Option<String>,
    title: String,
    sources: BTreeMap<String, (SourceEntry, FcsFile)>,
    sample_source: Option<String>,
    spillovers: BTreeMap<String, SpilloverEntry>,
    transforms: BTreeMap<String, (u32, TransformSpec)>,
    gate_history: BTreeMap<String, BTreeMap<u32, GateEntry>>,
    gate_current: BTreeMap<String, u32>,
    gate_names: BTreeMap<String, String>,
    tree_edges: Vec<(String, Option<String>)>,
    tree_revision: u32,
    decisions: Vec<DecisionEntry>,
    reverted_event_ids: BTreeSet<String>,
    conflicts: Vec<ConflictView>,
    forks: Vec<ForkView>,
    head_seq: u64,
    /// (gate_id, gate_revision, dependency_sig) -> 旧结论；修订上游不覆盖
    pop_cache: BTreeMap<(String, u32, String), CachedPop>,
    /// gate_id -> 自哪个 seq 起下游待复核
    stale_since: BTreeMap<String, u64>,
}

#[derive(Clone)]
struct CachedPop {
    count: usize,
    members: Vec<usize>,
    computed_at_seq: u64,
    denom_sig: String,
    denom_parent_result: Option<String>,
    denom_count: usize,
}

pub fn fold(envelopes: &[Envelope], branch: &str, head_seq: Option<u64>) -> ViewState {
    let upto = head_seq.unwrap_or(u64::MAX);
    let visible = visible_envelopes(envelopes, branch, upto);
    let mut st = FoldState::default();
    st.head_seq = visible.last().map(|e| e.seq).unwrap_or(0);

    for env in &visible {
        apply_event(&mut st, env, branch);
    }
    build_view(st, branch)
}

/// 计算某分支可见的事件：自身分支全部 + 沿 fork 链继承的祖先事件（截止 fork 点）。
fn visible_envelopes<'a>(
    envelopes: &'a [Envelope],
    branch: &str,
    upto: u64,
) -> Vec<&'a Envelope> {
    let mut chain: Vec<(String, Option<u64>)> = vec![(branch.to_string(), None)];
    let mut cursor = branch.to_string();
    loop {
        let fork = envelopes.iter().find(|e| {
            e.seq <= upto
                && matches!(&e.payload, DomainEvent::ForkCreated { new_branch, .. } if new_branch == &cursor)
        });
        let Some(fe) = fork else { break };
        let DomainEvent::ForkCreated { forked_from_seq, .. } = &fe.payload else {
            break;
        };
        let parent = fe.branch.clone();
        chain.push((parent.clone(), Some(*forked_from_seq)));
        cursor = parent;
    }
    let mut out: Vec<&Envelope> = Vec::new();
    for env in envelopes {
        if env.seq > upto {
            continue;
        }
        for (b, upper) in &chain {
            if env.branch == *b && upper.map_or(true, |u| env.seq <= u) {
                out.push(env);
                break;
            }
        }
    }
    out.sort_by_key(|e| e.seq);
    out
}

fn apply_event(st: &mut FoldState, env: &Envelope, view_branch: &str) {
    match &env.payload {
        DomainEvent::ProjectCreated { project_id, title } => {
            st.project_id = Some(project_id.clone());
            st.title = title.clone();
        }
        DomainEvent::FcsImported {
            source_id,
            role,
            revision: _,
            replaces,
            filename,
            sha256,
            byte_len,
            fcs,
            affected_objects: _,
        } => {
            if *role == SourceRole::Sample {
                // 样本源被替换：切换当前样本，旧源仍保留在 sources 中可被时间旅行读取
                st.sample_source = Some(source_id.clone());
            }
            let _ = replaces;
            st.sources.insert(
                source_id.clone(),
                (
                    SourceEntry {
                        source_id: source_id.clone(),
                        role: *role,
                        filename: filename.clone(),
                        sha256: sha256.clone(),
                        byte_len: *byte_len,
                        event_count: fcs.events.len(),
                        channel_names: fcs.channels.iter().map(|c| c.name.clone()).collect(),
                        diagnostic_count: fcs.diagnostics.len(),
                    },
                    fcs.clone(),
                ),
            );
        }
        DomainEvent::SpilloverRecorded {
            spillover_id,
            revision,
            channel_order,
            matrix,
            unit,
            basis,
            derived_from_sources,
        } => {
            let verdict = Some(assess_matrix(matrix));
            st.spillovers.insert(
                spillover_id.clone(),
                SpilloverEntry {
                    id: spillover_id.clone(),
                    revision: *revision,
                    channel_order: channel_order.clone(),
                    matrix: matrix.clone(),
                    unit: unit.clone(),
                    basis: basis.clone(),
                    derived_from_sources: derived_from_sources.clone(),
                    verdict,
                },
            );
        }
        DomainEvent::TransformSet {
            transform_id,
            revision,
            channel: _,
            spec,
        } => {
            st.transforms
                .insert(transform_id.clone(), (*revision, spec.clone()));
        }
        DomainEvent::GateGeometrySaved {
            gate_id,
            revision,
            name,
            x_channel,
            y_channel,
            coordinate_basis,
            polygon,
        } => {
            st.gate_names.insert(gate_id.clone(), name.clone());
            let parent = st
                .tree_edges
                .iter()
                .find_map(|(g, p)| (g == gate_id).then_some(p.clone()).flatten());
            let mut revs: BTreeSet<u32> = st
                .gate_history
                .get(gate_id)
                .map(|m| m.keys().copied().collect())
                .unwrap_or_default();
            revs.insert(*revision);
            let entry = GateEntry {
                gate_id: gate_id.clone(),
                revision: *revision,
                name: name.clone(),
                x_channel: x_channel.clone(),
                y_channel: y_channel.clone(),
                coordinate_basis: coordinate_basis.clone(),
                polygon: polygon.clone(),
                parent,
                revisions: revs.into_iter().collect(),
            };
            st.gate_history
                .entry(gate_id.clone())
                .or_default()
                .insert(*revision, entry);
            st.gate_current.insert(gate_id.clone(), *revision);
        }
        DomainEvent::GateRenamed { gate_id, new_name } => {
            st.gate_names.insert(gate_id.clone(), new_name.clone());
        }
        DomainEvent::GateTreeSaved {
            tree_id: _,
            revision,
            edges,
        } => {
            st.tree_edges = edges.clone();
            st.tree_revision = *revision;
            for (g, p) in edges {
                if let Some(hist) = st.gate_history.get_mut(g) {
                    for e in hist.values_mut() {
                        e.parent = p.clone();
                    }
                }
            }
        }
        DomainEvent::CompensationAssessed {
            spillover_id,
            verdict,
            affected_objects: _,
        } => {
            if let Some(entry) = st.spillovers.get_mut(spillover_id) {
                entry.verdict = Some(verdict.clone());
            }
        }
        DomainEvent::CompensationDecided {
            decision_id,
            spillover_id,
            outcome,
            candidate_id,
            rationale,
        } => {
            st.decisions.push(DecisionEntry {
                decision_id: decision_id.clone(),
                spillover_id: spillover_id.clone(),
                outcome: *outcome,
                candidate_id: candidate_id.clone(),
                rationale: rationale.clone(),
                reverted: false,
                at_seq: env.seq,
                event_id: env.event_id.clone(),
            });
        }
        DomainEvent::DecisionReverted {
            reverted_event_id, ..
        } => {
            st.reverted_event_ids.insert(reverted_event_id.clone());
            for d in st.decisions.iter_mut() {
                if d.event_id == *reverted_event_id {
                    d.reverted = true;
                }
            }
        }
        DomainEvent::VersionConflictRecorded {
            conflict_id,
            branch: b,
            expected_head,
            actual_head,
            attempted_action,
            left_draft,
            right_draft,
            diff,
        } => {
            st.conflicts.push(ConflictView {
                conflict_id: conflict_id.clone(),
                branch: b.clone(),
                expected_head: *expected_head,
                actual_head: *actual_head,
                attempted_action: attempted_action.clone(),
                left_draft: left_draft.clone(),
                right_draft: right_draft.clone(),
                diff: diff.clone(),
                at_seq: env.seq,
            });
        }
        DomainEvent::GateRevised {
            gate_id,
            affected_objects,
            ..
        } => {
            for obj in affected_objects {
                if let Some(g) = obj.strip_prefix("gate:") {
                    st.stale_since.entry(g.to_string()).or_insert(env.seq);
                }
            }
            st.stale_since.entry(gate_id.clone()).or_insert(env.seq);
        }
        DomainEvent::ForkCreated {
            new_branch,
            display_name,
            forked_from_seq,
        } => {
            let _ = view_branch;
            st.forks.push(ForkView {
                new_branch: new_branch.clone(),
                display_name: display_name.clone(),
                forked_from_seq: *forked_from_seq,
                at_seq: env.seq,
            });
        }
    }
}

fn build_view(st: FoldState, branch: &str) -> ViewState {
    let FoldState {
        project_id,
        title,
        sources,
        sample_source,
        spillovers,
        transforms,
        gate_history,
        gate_current,
        gate_names,
        tree_edges,
        tree_revision,
        decisions,
        reverted_event_ids,
        conflicts,
        forks,
        head_seq,
        mut pop_cache,
        stale_since,
    } = st;

    // 当前门集合（最新几何），保留修订历史
    let mut gates: BTreeMap<String, GateEntry> = BTreeMap::new();
    for (gid, rev) in &gate_current {
        if let Some(hist) = gate_history.get(gid) {
            if let Some(e) = hist.get(rev) {
                let mut entry = e.clone();
                entry.name = gate_names.get(gid).cloned().unwrap_or(entry.name);
                entry.parent = tree_edges
                    .iter()
                    .find_map(|(g, p)| (g == gid).then_some(p.clone()).flatten());
                entry.revisions = hist.keys().copied().collect();
                gates.insert(gid.clone(), entry);
            }
        }
    }

    // 补偿视图：取最近记录的 spillover（本案例只有一个 id）
    let compensation = spillovers.values().last().map(|sp| {
        let all_decisions: Vec<DecisionEntry> = decisions
            .iter()
            .filter(|d| d.spillover_id == sp.id)
            .cloned()
            .collect();
        let active = all_decisions
            .iter()
            .filter(|d| !d.reverted)
            .last()
            .cloned();
        let verdict = sp
            .verdict
            .clone()
            .expect("spillover 记录时一定已评估");
        let mut blocked = false;
        let mut blocked_reason = None;
        let mut effective = None;
        match &active {
            None => {
                if verdict.singular {
                    blocked = true;
                    blocked_reason = Some("矩阵精确奇异：无候选可补偿，等待重新采集/人工决定".into());
                } else if verdict.near_singular {
                    blocked = true;
                    blocked_reason = Some(
                        "近奇异矩阵未裁定：保留全部候选，未接受前不能进入下游".into(),
                    );
                } else {
                    effective = verdict.inverse.clone();
                }
            }
            Some(d) => match d.outcome {
                DecisionOutcome::AcceptedAuto => effective = verdict.inverse.clone(),
                DecisionOutcome::AcceptedCandidate => {
                    if let Some(cid) = &d.candidate_id {
                        if let Some(c) = verdict.candidates.iter().find(|c| &c.candidate_id == cid)
                        {
                            effective = Some(c.inverse.clone());
                        }
                    }
                    if effective.is_none() {
                        blocked = true;
                        blocked_reason = Some("裁定引用的候选已不存在".into());
                    }
                }
                DecisionOutcome::Rejected => {
                    blocked = true;
                    blocked_reason =
                        Some("补偿矩阵被否定：必须提供新矩阵后才能继续（旧结论保留）".into());
                }
            },
        }
        CompensationView {
            spillover_id: sp.id.clone(),
            revision: sp.revision,
            channel_order: sp.channel_order.clone(),
            verdict,
            active_decision: active,
            all_decisions,
            effective_inverse: effective,
            blocked,
            blocked_reason,
        }
    });

    // 群评估（依赖排序：父先于子）
    let mut populations: BTreeMap<String, PopulationResult> = BTreeMap::new();
    let mut evaluated = 0u32;
    while evaluated < gates.len() as u32 {
        let before = populations.len();
        for (gid, gate) in gates.iter() {
            if populations.contains_key(gid) {
                continue;
            }
            match &gate.parent {
                None => {}
                Some(p) => {
                    if !populations.contains_key(p) && gates.contains_key(p) {
                        continue;
                    }
                }
            }
            if let Some(sample_id) = &sample_source {
                let fcs = &sources.get(sample_id).expect("sample 存在").1;
                let result = evaluate_gate(
                    gid,
                    gate,
                    &populations,
                    &compensation,
                    &transforms,
                    fcs,
                    sample_id,
                    head_seq,
                    &mut pop_cache,
                    &stale_since,
                );
                populations.insert(gid.clone(), result);
            }
        }
        if populations.len() == before {
            break; // 父门缺失（孤立门），结束
        }
        evaluated += 1;
    }

    ViewState {
        project_id: project_id.unwrap_or_default(),
        title,
        branch: branch.to_string(),
        head_seq,
        algorithm_version: ALGORITHM_VERSION.into(),
        sources: sources
            .into_iter()
            .map(|(k, (e, _))| (k, e))
            .collect(),
        sample_source,
        spillovers,
        transforms,
        gates,
        tree_revision,
        decisions,
        compensation,
        populations,
        conflicts,
        forks,
        reverted_event_ids,
    }
}

#[allow(clippy::too_many_arguments)]
fn evaluate_gate(
    gid: &str,
    gate: &GateEntry,
    populations: &BTreeMap<String, PopulationResult>,
    compensation: &Option<CompensationView>,
    transforms: &BTreeMap<String, (u32, TransformSpec)>,
    fcs: &FcsFile,
    sample_id: &str,
    head_seq: u64,
    cache: &mut BTreeMap<(String, u32, String), CachedPop>,
    stale_since: &BTreeMap<String, u64>,
) -> PopulationResult {
    let xi = fcs
        .channels
        .iter()
        .position(|c| c.name == gate.x_channel);
    let yi = fcs
        .channels
        .iter()
        .position(|c| c.name == gate.y_channel);

    // 依赖版本签名：spillover 修订 + 生效补偿决策 + 两个通道的变换修订 + 门几何修订
    let transform_rev = |ch: &str| {
        transforms
            .iter()
            .find_map(|(tid, (r, spec))| {
                let id_ch = tid.strip_prefix("transform-").unwrap_or(tid);
                (id_ch == ch).then_some((*r, spec.clone()))
            })
    };
    let tx = transform_rev(&gate.x_channel);
    let ty = transform_rev(&gate.y_channel);
    let tx_sig = tx
        .as_ref()
        .map(|(r, s)| format!("{}:{}:{}", gate.x_channel, r, s.summary()))
        .unwrap_or_else(|| format!("{}:0:identity", gate.x_channel));
    let ty_sig = ty
        .as_ref()
        .map(|(r, s)| format!("{}:{}:{}", gate.y_channel, r, s.summary()))
        .unwrap_or_else(|| format!("{}:0:identity", gate.y_channel));

    let comp_sig = compensation.as_ref().map(|c| {
        let dec = c
            .active_decision
            .as_ref()
            .map(|d| format!("{}/{}", d.decision_id, d.event_id))
            .unwrap_or_else(|| "auto".into());
        format!("spill{}:{}:{}", c.revision, c.verdict.determinant, dec)
    });
    let spillover_revision = compensation.as_ref().map(|c| c.revision);
    let dependency_sig = format!(
        "alg={}|src={}|{}|{}|{}|gate-{}@{}",
        ALGORITHM_VERSION,
        sample_id,
        comp_sig.clone().unwrap_or_else(|| "no-comp".into()),
        tx_sig,
        ty_sig,
        gid,
        gate.revision
    );

    // 分母钉版：父群必须是计算时那个版本的结论
    let (parent_members, denom_parent_result, denom_sig, denom_count) = match &gate.parent {
        None => ((0..fcs.events.len()).collect::<Vec<_>>(), None, "root".to_string(), fcs.events.len()),
        Some(pid) => {
            if let Some(parent) = populations.get(pid) {
                (
                    parent.member_indices.clone(),
                    Some(parent.pop_result_id.clone()),
                    parent.dependency_sig.clone(),
                    parent.count,
                )
            } else {
                (vec![], None, "missing-parent".into(), 0)
            }
        }
    };

    let key = (gid.to_string(), gate.revision, dependency_sig.clone());
    let cached = cache.get(&key).cloned();
    let (members, computed_at_seq) = if let Some(c) = cached {
        (c.members, c.computed_at_seq)
    } else if xi.is_none() || yi.is_none() || compensation.as_ref().map(|c| c.blocked).unwrap_or(false) {
        // 通道缺失或补偿被阻断：无法唯一计算，返回空成员但签名保留
        (vec![], head_seq)
    } else {
        let (xi, yi) = (xi.unwrap(), yi.unwrap());
        let inv = compensation
            .as_ref()
            .and_then(|c| c.effective_inverse.clone());
        let comp_order = compensation.as_ref().map(|c| c.channel_order.clone());
        let mut members = Vec::new();
        for idx in &parent_members {
            let raw = &fcs.events[*idx];
            let values = match (&inv, &comp_order) {
                (Some(inv), Some(order)) => {
                    // 补偿严格按 spillover 的通道顺序组装向量，再映射回 FCS 通道
                    let ordered: Vec<f64> = order
                        .iter()
                        .map(|ch| {
                            fcs.channels
                                .iter()
                                .position(|c| &c.name == ch)
                                .map(|ci| raw[ci])
                                .unwrap_or(f64::NAN)
                        })
                        .collect();
                    let comp_ordered = apply_compensation(&ordered, inv);
                    let mut mapped = raw.clone();
                    for (ch, v) in order.iter().zip(comp_ordered) {
                        if let Some(ci) = fcs.channels.iter().position(|c| &c.name == ch) {
                            mapped[ci] = v;
                        }
                    }
                    mapped
                }
                _ => raw.clone(),
            };
            let (vx, vx_ok) = tx
                .as_ref()
                .map(|(_, s)| s.apply(values[xi]).map(|v| (v, true)).unwrap_or((values[xi], false)))
                .unwrap_or((values[xi], values[xi].is_finite()));
            let (vy, vy_ok) = ty
                .as_ref()
                .map(|(_, s)| s.apply(values[yi]).map(|v| (v, true)).unwrap_or((values[yi], false)))
                .unwrap_or((values[yi], values[yi].is_finite()));
            if vx_ok && vy_ok && vx.is_finite() && vy.is_finite()
                && point_in_polygon(Point { x: vx, y: vy }, &gate.polygon)
            {
                members.push(*idx);
            }
        }
        cache.insert(
            key.clone(),
            CachedPop {
                count: members.len(),
                members: members.clone(),
                computed_at_seq: head_seq,
                denom_sig: denom_sig.clone(),
                denom_parent_result: denom_parent_result.clone(),
                denom_count,
            },
        );
        (members, head_seq)
    };

    let count = members.len();
    let root_count = match &gate.parent {
        None => fcs.events.len(),
        Some(pid) => populations
            .get(pid)
            .map(|p| p.root_count)
            .unwrap_or(fcs.events.len()),
    };
    let percent_of_parent = if denom_count > 0 {
        100.0 * count as f64 / denom_count as f64
    } else {
        0.0
    };
    let percent_of_root = if root_count > 0 {
        100.0 * count as f64 / root_count as f64
    } else {
        0.0
    };
    let pop_result_id = format!("pop-{gid}-r{}", &hash8(&dependency_sig)[..8]);

    // 失效标记：上游门自上次计算后修订过（只标记，不覆盖旧结论——旧结论仍在本 fold 的缓存身份中）
    let mut status = "fresh".to_string();
    let mut stale_reason = None;
    let mut stale_since_seq = None;
    if let Some(pid) = &gate.parent {
        if let Some(s) = stale_since.get(pid) {
            status = "stale_pending_revision".into();
            stale_reason = Some(format!("上游门 {pid} 已修订，下游百分比待按新版父群重锚"));
            stale_since_seq = Some(*s);
        }
    }
    if let Some(s) = stale_since.get(gid) {
        if gate.parent.is_some() {
            // 自身修订不影响自己 fresh，但保留修订时间
        }
        stale_since_seq.get_or_insert(*s);
    }

    PopulationResult {
        pop_result_id,
        gate_id: gid.to_string(),
        gate_revision: gate.revision,
        gate_name: gate.name.clone(),
        parent_gate: gate.parent.clone(),
        dependency_sig,
        spillover_revision,
        transform_sig: format!("{tx_sig}|{ty_sig}"),
        denominator: DenominatorRef {
            parent_gate: gate.parent.clone(),
            parent_pop_result_id: denom_parent_result,
            parent_count: denom_count,
            parent_revision_sig: denom_sig,
            root_source_id: sample_id.to_string(),
        },
        count,
        percent_of_parent,
        percent_of_root,
        root_count,
        status,
        stale_reason,
        stale_since_seq,
        computed_at_seq,
        member_indices: members,
    }
}

fn hash8(s: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    format!("{:x}", h.finalize())
}

/// 构造审阅页散点：显示原始与（补偿+变换后的）派生坐标；无法转换的点保留在原位。
pub fn build_scatter(view: &ViewState, fcs: &FcsFile, x_channel: &str, y_channel: &str) -> ScatterData {
    let xi = fcs.channels.iter().position(|c| c.name == x_channel);
    let yi = fcs.channels.iter().position(|c| c.name == y_channel);
    let spec_for = |ch: &str| {
        view.transforms.iter().find_map(|(tid, (r, s))| {
            let id_ch = tid.strip_prefix("transform-").unwrap_or(tid);
            (id_ch == ch).then_some((*r, s.clone()))
        })
    };
    let tx = spec_for(x_channel);
    let ty = spec_for(y_channel);
    let (x_basis, y_basis) = (
        tx.as_ref().map(|s| s.1.basis().to_string()).unwrap_or("raw detector unit".into()),
        ty.as_ref().map(|s| s.1.basis().to_string()).unwrap_or("raw detector unit".into()),
    );
    let mut points = Vec::with_capacity(fcs.events.len());
    let mut untransformable = 0usize;
    if let (Some(xi), Some(yi)) = (xi, yi) {
        let inv = view
            .compensation
            .as_ref()
            .filter(|c| !c.blocked)
            .and_then(|c| c.effective_inverse.clone());
        let order = view.compensation.as_ref().map(|c| c.channel_order.clone());
        for (i, raw) in fcs.events.iter().enumerate() {
            let values = match (&inv, &order) {
                (Some(inv), Some(ord)) => {
                    let ordered: Vec<f64> = ord
                        .iter()
                        .map(|ch| {
                            fcs.channels
                                .iter()
                                .position(|c| &c.name == ch)
                                .map(|ci| raw[ci])
                                .unwrap_or(f64::NAN)
                        })
                        .collect();
                    let co = apply_compensation(&ordered, inv);
                    let mut mapped = raw.clone();
                    for (ch, v) in ord.iter().zip(co) {
                        if let Some(ci) = fcs.channels.iter().position(|c| &c.name == ch) {
                            mapped[ci] = v;
                        }
                    }
                    mapped
                }
                _ => raw.clone(),
            };
            let (x_raw, y_raw) = (values[xi], values[yi]);
            let (x, x_ok) = tx
                .as_ref()
                .map(|(_, s)| s.apply(x_raw))
                .map(|r| match r {
                    Ok(v) => (v, true),
                    Err(_) => (x_raw, false),
                })
                .unwrap_or((x_raw, x_raw.is_finite()));
            let (y, y_ok) = ty
                .as_ref()
                .map(|(_, s)| s.apply(y_raw))
                .map(|r| match r {
                    Ok(v) => (v, true),
                    Err(_) => (y_raw, false),
                })
                .unwrap_or((y_raw, y_raw.is_finite()));
            if !(x_ok && y_ok) {
                untransformable += 1;
            }
            points.push(ScatterPoint {
                event_index: i,
                x,
                y,
                x_raw,
                y_raw,
                x_ok,
                y_ok,
            });
        }
    }
    ScatterData {
        x_channel: x_channel.into(),
        y_channel: y_channel.into(),
        x_basis,
        y_basis,
        points,
        untransformable,
        source_id: view.sample_source.clone().unwrap_or_default(),
    }
}

/// 并排两个草案的显示值差异（每个值都有稳定身份）。
pub fn diff_views(left: &ViewState, right: &ViewState) -> Vec<DiffRow> {
    let mut rows = vec![DiffRow {
        identity: "view:head_seq".into(),
        label: "分支头版本".into(),
        left: left.head_seq.to_string(),
        right: right.head_seq.to_string(),
        differs: left.head_seq != right.head_seq,
    }];
    if let (Some(lc), Some(rc)) = (&left.compensation, &right.compensation) {
        rows.push(DiffRow {
            identity: format!("compensation:{}", lc.spillover_id),
            label: "补偿状态".into(),
            left: comp_status(lc),
            right: comp_status(rc),
            differs: comp_status(lc) != comp_status(rc),
        });
    }
    let mut gates: BTreeSet<&str> = left.gates.keys().map(|s| s.as_str()).collect();
    gates.extend(right.gates.keys().map(|s| s.as_str()));
    for g in gates {
        let lp = left.populations.get(g);
        let rp = right.populations.get(g);
        rows.push(DiffRow {
            identity: format!("population:{g}:count"),
            label: format!("{} 细胞数", g),
            left: lp.map(|p| p.count.to_string()).unwrap_or_else(|| "—".into()),
            right: rp.map(|p| p.count.to_string()).unwrap_or_else(|| "—".into()),
            differs: lp.map(|p| p.count) != rp.map(|p| p.count),
        });
        rows.push(DiffRow {
            identity: format!("population:{g}:percent_of_parent"),
            label: format!("{} 占父群%", g),
            left: lp.map(|p| format!("{:.4}", p.percent_of_parent)).unwrap_or_else(|| "—".into()),
            right: rp.map(|p| format!("{:.4}", p.percent_of_parent)).unwrap_or_else(|| "—".into()),
            differs: lp.map(|p| (p.percent_of_parent * 1e4) as i64)
                != rp.map(|p| (p.percent_of_parent * 1e4) as i64),
        });
        rows.push(DiffRow {
            identity: format!("population:{g}:status"),
            label: format!("{} 状态", g),
            left: lp.map(|p| p.status.clone()).unwrap_or_else(|| "—".into()),
            right: rp.map(|p| p.status.clone()).unwrap_or_else(|| "—".into()),
            differs: lp.map(|p| &p.status) != rp.map(|p| &p.status),
        });
    }
    rows
}

fn comp_status(c: &CompensationView) -> String {
    if c.blocked {
        format!("blocked: {}", c.blocked_reason.clone().unwrap_or_default())
    } else {
        format!(
            "active:{}",
            c.active_decision
                .as_ref()
                .map(|d| d.candidate_id.clone().unwrap_or_else(|| "auto".into()))
                .unwrap_or_else(|| "auto".into())
        )
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DiffRow {
    pub identity: String,
    pub label: String,
    pub left: String,
    pub right: String,
    pub differs: bool,
}
