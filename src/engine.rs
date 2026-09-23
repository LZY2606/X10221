//! 派生层：原始通道值 -> 补偿 -> 变换 -> 门归属 -> 群体计数。
//!
//! 输入全部引用“来源层”的已解析对象；派生结果显式带 dependency_snapshot
//! （钉住每个输入的对象版本），来源变化时由服务层计算失效范围。

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

use crate::domain::{
    point_in_polygon, transform_value, ChannelDef, CompensationData, Diagnostic, EventsData,
    GateGeometry, LineageData, RawEvent, SpilloverData, TransformData,
};
use crate::linalg;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EventPoint {
    pub event_id: String,
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PopStatus {
    Current,
    Stale,
    Blocked,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PopulationResult {
    pub population_id: String,
    pub gate_id: String,
    pub label: String,
    pub parent_population_id: Option<String>,
    pub count: usize,
    /// 分母：钉住的父群 population 版本 id，绝不指向“最新”。
    pub denominator_version: Option<String>,
    pub denominator_count: usize,
    pub percent_of_parent: Option<f64>,
    pub member_events: Vec<String>,
    pub status: PopStatus,
    pub stale_reason: Option<String>,
    pub dependency_snapshot: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GateResult {
    pub gate_id: String,
    pub members: Vec<String>,
    pub boundary_members: Vec<String>,
    pub points: Vec<EventPoint>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompCandidate {
    pub key: String,
    pub method: String,
    pub ridge_epsilon: Option<f64>,
    pub pivot_ratio: f64,
    pub determinant: f64,
    pub amplification: f64,
    pub matrix: Vec<Vec<f64>>,
    pub accepted: bool,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompDerivation {
    pub spillover_version: String,
    pub state: String, // unique | candidates | blocked
    pub candidates: Vec<CompCandidate>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone)]
pub struct PreparedInputs {
    pub channels: Vec<ChannelDef>,
    pub events: EventsData,
    pub spillover: Option<SpilloverData>,
    pub compensation: Option<CompensationData>,
    pub transforms: TransformData,
    pub gates: Vec<GateGeometry>,
    pub lineage: LineageData,
}

pub const NEAR_THRESHOLD: f64 = 1e-6;

#[derive(Debug, Clone)]
pub struct WorkspaceEvent {
    pub event_id: String,
    pub raw: BTreeMap<String, f64>,
    pub unparseable: BTreeSet<String>,
    pub saturated: Vec<(String, String)>,
    pub excluded_fluo: bool,
    pub after: BTreeMap<String, f64>,
    pub coords: BTreeMap<String, f64>,
}

fn parse_all(
    channels: &[ChannelDef],
    events: &EventsData,
) -> (Vec<WorkspaceEvent>, Vec<Diagnostic>) {
    let mut diags = Vec::new();
    let mut workspace = Vec::new();
    for RawEvent { event_id, channels: map } in &events.events {
        let mut raw = BTreeMap::new();
        let mut unparseable = BTreeSet::new();
        let mut saturated = Vec::new();
        for (ch_id, repr) in map {
            let trimmed = repr.trim();
            match trimmed.parse::<f64>() {
                Ok(v) if v.is_finite() => {
                    raw.insert(ch_id.clone(), v);
                    if let Some(def) = channels.iter().find(|c| c.id == *ch_id) {
                        if v < def.range_min || v > def.range_max {
                            saturated.push((ch_id.clone(), repr.clone()));
                        }
                    }
                }
                _ => {
                    unparseable.insert(ch_id.clone());
                    diags.push(Diagnostic {
                        code: "unconvertible_value".to_string(),
                        severity: "error".to_string(),
                        event_id: Some(event_id.clone()),
                        channel: Some(ch_id.clone()),
                        message: "原始值无法转换为数值，保留原始表示，该事件从该轴排除"
                            .to_string(),
                        value_repr: Some(repr.clone()),
                    });
                }
            }
        }
        for (ch_id, repr) in &saturated {
            diags.push(Diagnostic {
                code: "extreme_signal".to_string(),
                severity: "warning".to_string(),
                event_id: Some(event_id.clone()),
                channel: Some(ch_id.clone()),
                message: "信号超出声明量程（极端信号），保留数值用于审查并标记".to_string(),
                value_repr: Some(repr.clone()),
            });
        }
        workspace.push(WorkspaceEvent {
            event_id: event_id.clone(),
            raw,
            unparseable,
            saturated,
            excluded_fluo: false,
            after: BTreeMap::new(),
            coords: BTreeMap::new(),
        });
    }
    (workspace, diags)
}

/// 构造补偿候选（绝不静默求逆）。
pub fn derive_compensation(spillover_version: &str, spill: &SpilloverData) -> CompDerivation {
    let analysis = linalg::analyze(&spill.matrix, NEAR_THRESHOLD);
    let mut diagnostics = Vec::new();
    if analysis.singular {
        diagnostics.push(Diagnostic {
            code: "singular_compensation_matrix".to_string(),
            severity: "error".to_string(),
            event_id: None,
            channel: None,
            message: "spillover 矩阵精确奇异（存在零枢轴），无法求逆，补偿被阻断"
                .to_string(),
            value_repr: None,
        });
        return CompDerivation {
            spillover_version: spillover_version.to_string(),
            state: "blocked".to_string(),
            candidates: vec![],
            diagnostics,
        };
    }
    let inv = linalg::inverse(&spill.matrix).expect("非奇异矩阵必然可求逆");
    if analysis.near_singular {
        let mut candidates = Vec::new();
        for (key, eps) in [("ridge_1e6", 1e-6), ("ridge_1e3", 1e-3)] {
            let m = linalg::ridge_inverse(&spill.matrix, eps).unwrap();
            candidates.push(CompCandidate {
                key: key.to_string(),
                method: "tikhonov_ridge".to_string(),
                ridge_epsilon: Some(eps),
                pivot_ratio: analysis.pivot_ratio,
                determinant: analysis.determinant,
                amplification: linalg::amplification(&m),
                matrix: m,
                accepted: false,
                note: "近奇异矩阵：显式正则化候选，需人工裁定".to_string(),
            });
        }
        diagnostics.push(Diagnostic {
            code: "near_singular_compensation_matrix".to_string(),
            severity: "warning".to_string(),
            event_id: None,
            channel: None,
            message: format!(
                "最小/最大枢轴比 {:.3e} 低于阈值 {:.0e}，不静默求逆，给出 {} 个候选",
                analysis.pivot_ratio,
                NEAR_THRESHOLD,
                candidates.len()
            ),
            value_repr: None,
        });
        CompDerivation {
            spillover_version: spillover_version.to_string(),
            state: "candidates".to_string(),
            candidates,
            diagnostics,
        }
    } else {
        let cand = CompCandidate {
            key: "exact_inverse".to_string(),
            method: "lu_inverse".to_string(),
            ridge_epsilon: None,
            pivot_ratio: analysis.pivot_ratio,
            determinant: analysis.determinant,
            amplification: linalg::amplification(&inv),
            matrix: inv,
            accepted: false,
            note: "条件良好，唯一普通逆".to_string(),
        };
        CompDerivation {
            spillover_version: spillover_version.to_string(),
            state: "unique".to_string(),
            candidates: vec![cand],
            diagnostics,
        }
    }
}

fn apply_compensation(
    channels: &[ChannelDef],
    spill: Option<&SpilloverData>,
    comp: Option<&CompensationData>,
    workspace: &mut [WorkspaceEvent],
) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    let fluo_order: Vec<&ChannelDef> = match comp {
        Some(c) => c
            .detector_order
            .iter()
            .map(|id| channels.iter().find(|ch| ch.id == *id).unwrap())
            .collect(),
        None => channels
            .iter()
            .filter(|c| c.fluorophore.is_some())
            .collect(),
    };
    let order_ids: Vec<String> = fluo_order.iter().map(|c| c.id.clone()).collect();
    for ev in workspace.iter_mut() {
        let mut obs = Vec::new();
        let mut usable = true;
        for id in &order_ids {
            match ev.raw.get(id) {
                Some(v) => obs.push(*v),
                None => usable = false,
            }
        }
        if !order_ids.is_empty() && !usable {
            ev.excluded_fluo = true;
            diags.push(Diagnostic {
                code: "event_excluded_from_compensation".to_string(),
                severity: "warning".to_string(),
                event_id: Some(ev.event_id.clone()),
                channel: None,
                message: "存在不可解析荧光值，事件保留但不参与补偿/荧光门".to_string(),
                value_repr: None,
            });
        }
        if let Some(c) = comp {
            if usable {
                let v = linalg::mat_vec(&c.matrix, &obs);
                for (id, val) in order_ids.iter().zip(v) {
                    ev.after.insert(id.clone(), val);
                }
                for ch in &fluo_order {
                    if let Some(val) = ev.after.get(&ch.id) {
                        if *val < ch.range_min || *val > ch.range_max {
                            diags.push(Diagnostic {
                                code: "compensated_value_out_of_range".to_string(),
                                severity: "warning".to_string(),
                                event_id: Some(ev.event_id.clone()),
                                channel: Some(ch.id.clone()),
                                message: "补偿后信号超出声明量程（极端信号），保留数值并标记"
                                    .to_string(),
                                value_repr: None,
                            });
                        }
                    }
                }
            }
        }
    }
    let _ = spill;
    diags
}

fn apply_transforms(
    prepared: &PreparedInputs,
    workspace: &mut [WorkspaceEvent],
) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    for ev in workspace.iter_mut() {
        for (ch_id, tr) in &prepared.transforms.channels {
            let source = match prepared
                .channels
                .iter()
                .find(|c| c.id == *ch_id)
            {
                Some(def) if def.fluorophore.is_some() => ev.after.get(ch_id).copied(),
                _ => ev.raw.get(ch_id).copied(),
            };
            if let Some(v) = source {
                ev.coords.insert(ch_id.clone(), transform_value(tr, v));
            } else if matches!(tr.kind, crate::domain::TransformKind::Asinh { .. })
                && ev.excluded_fluo
            {
                diags.push(Diagnostic {
                    code: "axis_unavailable".to_string(),
                    severity: "info".to_string(),
                    event_id: Some(ev.event_id.clone()),
                    channel: Some(ch_id.clone()),
                    message: "补偿不可用，无法生成该荧光轴坐标".to_string(),
                    value_repr: None,
                });
            }
        }
    }
    diags
}

pub struct EngineOutput {
    pub gates: Vec<GateResult>,
    pub populations: Vec<PopulationResult>,
    pub workspace: Vec<WorkspaceEvent>,
    pub diagnostics: Vec<Diagnostic>,
    pub comp_blocked: bool,
}

/// 计算一个门的归属；eligible 为进入该门父群的候选事件。
fn eval_gate(
    gate: &GateGeometry,
    eligible: &BTreeSet<String>,
    workspace: &[WorkspaceEvent],
) -> GateResult {
    let mut members = Vec::new();
    let mut boundary = Vec::new();
    let mut points = Vec::new();
    for ev in workspace {
        if !eligible.contains(&ev.event_id) {
            continue;
        }
        let (cx, cy) = match (ev.coords.get(&gate.x.channel), ev.coords.get(&gate.y.channel)) {
            (Some(x), Some(y)) => (*x, *y),
            _ => continue,
        };
        let p = crate::domain::Point { x: cx, y: cy };
        let (inside, on_b) = point_in_polygon(p, &gate.polygon);
        if inside {
            members.push(ev.event_id.clone());
            points.push(EventPoint {
                event_id: ev.event_id.clone(),
                x: cx,
                y: cy,
            });
            if on_b {
                boundary.push(ev.event_id.clone());
            }
        }
    }
    GateResult {
        gate_id: gate.gate_id.clone(),
        members,
        boundary_members: boundary,
        points,
    }
}

fn gate_dependencies(
    gate: &GateGeometry,
    prepared: &PreparedInputs,
) -> BTreeMap<String, String> {
    let mut deps = BTreeMap::new();
    deps.insert("events".to_string(), "__events__".to_string());
    deps.insert(
        format!("transform:{}", gate.x.channel),
        gate.x.basis.clone(),
    );
    deps.insert(
        format!("transform:{}", gate.y.channel),
        gate.y.basis.clone(),
    );
    deps.insert(
        format!("polygon:{}", gate.gate_id),
        gate.coordinate_basis.clone(),
    );
    let fluo_axes = [&gate.x.channel, &gate.y.channel]
        .into_iter()
        .filter(|id| {
            prepared
                .channels
                .iter()
                .find(|c| c.id == **id)
                .and_then(|c| c.fluorophore.clone())
                .is_some()
        })
        .count();
    if fluo_axes > 0 {
        deps.insert("compensation".to_string(), "__comp__".to_string());
        deps.insert("spillover".to_string(), "__spill__".to_string());
    }
    deps
}

pub fn run(
    prepared: &PreparedInputs,
    version_refs: &BTreeMap<String, String>,
) -> EngineOutput {
    let (mut workspace, mut diags) = parse_all(&prepared.channels, &prepared.events);
    diags.extend(apply_compensation(
        &prepared.channels,
        prepared.spillover.as_ref(),
        prepared.compensation.as_ref(),
        &mut workspace,
    ));
    diags.extend(apply_transforms(prepared, &mut workspace));

    let comp_blocked = match prepared.spillover {
        Some(_) => prepared.compensation.is_none(),
        None => false,
    };

    let gate_by_id: BTreeMap<String, &GateGeometry> = prepared
        .gates
        .iter()
        .map(|g| (g.gate_id.clone(), g))
        .collect();
    let node_by_gate: BTreeMap<String, &crate::domain::LineageNode> = prepared
        .lineage
        .nodes
        .iter()
        .map(|n| (n.gate_id.clone(), n))
        .collect();
    let parent_edge: BTreeMap<String, Option<String>> = prepared
        .lineage
        .nodes
        .iter()
        .map(|n| {
            let parent = prepared
                .lineage
                .edges
                .iter()
                .find(|e| e.child == n.node_id)
                .map(|e| e.parent.clone());
            (n.node_id.clone(), parent)
        })
        .collect();

    // 根群：全部导入事件（含不可解析事件，永不丢弃）。
    let root_events: BTreeSet<String> = workspace
        .iter()
        .map(|e| e.event_id.clone())
        .collect();

    // 沿谱系自上而下求值；父群成员是子门候选集。
    let mut eligible_by_node: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    eligible_by_node.insert(prepared.lineage.root_id.clone(), root_events.clone());
    let mut gate_results = BTreeMap::new();

    let mut ordered_nodes: Vec<&crate::domain::LineageNode> = Vec::new();
    let mut resolved: BTreeSet<String> = BTreeSet::new();
    while ordered_nodes.len() < prepared.lineage.nodes.len() {
        let before = ordered_nodes.len();
        for node in &prepared.lineage.nodes {
            if resolved.contains(&node.node_id) {
                continue;
            }
            let ready = match parent_edge.get(&node.node_id).unwrap() {
                Some(p) => resolved.contains(p) || *p == prepared.lineage.root_id,
                None => true,
            };
            if ready {
                ordered_nodes.push(node);
                resolved.insert(node.node_id.clone());
            }
        }
        if ordered_nodes.len() == before {
            break;
        }
    }

    let mut gate_pop = BTreeMap::new();
    for node in &ordered_nodes {
        let gate = gate_by_id[&node.gate_id];
        let parent_node = parent_edge
            .get(&node.node_id)
            .unwrap()
            .clone();
        let eligible: BTreeSet<String> = match &parent_node {
            Some(p) if *p == prepared.lineage.root_id => root_events.clone(),
            Some(p) => {
                let pn = prepared.lineage.nodes.iter().find(|n| n.node_id == *p).unwrap();
                gate_results
                    .get(&pn.gate_id)
                    .map(|g: &GateResult| g.members.iter().cloned().collect())
                    .unwrap_or_default()
            }
            None => root_events.clone(),
        };
        let result = eval_gate(gate, &eligible, &workspace);
        gate_results.insert(gate.gate_id.clone(), result);
        eligible_by_node.insert(node.node_id.clone(), eligible);
        gate_pop.insert(node.gate_id.clone(), (node, parent_node.clone()));
    }

    // 组装群体结果：分母钉住父群版本。
    let mut populations = Vec::new();
    let root_count = root_events.len();
    populations.push(PopulationResult {
        population_id: "pop:root".to_string(),
        gate_id: "__root__".to_string(),
        label: prepared
            .lineage
            .nodes
            .iter()
            .find(|n| n.node_id == prepared.lineage.root_id)
            .map(|n| n.label.clone())
            .unwrap_or_else(|| "全部导入事件".to_string()),
        parent_population_id: None,
        count: root_count,
        denominator_version: None,
        denominator_count: root_count,
        percent_of_parent: Some(100.0),
        member_events: root_events.iter().cloned().collect(),
        status: PopStatus::Current,
        stale_reason: None,
        dependency_snapshot: BTreeMap::from([(
            "events".to_string(),
            version_refs.get("events").cloned().unwrap_or_default(),
        )]),
    });

    for node in &ordered_nodes {
        let gate = gate_by_id[&node.gate_id];
        let result = gate_results.get(&gate.gate_id).unwrap();
        let parent_node = parent_edge.get(&node.node_id).unwrap().clone();
        let (parent_pop_id, denom_count, denom_version) = match &parent_node {
            None | Some(ref p) if *p == prepared.lineage.root_id => (
                Some("pop:root".to_string()),
                root_count,
                version_refs.get("events").cloned(),
            ),
            Some(p) => {
                let pn = prepared.lineage.nodes.iter().find(|n| n.node_id == *p).unwrap();
                let pg = gate_results.get(&pn.gate_id).unwrap();
                (
                    Some(format!("pop:{}", pn.gate_id)),
                    pg.members.len(),
                    version_refs
                        .get(&format!("population:{}", pn.gate_id))
                        .cloned(),
                )
            }
        };
        let mut deps = gate_dependencies(gate, prepared);
        for (_k, v) in deps.iter_mut() {
            *v = version_refs
                .get(v)
                .or_else(|| {
                    if v == "__events__" {
                        version_refs.get("events")
                    } else if v == "__comp__" {
                        version_refs.get("compensation")
                    } else if v == "__spill__" {
                        version_refs.get("spillover")
                    } else {
                        None
                    }
                })
                .cloned()
                .unwrap_or_else(|| v.clone());
        }
        let uses_fluo = deps.contains_key("compensation");
        let blocked = uses_fluo && comp_blocked;
        let status = if blocked {
            PopStatus::Blocked
        } else {
            PopStatus::Current
        };
        let (count, members) = if blocked {
            (0usize, Vec::new())
        } else {
            (result.members.len(), result.members.clone())
        };
        let percent = if blocked {
            None
        } else {
            Some(if denom_count == 0 {
                0.0
            } else {
                100.0 * count as f64 / denom_count as f64
            })
        };
        populations.push(PopulationResult {
            population_id: format!("pop:{}", gate.gate_id),
            gate_id: gate.gate_id.clone(),
            label: node.label.clone(),
            parent_population_id: parent_pop_id,
            count,
            denominator_version: denom_version.clone(),
            denominator_count: denom_count,
            percent_of_parent: percent,
            member_events: members,
            status,
            stale_reason: if blocked {
                Some("compensation_blocked".to_string())
            } else {
                None
            },
            dependency_snapshot: deps,
        });
    }

    let gates: Vec<GateResult> = prepared
        .gates
        .iter()
        .map(|g| gate_results.get(&g.gate_id).cloned().unwrap())
        .collect();
    let _ = node_by_gate;

    EngineOutput {
        gates,
        populations,
        workspace,
        diagnostics: diags,
        comp_blocked,
    }
}
