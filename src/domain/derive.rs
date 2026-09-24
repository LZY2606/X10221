//! 派生层（补偿后信号、变换坐标、门控细胞群）与失效传播。
//!
//! 纯函数：只依赖 `State` 当前重放结果，不改写任何来源。
//! 结果带完整来源链（依赖槽位 + 事件 id + 矩阵/变换/门修订号），
//! 因此同一事件点重放总能得到同一可见结论。

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::domain::math::{asinh_transform, invert, point_in_polygon};
use crate::domain::model::{GatePolygon, State};
use crate::domain::state::State as Aggregate;
use crate::domain::units::Unit;

/// 一个依赖槽位的钉版本引用：分母 / 候选 / 结论都引用这种坐标。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct DepRef {
    pub slot: String,
    pub version: i64,
    pub event_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompensationOutcome {
    pub status: String,
    pub channels: Vec<String>,
    /// 行=事件，列=通道；无法计算（NaN 通道 / 未解决近奇异）时为 None。
    pub values: Vec<Vec<Option<f64>>>,
    pub inverse_used: Option<String>,
    pub inverse: Option<Vec<Vec<f64>>>,
    pub deps: Vec<DepRef>,
    pub diagnostics: Vec<String>,
    pub candidates_pending: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransformedOutcome {
    /// 每个通道的变换坐标；缺失值（NaN 等）保持 None。
    pub values: Vec<Vec<Option<f64>>>,
    pub channels: Vec<String>,
    pub cofactors: BTreeMap<String, f64>,
    pub deps: Vec<DepRef>,
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PopulationOutcome {
    pub gate_id: String,
    pub gate_revision: i64,
    pub name: String,
    pub parent_gate_id: Option<String>,
    pub included: Vec<usize>,
    pub boundary_events: Vec<usize>,
    pub numerator: usize,
    /// 分母数值：钉住的父群版本的计数（根群为全部可用事件）。
    pub denominator: usize,
    pub percent: f64,
    /// 分母身份：父群结果指纹（版本依赖集合），规则要求「钉住父群版本」。
    pub denominator_ref: DenominatorRef,
    pub deps: BTreeSet<DepRef>,
    pub status: String,
    pub stale_reasons: Vec<String>,
    pub blocked_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DenominatorRef {
    pub kind: String,
    pub parent_gate_id: Option<String>,
    /// 父群结果计算时的依赖指纹（排序后的 DepRef）。
    pub parent_fingerprint: String,
    pub parent_event_scope: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateNode {
    pub gate_id: String,
    pub name: String,
    pub parent_gate_id: Option<String>,
    pub children: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Lineage {
    pub gates: BTreeMap<String, GatePolygon>,
    pub ordered: Vec<String>,
    pub children: BTreeMap<String, Vec<String>>,
    pub roots: Vec<String>,
}

/// 当前解释下的门树（含嵌套/改名后的父子关系），拓扑排序便于自上而下计算。
pub fn gate_lineage(st: &Aggregate) -> Result<Lineage, String> {
    let mut lineage = Lineage::default();
    for g in st.gates.values() {
        lineage.gates.insert(g.id.clone(), g.clone());
    }
    // link 取每个 gate 最新一条（slot_versions 已按 seq 递增覆盖）。
    let mut link_of: BTreeMap<String, _> = BTreeMap::new();
    for l in st.links.values() {
        link_of.insert(l.gate_id.clone(), l);
    }
    let mut indeg: BTreeMap<String, usize> = BTreeMap::new();
    for id in lineage.gates.keys() {
        indeg.insert(id.clone(), 0);
    }
    for (id, link) in &link_of {
        if !lineage.gates.contains_key(id) {
            continue;
        }
        if let Some(p) = &link.parent_gate_id {
            if !lineage.gates.contains_key(p) {
                return Err(format!("门 {} 引用了不存在的父门 {}", link.display_name, p));
            }
            *indeg.entry(id.clone()).or_insert(0) += 1;
            lineage.children.entry(p.clone()).or_default().push(id.clone());
        } else {
            lineage.roots.push(id.clone());
        }
    }
    for v in lineage.children.values_mut() {
        v.sort();
    }
    lineage.roots.sort();
    // Kahn 拓扑排序；有环时报错。
    let mut queue: Vec<String> = lineage.roots.clone();
    let mut seen = BTreeSet::new();
    while let Some(id) = queue.pop() {
        if !seen.insert(id.clone()) {
            continue;
        }
        lineage.ordered.push(id.clone());
        if let Some(kids) = lineage.children.get(&id) {
            for c in kids {
                let d = indeg.get_mut(c).unwrap();
                *d -= 1;
                if *d == 0 {
                    queue.push(c.clone());
                }
            }
        }
    }
    if lineage.ordered.len() != lineage.gates.len() {
        return Err("门嵌套存在环，无法形成谱系".into());
    }
    Ok(lineage)
}

fn dep(st: &Aggregate, slot: &str) -> Option<DepRef> {
    let v = *st.slot_versions.get(slot)?;
    let e = st.slot_events.get(slot)?.clone();
    Some(DepRef { slot: slot.into(), version: v, event_id: e })
}

pub fn fingerprint(deps: &BTreeSet<DepRef>) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    for d in deps {
        h.update(d.slot.as_bytes());
        h.update(b":");
        h.update(d.version.to_string().as_bytes());
        h.update(b":");
        h.update(d.event_id.as_bytes());
        h.update(b"\n");
    }
    hex::encode(h.finalize())
}

/// 补偿计算。规则：近奇异且没有人工接受的候选 -> blocked，绝不默默求逆。
pub fn compensate(st: &Aggregate) -> CompensationOutcome {
    let mut deps = Vec::new();
    if let Some(d) = dep(st, "source") {
        deps.push(d);
    }
    let raw = match &st.raw {
        Some(r) => r,
        None => {
            return CompensationOutcome {
                status: "no_source".into(),
                channels: vec![],
                values: vec![],
                inverse_used: None,
                inverse: None,
                deps,
                diagnostics: vec!["尚未导入 FCS 事件".into()],
                candidates_pending: vec![],
            }
        }
    };
    let channels: Vec<String> = raw.channels.iter().map(|c| c.name.clone()).collect();
    let matrix = match &st.matrix {
        Some(m) => m,
        None => {
            return CompensationOutcome {
                status: "no_matrix".into(),
                channels,
                values: raw.events.iter().map(|row| row.iter().map(|m| m.finite()).collect()).collect(),
                inverse_used: None,
                inverse: None,
                deps,
                diagnostics: vec!["尚未采纳补偿矩阵，显示原始通道值".into()],
                candidates_pending: vec![],
            }
        }
    };
    if let Some(d) = dep(st, "matrix") {
        deps.push(d);
    }

    let status_info = st.matrix_status.as_ref();
    let near = status_info.map(|s| s.near_singular).unwrap_or(false);
    let accepted = status_info.and_then(|s| {
        s.accepted_candidate_id
            .as_ref()
            .map(|id| (id, s.candidates.iter().find(|c| &c.id == id)))
    });

    if near {
        let pending: Vec<String> = status_info
            .map(|s| s.candidates.iter().map(|c| c.id.clone()).collect())
            .unwrap_or_default();
        match accepted {
            Some((cid, Some(cand))) => {
                run_compensation(raw, matrix, &cand.inverse, deps, "adjudicated", Some(cid.clone()))
            }
            _ => CompensationOutcome {
                status: "blocked_near_singular".into(),
                channels,
                values: raw.events.iter().map(|row| row.iter().map(|m| m.finite()).collect()).collect(),
                inverse_used: None,
                inverse: None,
                deps,
                diagnostics: vec![
                    format!(
                        "近奇异补偿矩阵（rcond={}）：未默默求逆，等待人工裁定候选",
                        status_info
                            .and_then(|s| s.report.as_ref())
                            .map(|r| format!("{:.3e}", r.rcond))
                            .unwrap_or_else(|| "?".into())
                    ),
                ],
                candidates_pending: pending,
            },
        }
    } else {
        match invert(&matrix.spill) {
            Some(inv) => run_compensation(raw, matrix, &inv, deps, "compensated", Some("lu_inverse".into())),
            None => CompensationOutcome {
                status: "blocked_uninvertible".into(),
                channels,
                values: raw.events.iter().map(|row| row.iter().map(|m| m.finite()).collect()).collect(),
                inverse_used: None,
                inverse: None,
                deps,
                diagnostics: vec!["矩阵无法分解".into()],
                candidates_pending: vec![],
            },
        }
    }
}

fn run_compensation(
    raw: &crate::domain::model::RawEvents,
    matrix: &crate::domain::model::CompensationMatrix,
    inv: &[Vec<f64>],
    deps: Vec<DepRef>,
    status: &str,
    inverse_used: Option<String>,
) -> CompensationOutcome {
    let fluoro_idx: Vec<usize> = raw
        .channels
        .iter()
        .enumerate()
        .filter(|(_, c)| matrix.channels.iter().any(|n| n == &c.name))
        .map(|(i, _)| i)
        .collect();
    let src_values: Vec<Option<f64>> = raw
        .events
        .first()
        .map(|_| (0..raw.channels.len()).map(|_| None).collect())
        .unwrap_or_default();
    let _ = src_values;
    let mut values = Vec::with_capacity(raw.events.len());
    let mut diagnostics = Vec::new();
    for (ei, row) in raw.events.iter().enumerate() {
        let mut out = Vec::with_capacity(row.len());
        for (ci, m) in row.iter().enumerate() {
            let pos = fluoro_idx.iter().position(|&i| i == ci);
            match (m.finite(), pos) {
                // 荧光通道：y_p = sum_k inv[p][k] * x_k；任一源通道不可用则该输出置空并保留原值。
                (Some(_), Some(p)) => {
                    let mut acc = 0.0;
                    let mut missing = false;
                    for (k, &src_i) in fluoro_idx.iter().enumerate() {
                        match row[src_i].finite() {
                            Some(v) => acc += inv[p][k] * v,
                            None => {
                                missing = true;
                                diagnostics.push(format!(
                                    "event {ei} 通道 {} 含不可用原始值，对应补偿输出置空，原始值保留",
                                    raw.channels[src_i].name
                                ));
                            }
                        }
                    }
                    if missing {
                        out.push(None);
                    } else if acc.is_finite() {
                        out.push(Some(acc));
                        if acc.abs() > 262144.0 {
                            diagnostics.push(format!(
                                "event {ei} 通道 {} 补偿后极端值 {acc:.1}（超仪器量程，保留并标记）",
                                raw.channels[ci].name
                            ));
                        }
                    } else {
                        diagnostics.push(format!(
                            "event {ei} 通道 {} 补偿后非有限值，置空并保留原始值",
                            raw.channels[ci].name
                        ));
                        out.push(None);
                    }
                }
                // 非荧光通道：规范化值原样传递（含单位）。
                (v, None) => out.push(v),
                // 荧光源值本身不可用。
                (None, Some(_)) => out.push(None),
            }
        }
        values.push(out);
    }
    CompensationOutcome {
        status: status.into(),
        channels: raw.channels.iter().map(|c| c.name.clone()).collect(),
        values,
        inverse_used,
        inverse: Some(inv.to_vec()),
        deps,
        diagnostics,
        candidates_pending: vec![],
    }
}

/// 变换坐标。补偿被阻塞时，荧光通道坐标也不可用；散射通道可直接用原值。
pub fn transform(st: &Aggregate, comp: &CompensationOutcome) -> TransformedOutcome {
    let mut deps = comp.deps.clone();
    if let Some(d) = dep(st, "transform") {
        deps.push(d);
    }
    let raw = match &st.raw {
        Some(r) => r,
        None => {
            return TransformedOutcome {
                values: vec![],
                channels: vec![],
                cofactors: BTreeMap::new(),
                deps,
                diagnostics: vec!["无来源数据".into()],
            }
        }
    };
    let mut cofactors = BTreeMap::new();
    let mut values = Vec::with_capacity(comp.values.len());
    for row in &comp.values {
        let mut out = Vec::with_capacity(row.len());
        for (ci, v) in row.iter().enumerate() {
            let ch = &raw.channels[ci].name;
            let unit = raw.events[0].get(ci).map(|m| m.unit).unwrap_or(Unit::RawIntensity);
            match (v, unit) {
                (Some(x), Unit::ScatterAru) => out.push(Some(*x)),
                (Some(x), _) => {
                    let cf = st.cofactor_for(ch).unwrap_or(150.0);
                    cofactors.insert(ch.clone(), cf);
                    let t = asinh_transform(*x, cf);
                    if t.is_finite() {
                        out.push(Some(t))
                    } else {
                        out.push(None)
                    }
                }
                (None, _) => out.push(None),
            }
        }
        values.push(out);
    }
    TransformedOutcome {
        values,
        channels: comp.channels.clone(),
        cofactors,
        deps,
        diagnostics: if comp.status == "blocked_near_singular" {
            vec!["补偿被近奇异判定阻塞：荧光通道变换坐标暂不可用".into()]
        } else {
            vec![]
        },
    }
}

/// 全部门群结果，按门树自上而下计算。
/// * 分母钉住父群版本（依赖指纹）；
/// * 上游/依赖变化时旧结果保留并标记 stale，不覆盖；
/// * 近奇异未裁定：依赖荧光补偿的门 blocked，散射门仍可计算。
pub fn populations(
    st: &Aggregate,
    lineage: &Lineage,
    transformed: &TransformedOutcome,
    comp_status: &str,
) -> Vec<PopulationOutcome> {
    let total = transformed.values.len();
    let channel_index: BTreeMap<String, usize> = transformed
        .channels
        .iter()
        .enumerate()
        .map(|(i, c)| (c.clone(), i))
        .collect();
    let mut outcomes: BTreeMap<String, PopulationOutcome> = BTreeMap::new();
    let mut included_of: BTreeMap<String, Vec<usize>> = BTreeMap::new();

    // 根群（全部有限事件）身份，作为最顶层分母。
    let root_deps: BTreeSet<DepRef> = transformed.deps.iter().cloned().collect();
    let root_fp = fingerprint(&root_deps);

    for gate_id in &lineage.ordered {
        let gate = lineage.gates.get(gate_id).expect("gate present").clone();
        let link = st.links.values().filter(|l| &l.gate_id == gate_id).max_by_key(|l| l.revision);
        let link = link.cloned();
        let name = link.as_ref().map(|l| l.display_name.clone()).unwrap_or_else(|| gate.name.clone());
        let parent_gate_id = link.as_ref().and_then(|l| l.parent_gate_id.clone());

        let mut deps = BTreeSet::new();
        deps.extend(transformed.deps.iter().cloned());
        if let Some(d) = dep(st, &format!("gate:{}", gate.id)) {
            deps.insert(d);
        }
        if let Some(l) = &link {
            if let Some(d) = dep(st, &format!("link:{}", l.id)) {
                deps.insert(d);
            }
        }
        if let Some(d) = dep(st, &format!("boundary:{}", gate.id)) {
            deps.insert(d);
        }

        let (denominator, denominator_ref, parent_scope) = match &parent_gate_id {
            None => (
                total,
                DenominatorRef {
                    kind: "root_events".into(),
                    parent_gate_id: None,
                    parent_fingerprint: root_fp.clone(),
                    parent_event_scope: st.head_seq,
                },
                st.head_seq,
            ),
            Some(pid) => match outcomes.get(pid) {
                Some(pop) => (
                    pop.numerator,
                    DenominatorRef {
                        kind: "parent_population".into(),
                        parent_gate_id: Some(pid.clone()),
                        parent_fingerprint: fingerprint(&pop.deps),
                        parent_event_scope: st.head_seq,
                    },
                    st.head_seq,
                ),
                None => (
                    0,
                    DenominatorRef {
                        kind: "parent_unavailable".into(),
                        parent_gate_id: Some(pid.clone()),
                        parent_fingerprint: String::new(),
                        parent_event_scope: st.head_seq,
                    },
                    st.head_seq,
                ),
            },
        };

        let needs_fluoro = gate_coordinate_is_fluoro(st, &gate);
        let blocked_by_comp = needs_fluoro && comp_status == "blocked_near_singular";
        let parent_blocked = parent_gate_id
            .as_ref()
            .and_then(|p| outcomes.get(p))
            .map(|p| p.status == "blocked" || p.status == "stale_blocked")
            .unwrap_or(false);

        let mut blocked_reason = None;
        let mut included: Vec<usize> = Vec::new();
        let mut boundary_events = Vec::new();

        if blocked_by_comp {
            blocked_reason = Some("上游补偿矩阵近奇异且未裁定，门控结果阻塞".into());
        } else if parent_blocked {
            blocked_reason = Some("父群被阻塞，分母不可用".into());
        } else {
            let scope: &[usize] = match &parent_gate_id {
                Some(pid) => included_of.get(pid).map(|v| v.as_slice()).unwrap_or(&[]),
                None => &(0..total).collect::<Vec<_>>(),
            };
            let xi = match channel_index.get(&gate.x_axis.channel) {
                Some(i) => *i,
                None => {
                    blocked_reason = Some(format!("X 轴通道 {} 不存在", gate.x_axis.channel));
                    usize::MAX
                }
            };
            let yi = match channel_index.get(&gate.y_axis.channel) {
                Some(i) => *i,
                None => {
                    blocked_reason = Some(format!("Y 轴通道 {} 不存在", gate.y_axis.channel));
                    usize::MAX
                }
            };
            if blocked_reason.is_none() {
                for &ei in scope {
                    let row = transformed.values.get(ei);
                    let (x, y) = match row {
                        Some(r) => (r.get(xi).copied().flatten(), r.get(yi).copied().flatten()),
                        None => (None, None),
                    };
                    let (x, y) = match (x, y) {
                        (Some(x), Some(y)) => (x, y),
                        _ => continue,
                    };
                    let (inside, boundary) = point_in_polygon([x, y], &gate.vertices);
                    if boundary {
                        boundary_events.push(ei);
                    }
                    let keep = match st.boundary.get(&(gate.id.clone(), ei)) {
                        Some(v) => *v,
                        None => inside,
                    };
                    if keep {
                        included.push(ei);
                    }
                }
            }
        }

        let numerator = included.len();
        let percent = if denominator > 0 {
            100.0 * numerator as f64 / denominator as f64
        } else {
            0.0
        };
        let status = if blocked_reason.is_some() {
            "blocked"
        } else {
            "current"
        };

        // 人工否定：结果可见但标记 rejected（不删除，历史可回读）。
        let rejected = st.adjudications.iter().rev().find(|a| a.target == gate_id).map(|a| !a.accepted);
        let status = if rejected == Some(true) {
            "rejected"
        } else {
            status
        };

        let outcome = PopulationOutcome {
            gate_id: gate_id.clone(),
            gate_revision: gate.revision,
            name,
            parent_gate_id,
            included: included.clone(),
            boundary_events,
            numerator,
            denominator,
            percent,
            denominator_ref,
            deps,
            status: status.into(),
            stale_reasons: vec![],
            blocked_reason,
        };
        included_of.insert(gate_id.clone(), included);
        outcomes.insert(gate_id.clone(), outcome);
    }

    let mut out: Vec<PopulationOutcome> = outcomes.into_values().collect();
    out.sort_by(|a, b| lineage.ordered.iter().position(|g| g == &a.gate_id).cmp(&lineage.ordered.iter().position(|g| g == &b.gate_id)));
    out
}

fn gate_coordinate_is_fluoro(st: &Aggregate, gate: &GatePolygon) -> bool {
    let raw = match &st.raw {
        Some(r) => r,
        None => return true,
    };
    let is_scatter = |ch: &str| {
        raw.channels.iter().any(|c| {
            c.name == ch && (c.name.starts_with("FSC") || c.name.starts_with("SSC"))
        })
    };
    !is_scatter(&gate.x_axis.channel) || !is_scatter(&gate.y_axis.channel)
}

/// 计算失效范围：给定新旧状态，哪些门结果的依赖指纹发生了变化。
/// 返回 (受影响 gate_id, 原因)。旧结果由存储层保留，仅打 stale 标记。
pub fn invalidation_scope<'a>(
    changed_slots: &BTreeMap<String, (i64, i64)>,
    lineage: &Lineage,
    gate_deps: &'a BTreeMap<String, BTreeSet<DepRef>>,
) -> BTreeMap<String, Vec<String>> {
    let mut reasons: BTreeMap<String, Vec<String>> = BTreeMap::new();
    // 受影响槽位集合 + 沿门树向下传播。
    let mut affected: BTreeSet<String> = BTreeSet::new();
    let mut notes: BTreeMap<String, String> = BTreeMap::new();
    for slot in changed_slots.keys() {
        if slot == "source" {
            // 源数据改动：只影响实际使用了相关通道的门，这里保守传播全树，
            // 但在场景里「局部源数据改动」通过事件携带的 affected_channels 精确收窄，
            // 见 store 层 invalidation_with_channels。
            for g in &lineage.ordered {
                affected.insert(g.clone());
                notes.entry(g.clone()).or_insert_with(|| "源数据改动".into());
            }
        } else if slot == "matrix" || slot == "transform" {
            for (g, deps) in gate_deps {
                if deps.iter().any(|d| d.slot == *slot || d.slot == "source") {
                    affected.insert(g.clone());
                    notes.entry(g.clone()).or_insert_with(|| format!("{slot} 版本变化"));
                }
            }
        } else if let Some(g) = slot.strip_prefix("gate:") {
            affected.insert(g.to_string());
            notes.entry(g.to_string()).or_insert_with(|| "门多边形修订".into());
        } else if let Some(g) = slot.strip_prefix("link:") {
            // link:id 对应 gate 映射由调用方传入，这里交给 store 层解析后再传播。
            let _ = g;
        } else if let Some(g) = slot.strip_prefix("boundary:") {
            affected.insert(g.to_string());
            notes.entry(g.to_string()).or_insert_with(|| "边界点归属修订".into());
        }
    }
    // 沿门树向下传播：上游过期则下游过期（不覆盖旧结果）。
    let mut stack: Vec<String> = affected.iter().cloned().collect();
    while let Some(g) = stack.pop() {
        if let Some(kids) = lineage.children.get(&g) {
            for c in kids {
                if affected.insert(c.clone()) {
                    notes.entry(c.clone()).or_insert_with(|| format!("上游门 {g} 过期传播"));
                    stack.push(c.clone());
                }
            }
        }
    }
    for g in affected {
        reasons.insert(g, vec![notes.get(&g).cloned().unwrap_or_else(|| "依赖失效".into())]);
    }
    reasons
}
