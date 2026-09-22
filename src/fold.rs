//! 事件折叠出的分支状态，以及失效范围计算。
//! 关键规则：
//!  - 近奇异/奇异矩阵不默默求逆，进入 blocked + 多候选，等待人工裁定
//!  - 门的百分比分母钉住父群 pop_result_id（具体版本）
//!  - 修订上游门后下游只标记 stale，旧结论不覆盖
//!  - 参数/源数据版本变化只让依赖范围内的结论失效

use crate::db::Db;
use crate::eventlog::EventRow;
use crate::geometry::classify;
use crate::ids::content_id;
use crate::linalg;
use crate::models::*;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};

#[derive(Debug, Clone, Default)]
pub struct FoldState {
    pub fcs: Option<FcsData>,
    pub fcs_version_id: Option<String>,
    pub spill: Option<(SpilloverMeta, String)>,
    pub matrices: BTreeMap<String, (CompMatrix, String)>,
    pub active_matrix_version: Option<String>,
    pub transforms: HashMap<String, (TransformSpec, String)>,
    pub gates: HashMap<String, (GateDef, String)>,
    pub gate_label_at: HashMap<String, String>,
    pub trees: Vec<(GateTree, String)>,
    pub candidates: BTreeMap<String, CandidateSet>,
    pub decision_by_set: HashMap<String, DecisionRecord>,
    pub pops: HashMap<String, PopResult>,
    pub status_override: HashMap<String, (String, String)>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct CandidateSet {
    pub set_id: String,
    pub reason: String,
    pub matrix_version_id: String,
    pub candidates: Vec<CandidateOption>,
}

#[derive(Debug, Clone)]
pub struct CandidateOption {
    pub candidate_id: String,
    pub label: String,
    pub legal: bool,
    pub inverse: Option<Vec<Vec<f64>>>,
    pub detail: Value,
}

#[derive(Debug, Clone)]
pub struct DecisionRecord {
    pub candidate_id: Option<String>,
    pub action: String,
    pub rationale: String,
    pub event_id: String,
}

#[derive(Debug, Clone)]
pub struct Invalidation {
    pub stale: Vec<String>,
    pub blocked: Vec<String>,
    pub reason: String,
}

pub fn fold(events: &[EventRow]) -> FoldState {
    let mut st = FoldState::default();
    for ev in events {
        apply_event(&mut st, ev);
    }
    st
}

fn apply_event(st: &mut FoldState, ev: &EventRow) {
    match ev.kind.as_str() {
        "fcs_imported" => {
            let fcs: FcsData = serde_json::from_value(ev.payload["fcs"].clone()).unwrap();
            st.fcs = Some(fcs);
            st.fcs_version_id = Some(ev.payload["version_id"].as_str().unwrap().to_string());
        }
        "blob_registered" | "case_loaded" | "branch_forked" | "archive_imported" => {}
        "spillover_revised" => {
            let m: SpilloverMeta = serde_json::from_value(ev.payload["spillover"].clone()).unwrap();
            let vid = ev.payload["version_id"].as_str().unwrap().to_string();
            st.spill = Some((m, vid));
            // spillover 来源变化只影响补偿相关门
            invalidate_fluor_pops(st, &ev.event_id, "spillover 元数据版本变化");
        }
        "comp_matrix_revised" => {
            let m: CompMatrix = serde_json::from_value(ev.payload["matrix"].clone()).unwrap();
            let vid = ev.payload["version_id"].as_str().unwrap().to_string();
            st.matrices.insert(vid.clone(), (m.clone(), vid.clone()));
            st.active_matrix_version = Some(vid.clone());
            let threshold = ev
                .payload
                .get("near_threshold")
                .and_then(|v| v.as_f64())
                .unwrap_or(crate::db::NEAR_SINGULAR_RCOND);
            let report = linalg::report(&m.matrix, threshold);
            if report.singular || report.near_singular {
                invalidate_fluor_pops(st, &ev.event_id, &report.diagnose.join("; "));
                let set_id = format!("candset-comp-{}", vid.trim_start_matches("ver-comp-"));
                let mut options = vec![CandidateOption {
                    candidate_id: content_id(
                        "cand",
                        &json!({"set": &set_id, "kind": "refuse_inverse"}),
                    ),
                    label: "拒绝求逆（保留未补偿信号，等待修正矩阵）".into(),
                    legal: true,
                    inverse: None,
                    detail: json!({"kind":"refuse_inverse"}),
                }];
                if !report.singular {
                    for (idx, lambda) in [1e-6f64, 1e-4f64].iter().enumerate() {
                        if let Ok(inv) = linalg::regularized_inverse(&m.matrix, *lambda) {
                            options.push(CandidateOption {
                                candidate_id: content_id(
                                    "cand",
                                    &json!({"set": &set_id, "kind":"regularized","lambda":lambda,"inv":inv}),
                                ),
                                label: format!("Tikhonov 正则逆候选 λ={lambda}"),
                                legal: true,
                                inverse: Some(inv),
                                detail: json!({"kind":"regularized","lambda":lambda,"rank":idx}),
                            });
                        }
                    }
                }
                st.candidates.insert(
                    set_id.clone(),
                    CandidateSet {
                        set_id,
                        reason: report.diagnose.join("; "),
                        matrix_version_id: vid,
                        candidates: options,
                    },
                );
            } else if let Some((set_id, _)) = find_comp_candidate_set(st) {
                // 矩阵已修好，候选集保留为历史但结论可继续重算
                let _ = set_id;
            }
        }
        "transform_revised" => {
            let spec: TransformSpec = serde_json::from_value(ev.payload["transform"].clone()).unwrap();
            let vid = ev.payload["version_id"].as_str().unwrap().to_string();
            let ch = transform_channel(&spec).to_string();
            st.transforms.insert(ch.clone(), (spec.clone(), vid));
            // 只让使用该通道坐标的门失效
            for (_, pop) in st.pops.iter_mut() {
                if pop_uses_channel(st, pop, &ch) {
                    mark_stale(st, &pop.pop_result_id.clone(), &ev.event_id,
                        &format!("变换参数版本变化（通道 {ch}）"));
                }
            }
        }
        "gate_revised" => {
            let g: GateDef = serde_json::from_value(ev.payload["gate"].clone()).unwrap();
            let vid = ev.payload["version_id"].as_str().unwrap().to_string();
            st.gates.insert(g.gate_id.clone(), (g.clone(), vid));
            st.gate_label_at.insert(g.gate_id.clone(), g.name.clone());
            invalidate_gate_subtree(st, &g.gate_id, &ev.event_id, "门几何修订");
        }
        "gate_tree_revised" => {
            let tree: GateTree = serde_json::from_value(ev.payload["tree"].clone()).unwrap();
            let vid = ev.payload["version_id"].as_str().unwrap().to_string();
            let renamed: Vec<(String, String)> =
                serde_json::from_value(ev.payload["renamed"].clone()).unwrap_or_default();
            st.trees.push((tree.clone(), vid));
            for (gid, new_name) in renamed {
                if let Some((g, gv)) = st.gates.get(&gid).cloned() {
                    let mut g2 = g;
                    g2.name = new_name;
                    st.gates.insert(gid.clone(), (g2, gv));
                    st.gate_label_at.insert(gid, new_name);
                }
            }
            // 纯改名不改几何：不失效；父子关系变化才失效
            if ev.payload.get("relations_changed").and_then(|v| v.as_bool()).unwrap_or(false) {
                invalidate_all_pops(st, &ev.event_id, "门树父子关系变化");
            }
        }
        "events_revised" => {
            let fcs: FcsData = serde_json::from_value(ev.payload["fcs"].clone()).unwrap();
            st.fcs = Some(fcs);
            st.fcs_version_id = Some(ev.payload["version_id"].as_str().unwrap().to_string());
            invalidate_all_pops(st, &ev.event_id, "原始事件源数据修订（局部源改动，全部门重评）");
        }
        "pop_recomputed" => {
            let pop: PopResult = serde_json::from_value(ev.payload["pop"].clone()).unwrap();
            st.pops.insert(pop.pop_result_id.clone(), pop);
        }
        "pop_marked_stale" => {
            let ids: Vec<String> = serde_json::from_value(ev.payload["pop_result_ids"].clone()).unwrap();
            for id in ids {
                st.status_override.insert(
                    id,
                    ("stale".into(), ev.payload["reason"].as_str().unwrap_or("").to_string()),
                );
            }
        }
        "comp_decision" => {
            let set_id = ev.payload["candidate_set_id"].as_str().unwrap().to_string();
            let action = ev.payload["action"].as_str().unwrap().to_string();
            let rec = DecisionRecord {
                candidate_id: ev.payload.get("candidate_id").and_then(|v| v.as_str()).map(String::from),
                action,
                rationale: ev.payload.get("rationale").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                event_id: ev.event_id.clone(),
            };
            st.decision_by_set.insert(set_id, rec);
            invalidate_fluor_pops(st, &ev.event_id, "补偿裁定后需要按所选分支显式重算");
        }
        "note_added" => {
            st.notes.push(ev.payload.get("text").and_then(|v| v.as_str()).unwrap_or("").to_string());
        }
        _ => {}
    }
}

fn current_tree(st: &FoldState) -> Option<&GateTree> {
    st.trees.last().map(|(t, _)| t)
}

fn transform_channel(spec: &TransformSpec) -> &str {
    match spec {
        TransformSpec::Linear { channel_id, .. }
        | TransformSpec::Log10Offset { channel_id, .. } => channel_id,
    }
}

fn pop_uses_channel(st: &FoldState, pop: &PopResult, channel: &str) -> bool {
    if let Some((g, _)) = st.gates.get(&pop.gate_id) {
        match &g.geom {
            GateGeom::Polygon { x_channel, y_channel, .. }
            | GateGeom::Rectangle { x_channel, y_channel, .. } => {
                x_channel == channel || y_channel == channel
            }
        }
    } else {
        false
    }
}

fn mark_stale(st: &mut FoldState, pop_id: &str, event_id: &str, reason: &str) {
    st.status_override
        .insert(pop_id.to_string(), ("stale".into(), reason.into()));
    let _ = event_id;
}

fn invalidate_all_pops(st: &mut FoldState, event_id: &str, reason: &str) {
    let ids: Vec<String> = st.pops.keys().cloned().collect();
    for id in ids {
        mark_stale(st, &id, event_id, reason);
    }
}

fn invalidate_gate_subtree(st: &mut FoldState, gate_id: &str, event_id: &str, reason: &str) {
    let tree = match current_tree(st) {
        Some(t) => t.clone(),
        None => return,
    };
    let mut affected = vec![gate_id.to_string()];
    let mut frontier = vec![gate_id.to_string()];
    while let Some(g) = frontier.pop() {
        for node in &tree.nodes {
            if node.parent_gate_id.as_deref() == Some(g.as_str()) && !affected.contains(&node.gate_id) {
                affected.push(node.gate_id.clone());
                frontier.push(node.gate_id.clone());
            }
        }
    }
    for (_, pop) in st.pops.iter_mut() {
        if affected.contains(&pop.gate_id) {
            mark_stale(st, &pop.pop_result_id.clone(), event_id, reason);
        }
    }
}

fn invalidate_fluor_pops(st: &mut FoldState, event_id: &str, reason: &str) {
    let ids: Vec<String> = st
        .pops
        .iter()
        .filter_map(|(id, pop)| {
            let (g, _) = st.gates.get(&pop.gate_id)?;
            match &g.geom {
                GateGeom::Polygon { x_channel, y_channel, .. }
                | GateGeom::Rectangle { x_channel, y_channel, .. } => {
                    let fluor = !x_channel.starts_with("FSC")
                        && !x_channel.starts_with("SSC")
                        || !y_channel.starts_with("FSC") && !y_channel.starts_with("SSC");
                    if fluor { Some(id.clone()) } else { None }
                }
            }
        })
        .collect();
    for id in ids {
        mark_stale(st, &id, event_id, reason);
    }
}

fn find_comp_candidate_set(st: &FoldState) -> Option<(String, CandidateSet)> {
    st.candidates
        .iter()
        .find(|(_, cs)| {
            Some(cs.matrix_version_id.clone()) == st.active_matrix_version
        })
        .map(|(k, v)| (k.clone(), v.clone()))
}

/// 某群体在当前状态下的有效状态：stale 覆盖不删除旧结论。
pub fn effective_status(st: &FoldState, pop: &PopResult) -> (String, String) {
    if let Some((s, note)) = st.status_override.get(&pop.pop_result_id) {
        return (s.clone(), note.clone());
    }
    (pop.status.clone(), String::new())
}

fn find_gate<'a>(st: &'a FoldState, gate_id: &str) -> Option<&'a (GateDef, String)> {
    st.gates.get(gate_id)
}

fn parent_of(tree: &GateTree, gate_id: &str) -> Option<String> {
    tree.nodes
        .iter()
        .find(|n| n.gate_id == gate_id)
        .and_then(|n| n.parent_gate_id.clone())
}

fn channel_index(fcs: &FcsData, channel: &str) -> Option<usize> {
    if let Some(i) = fcs.channels.iter().position(|c| c.channel_id == channel) {
        return Some(i);
    }
    fcs.channels.iter().position(|c| c.short_name == channel)
}

fn matrix_labels_compatible(m: &CompMatrix, fcs: &FcsData) -> bool {
    m.labels.iter().all(|lab| channel_index(fcs, lab).is_some())
}

/// 解析当前补偿：返回 (inverse, labels, comp_inverse_id, blocked_reason)。
pub fn resolve_compensation(st: &FoldState) -> Result<
    (Vec<Vec<f64>>, Vec<String>, String),
    String,
> {
    let vid = st
        .active_matrix_version
        .clone()
        .ok_or_else(|| "尚无补偿矩阵版本".to_string())?;
    let (matrix, _) = st.matrices.get(&vid).cloned().ok_or("矩阵版本丢失")?;
    let fcs = st.fcs.as_ref().ok_or("尚无 FCS 事件")?;
    if !matrix_labels_compatible(&matrix, fcs) {
        return Err("补偿矩阵标签与通道不匹配".into());
    }
    let report = linalg::report(&matrix.matrix, crate::db::NEAR_SINGULAR_RCOND);
    let set_id = format!("candset-comp-{}", vid.trim_start_matches("ver-comp-"));
    if report.singular {
        return Err(format!("BLOCKED:补偿矩阵精确奇异，拒绝默默求逆（候选集 {set_id}）"));
    }
    if report.near_singular {
        let decision = st
            .decision_by_set
            .get(&set_id)
            .cloned()
            .ok_or_else(|| format!("BLOCKED:近奇异补偿矩阵未裁定（候选集 {set_id}）：{}", report.diagnose.join(";")))?;
        if decision.action == "reject" {
            return Err(format!(
                "BLOCKED:近奇异矩阵裁定为拒绝求逆（{}）",
                decision.rationale
            ));
        }
        let cs = st
            .candidates
            .get(&set_id)
            .ok_or("候选集丢失")?;
        let cand = cs
            .candidates
            .iter()
            .find(|c| Some(&c.candidate_id) == decision.candidate_id.as_ref())
            .ok_or("裁定指向的候选不存在")?;
        let inv = cand
            .inverse
            .clone()
            .ok_or("该候选不包含逆矩阵（拒绝求逆）")?;
        let inv_id = content_id("compinv", &json!({"matrix": vid, "candidate": cand.candidate_id}));
        return Ok((inv, matrix.labels.clone(), inv_id));
    }
    let inv = linalg::invert(&matrix.matrix).map_err(|e| format!("BLOCKED:{e}"))?;
    let inv_id = content_id("compinv", &json!({"matrix": vid, "exact_inverse": true}));
    Ok((inv, matrix.labels.clone(), inv_id))
}

#[derive(Debug, Clone)]
pub struct RecomputeOutput {
    pub pop: PopResult,
    pub stale_marked: Vec<String>,
}

/// 显式重算一个门群体。分母钉住父群版本 pop_result_id。
#[allow(clippy::too_many_arguments)]
pub fn recompute_pop(
    st: &FoldState,
    gate_id: &str,
    event_id: &str,
    draft_id: &str,
) -> Result<PopResult, String> {
    let fcs = st.fcs.as_ref().ok_or("尚无 FCS 事件")?.clone();
    let tree = current_tree(st).cloned().ok_or("尚无门树")?;
    let (gate, gate_geom_version_id) = find_gate(st, gate_id)
        .cloned()
        .ok_or_else(|| format!("门 {gate_id} 尚无版本"))?;
    let (xch, ych) = match &gate.geom {
        GateGeom::Polygon { x_channel, y_channel, .. }
        | GateGeom::Rectangle { x_channel, y_channel, .. } => {
            (x_channel.clone(), y_channel.clone())
        }
    };
    let xi = channel_index(&fcs, &xch).ok_or_else(|| format!("通道 {xch} 不存在"))?;
    let yi = channel_index(&fcs, &ych).ok_or_else(|| format!("通道 {ych} 不存在"))?;

    // 决定坐标来源：若该通道是补偿矩阵标签之一，使用补偿后值
    let comp = resolve_compensation(st).ok();
    let mut coord: Vec<(Option<f64>, Option<f64>, Option<String>)> = Vec::with_capacity(fcs.rows.len());
    let n_events = fcs.rows.len();
    for (ei, row) in fcs.rows.iter().enumerate() {
        let mut xraw = row[xi].value;
        let mut yraw = row[yi].value;
        let mut excluded = None;
        if let Some((ref inv, ref labels, ref inv_id)) = comp {
            if labels.contains(&fcs.channels[xi].short_name)
                || labels.contains(&fcs.channels[yi].short_name)
            {
                let mut vec_in = Vec::new();
                let mut idxs = Vec::new();
                for lab in labels {
                    let ci = channel_index(&fcs, lab).unwrap();
                    vec_in.push(row[ci].value);
                    idxs.push(ci);
                }
                if vec_in.iter().any(|v| v.is_none()) {
                    excluded = Some("补偿输入含无法转换的事件（NaN/溢出），该事件排除并保留".into());
                } else {
                    let vin: Vec<f64> = vec_in.iter().map(|v| v.unwrap()).collect();
                    let out = linalg::apply(inv, &vin);
                    for (k, lab) in labels.iter().enumerate() {
                        if lab == &fcs.channels[xi].short_name {
                            xraw = Some(out[k]);
                        }
                        if lab == &fcs.channels[yi].short_name {
                            yraw = Some(out[k]);
                        }
                    }
                }
                let _ = inv_id;
            }
        }
        // 派生层变换
        let tx = st.transforms.get(&xch).cloned();
        let ty = st.transforms.get(&ych).cloned();
        let (xv, dx) = match (&tx, xraw) {
            (Some((spec, _)), Some(v)) => {
                let (t, diag) = crate::transform::apply_transform(spec, v);
                (t, diag)
            }
            (Some(_), None) => (None, vec!["原始值无法转换".into()]),
            (None, v) => (v, vec![]),
        };
        let (yv, dy) = match (&ty, yraw) {
            (Some((spec, _)), Some(v)) => {
                let (t, diag) = crate::transform::apply_transform(spec, v);
                (t, diag)
            }
            (Some(_), None) => (None, vec!["原始值无法转换".into()]),
            (None, v) => (v, vec![]),
        };
        let mut why = excluded.clone();
        if xv.is_none() && dx.iter().any(|d| d.contains("无法") || d.contains("NaN") || d.contains("log10")) {
            why = Some(dx.join("; ") + &dy.join("; "));
        }
        if yv.is_none() && why.is_none() {
            why = Some(dy.join("; "));
        }
        coord.push((xv, yv, why));
        let _ = (ei, n_events);
    }

    // 父群版本钉住：必须显式提供已存在的父 pop_result
    let parent_gate = parent_of(&tree, gate_id);
    let (denominator_kind, denominator_ref, denominator_count, parent_members):
        (String, String, i64, Option<std::collections::HashSet<u32>>) =
        match parent_gate {
            None => (
                "all_events".into(),
                format!("fcs:{}", st.fcs_version_id.clone().unwrap_or_default()),
                n_events as i64,
                None,
            ),
            Some(pg) => {
                let parent_pop = st
                    .pops
                    .values()
                    .find(|p| p.gate_id == pg && !matches!(effective_status(st, p).0.as_str(), "stale" | "blocked"))
                    .or_else(|| st.pops.values().find(|p| p.gate_id == pg))
                    .ok_or_else(|| format!("父门 {pg} 尚无群体结果；百分比无法钉住父群版本"))?
                    .clone();
                let (stat, _) = effective_status(st, &parent_pop);
                if stat == "stale" {
                    return Err(format!(
                        "BLOCKED:父群 {} 已过期（stale），必须先显式重算父群；不允许沿用过期分母",
                        parent_pop.pop_result_id
                    ));
                }
                let members: std::collections::HashSet<u32> =
                    parent_pop.detail.iter().filter(|m| m.inside && m.excluded.is_none()).map(|m| m.event_index).collect();
                (
                    "parent_population".into(),
                    format!("pop:{}", parent_pop.pop_result_id),
                    members.len() as i64,
                    Some(members),
                )
            }
        };

    let mut detail = Vec::new();
    let mut numerator = 0i64;
    let mut excluded_n = 0i64;
    for (ei, (xv, yv, why)) in coord.into_iter().enumerate() {
        let event_index = ei as u32;
        if let Some(ref members) = parent_members {
            if !members.contains(&event_index) {
                continue; // 只在父群成员内评门
            }
        }
        let (inside, boundary) = match (xv, yv) {
            (Some(x), Some(y)) => {
                let c = classify(&gate.geom, x, y);
                (c.inside, c.boundary)
            }
            _ => (false, false),
        };
        let ex = if why.is_some() {
            excluded_n += 1;
            why.clone()
        } else {
            None
        };
        let stage = if comp.is_some() { "compensated" } else { "raw" };
        let vid_x = crate::ids::raw_value_id(draft_id, event_index, &xch, stage);
        let vid_y = crate::ids::raw_value_id(draft_id, event_index, &ych, stage);
        if inside && ex.is_none() {
            numerator += 1;
        }
        detail.push(PopMembership {
            event_index,
            value_id_x: vid_x,
            value_id_y: vid_y,
            inside: inside && ex.is_none(),
            boundary,
            excluded: ex,
        });
    }

    let percent = if denominator_count > 0 {
        Some(100.0 * numerator as f64 / denominator_count as f64)
    } else {
        None
    };
    let deps = json!({
        "fcs_version": st.fcs_version_id,
        "gate_geom_version": gate_geom_version_id,
        "tree_version": st.trees.last().map(|(_, v)| v.clone()),
        "transforms": st.transforms.iter().map(|(c, (_, v))| (c.clone(), v.clone())).collect::<HashMap<_,_>>(),
        "compensation": comp.as_ref().map(|(_, _, id)| id.clone()),
        "spillover": st.spill.as_ref().map(|(_, v)| v.clone()),
        "denominator_ref": denominator_ref,
    });
    let identity = json!({
        "gate_id": gate_id,
        "geom_version": gate_geom_version_id,
        "fcs_version": st.fcs_version_id,
        "denominator_ref": denominator_ref,
        "detail": detail,
        "transforms": deps["transforms"],
        "compensation": deps["compensation"],
    });
    let parent_pop_result_id = if denominator_kind == "parent_population" {
        denominator_ref.strip_prefix("pop:").map(String::from)
    } else {
        None
    };
    let pop_result_id = content_id("pop", &identity);
    Ok(PopResult {
        pop_result_id,
        gate_id: gate_id.to_string(),
        gate_name: gate.name.clone(),
        gate_geom_version_id,
        parent_pop_result_id,
        denominator_kind,
        denominator_ref,
        denominator_count,
        numerator_count: numerator,
        excluded_unconvertible: excluded_n,
        percent,
        status: "current".into(),
        detail,
        deps: deps.clone(),
        created_event_id: event_id.to_string(),
    })
}
