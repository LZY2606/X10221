//! 谱系视图推导：从五个来源版本 + 人工裁定，推导补偿、变换、门控与各群体结论。
//!
//! 结论不是独立文件，而是一份带完整来源身份的快照；历史事件直接保存快照，
//! 因此回到旧事件时恢复的是当时的依赖版本与可见结论，而非最新计算。

use serde_json::{json, Value};

use crate::domain::gate::{point_in_polygon, polygon_of};
use crate::domain::matrix::{mat_vec, CompAnalysis};
use crate::domain::{
    arcsinh_transform, find_gate, is_descendant_or_self, percent, raw_number, topo_order,
    KIND_GATES,
};

/// 一次推导所依据的五份来源版本（身份 + 载荷）。
pub struct SourceSet<'a> {
    pub fcs_id: &'a str,
    pub fcs: &'a Value,
    pub spill_id: &'a str,
    pub spill: &'a Value,
    pub comp_id: &'a str,
    pub comp: &'a Value,
    pub transform_id: &'a str,
    pub transform: &'a Value,
    pub gates_id: &'a str,
    pub gates: &'a Value,
}

/// 推导选项：变化类型、被改门集合、人工选择的候选逆、上一版视图。
pub struct DeriveOptions<'a> {
    pub event_id: &'a str,
    pub branch: &'a str,
    pub change: &'a str,
    pub changed_gates: &'a [String],
    pub chosen_candidate_id: Option<&'a str>,
    pub chosen_inverse: Option<&'a crate::domain::matrix::Mat>,
    pub prev: Option<&'a Value>,
}

fn matrix_from(comp: &Value) -> crate::domain::matrix::Mat {
    comp["matrix"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| {
            row.as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_f64().unwrap())
                .collect()
        })
        .collect()
}

fn round9(m: &crate::domain::matrix::Mat) -> Value {
    json!(m
        .iter()
        .map(|row| row.iter().map(|v| (v * 1e9).round() / 1e9).collect::<Vec<_>>())
        .collect::<Vec<_>>())
}

/// 主推导入口。
pub fn compute_view(sources: &SourceSet, opt: &DeriveOptions) -> Value {
    let channels: Vec<String> = sources.fcs["channels"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();

    let matrix = matrix_from(sources.comp);
    let analysis = crate::domain::matrix::analyze_compensation(&matrix);
    let usable_inverse = analysis.inverse.as_ref().or(opt.chosen_inverse);

    let comp_block = build_comp_block(sources, &analysis, opt);

    let cofactor = sources.transform["cofactor"].as_f64().unwrap_or(150.0);

    // ---- 逐事件：原始表示 -> 数字 -> 补偿 -> 变换，诊断就地保留 ----
    let mut events_out: Vec<Value> = Vec::new();
    let mut gate_boundary: Vec<Value> = Vec::new();
    let raw_events = sources.fcs["events"].as_array().unwrap();

    for ev in raw_events {
        let idx = ev["idx"].as_u64().unwrap();
        let raw = ev["raw"].as_array().unwrap();
        let mut diags: Vec<Value> = Vec::new();
        let mut numeric: Vec<Option<f64>> = Vec::new();
        for (ch, rv) in raw.iter().enumerate() {
            match raw_number(rv) {
                Some(n) => {
                    if n < crate::domain::EXTREME_LOW || n > crate::domain::EXTREME_HIGH {
                        diags.push(crate::domain::diag(
                            "extreme_signal",
                            format!(
                                "通道 {} 原始值 {} 超出常规模拟检测区间，保留原值，不截断不清洗。",
                                channels[ch], n
                            ),
                            json!({"channel": channels[ch], "raw": n}),
                        ));
                    }
                    numeric.push(Some(n));
                }
                None => {
                    diags.push(crate::domain::diag(
                        "unconvertible_value",
                        format!(
                            "通道 {} 原始表示 {:?} 无法转换为数值，留在原位置并排除出数值推导。",
                            channels[ch], rv
                        ),
                        json!({"channel": channels[ch], "raw": rv}),
                    ));
                    numeric.push(None);
                }
            }
        }

        let all_numeric = numeric.iter().all(|n| n.is_some());
        let compensated: Option<Vec<f64>> = match (all_numeric, usable_inverse) {
            (true, Some(inv)) => {
                let v: Vec<f64> = numeric.iter().map(|n| n.unwrap()).collect();
                Some(mat_vec(inv, &v))
            }
            _ => None,
        };
        if all_numeric && usable_inverse.is_none() {
            diags.push(crate::domain::diag(
                "compensation_unavailable",
                "补偿矩阵不可用（奇异/近奇异未裁定），该事件不做补偿与后续门控。",
                json!({"comp_matrix": sources.comp_id}),
            ));
        }

        let transformed: Option<Vec<f64>> = compensated.as_ref().map(|cv| {
            cv.iter()
                .map(|v| arcsinh_transform(*v, cofactor))
                .collect()
        });

        events_out.push(json!({
            "idx": idx,
            "raw": raw,
            "numeric": numeric,
            "compensated": compensated,
            "transformed": transformed,
            "diags": diags,
        }));
    }

    // ---- 门控成员（拓扑序：父门先算）----
    let order = topo_order(sources.gates);
    let mut membership: std::collections::HashMap<&str, Vec<bool>> =
        std::collections::HashMap::new();
    for gid in &order {
        let gate = find_gate(sources.gates, gid).unwrap();
        let parent = gate["parent"].as_str();
        let axes: Vec<usize> = gate["axes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| channels.iter().position(|c| c == a.as_str().unwrap()).unwrap())
            .collect();
        let polygon = polygon_of(gate);
        let mut in_gate = vec![false; events_out.len()];
        for (i, ev) in events_out.iter().enumerate() {
            if let Some(pid) = parent {
                if !membership.get(pid).map(|m| m[i]).unwrap_or(false) {
                    continue;
                }
            }
            let t = ev["transformed"].as_array();
            let coords: Option<Vec<f64>> =
                t.map(|arr| axes.iter().map(|a| arr[*a].as_f64().unwrap()).collect());
            if let Some(c) = coords {
                let (inside, boundary) = point_in_polygon(c[0], c[1], &polygon);
                if inside {
                    in_gate[i] = true;
                }
                if boundary {
                    gate_boundary.push(json!({"event": ev["idx"], "gate": gid,
                        "coordinates": [c[0], c[1]]}));
                }
            }
        }
        membership.insert(gid.as_str(), in_gate);
    }

    // ---- 过期集合：门修订后，下游门的旧结论整体保留并标记 stale ----
    let stale_ids: Vec<String> = if opt.change == KIND_GATES {
        order
            .iter()
            .filter(|gid| {
                opt.changed_gates
                    .iter()
                    .any(|ch| ch != *gid && is_descendant_or_self(sources.gates, ch, gid))
            })
            .cloned()
            .collect()
    } else {
        Vec::new()
    };

    // ---- 群体版本、计数、钉版分母 ----
    let mut populations: Vec<Value> = Vec::new();
    let total_events = events_out.len();
    for gid in &order {
        let gate = find_gate(sources.gates, gid).unwrap();
        let gate_version = gate["version"].as_u64().unwrap_or(1) as u32;

        if stale_ids.contains(gid) {
            if let Some(old) = opt
                .prev
                .and_then(|p| p["populations"].as_array())
                .and_then(|arr| arr.iter().find(|p| p["gate"] == gid.as_str()))
            {
                let changed = opt.changed_gates.first().cloned().unwrap_or_default();
                let new_v = find_gate(sources.gates, &changed)
                    .and_then(|g| g["version"].as_u64())
                    .unwrap_or(0);
                let mut carried = old.clone();
                carried["stale"] = json!(true);
                carried["stale_reason"] = json!(format!(
                    "上游门 {changed} 已修订至 v{new_v}（事件 {}）；下游旧结果标记过期但不覆盖。",
                    opt.event_id
                ));
                populations.push(carried);
            }
            continue;
        }

        let members: Vec<u64> = membership[gid.as_str()]
            .iter()
            .enumerate()
            .filter_map(|(i, in_gate)| in_gate.then_some(events_out[i]["idx"].as_u64().unwrap()))
            .collect();
        let count = members.len();
        let parent_gate = gate["parent"].as_str();
        let (denominator, pct): (Value, Option<f64>) = match parent_gate {
            None => (
                json!({
                    "kind": "total_events",
                    "fcs_version": sources.fcs_id,
                    "count": total_events,
                    "pinned_by_event": opt.event_id,
                }),
                percent(count, total_events),
            ),
            Some(pg) => {
                let parent_pop = populations
                    .iter()
                    .find(|p| p["gate"] == pg)
                    .expect("父门拓扑在前，群体应已生成");
                let pv = parent_pop["gate_version"].as_u64().unwrap() as u32;
                let pcount = parent_pop["count"].as_u64().unwrap_or(0) as usize;
                (
                    json!({
                        "kind": "population",
                        "pop": format!("pop-{pg}"),
                        "gate": pg,
                        "gate_version": pv,
                        "count": pcount,
                        "pinned_by_event": parent_pop["produced_by_event"],
                    }),
                    percent(count, pcount),
                )
            }
        };

        populations.push(json!({
            "id": format!("pop-{gid}"),
            "gate": gid,
            "name": gate["name"],
            "gate_version": gate_version,
            "parent_pop": parent_gate.map(|p| format!("pop-{p}")),
            "count": count,
            "percent": pct.map(|p| (p * 1e6).round() / 1e6),
            "denominator": denominator,
            "members": members,
            "stale": false,
            "stale_reason": Value::Null,
            "produced_by_event": opt.event_id,
        }));
    }

    let mut pending: Vec<Value> = Vec::new();
    if analysis.status == "singular" {
        pending.push(json!({
            "id": format!("pend-comp-singular-{}", opt.event_id),
            "kind": "comp_singular",
            "summary": "补偿矩阵精确奇异，补偿与门控未计算，需替换矩阵后重放。",
            "comp_matrix": sources.comp_id,
        }));
    }
    if analysis.status == "indeterminate" && opt.chosen_inverse.is_none() {
        pending.push(json!({
            "id": format!("pend-comp-candidates-{}", opt.event_id),
            "kind": "comp_candidates",
            "summary": "补偿矩阵近奇异，保留全部合法候选逆，等待人工选择（不自动取任一候选）。",
            "comp_matrix": sources.comp_id,
            "rcond": analysis.rcond,
            "candidates": analysis.candidates.iter().map(|c| json!({
                "id": c.id, "label": c.label, "inverse": round9(&c.inverse),
            })).collect::<Vec<_>>(),
        }));
    }
    let stale_list: Vec<&Value> = populations
        .iter()
        .filter(|p| p["stale"].as_bool() == Some(true))
        .collect();
    if !stale_list.is_empty() {
        pending.push(json!({
            "id": format!("pend-stale-{}", opt.event_id),
            "kind": "stale_populations",
            "summary": "存在上游修订后标记过期、尚未重算的下游群体。",
            "populations": stale_list.iter().map(|p| p["id"].clone()).collect::<Vec<_>>(),
        }));
    }

    json!({
        "branch": opt.branch,
        "head_event": opt.event_id,
        "change": opt.change,
        "sources": {
            "fcs_events": {"id": sources.fcs_id, "unit": sources.fcs["unit"],
                           "specimen": sources.fcs["specimen"]},
            "spillover": {"id": sources.spill_id},
            "comp_matrix": {"id": sources.comp_id},
            "transform": {"id": sources.transform_id,
                          "cofactor": sources.transform["cofactor"],
                          "space": sources.transform["space"],
                          "unit": sources.transform["unit"]},
            "gate_set": {"id": sources.gates_id,
                         "space": sources.gates["space"],
                         "unit": sources.gates["axes_unit"]},
        },
        "channels": channels,
        "comp": comp_block,
        "events": events_out,
        "gates": sources.gates["gates"],
        "populations": populations,
        "gate_boundary": gate_boundary,
        "pending": pending,
    })
}

fn build_comp_block(
    sources: &SourceSet,
    analysis: &CompAnalysis,
    opt: &DeriveOptions,
) -> Value {
    let mut block = json!({
        "comp_matrix": sources.comp_id,
        "status": analysis.status,
        "rcond": analysis.rcond,
        "matrix": json!(sources.comp["matrix"]),
        "inverse": Value::Null,
        "candidates": analysis.candidates.iter().map(|c| json!({
            "id": c.id, "label": c.label, "inverse": round9(&c.inverse),
        })).collect::<Vec<_>>(),
        "resolved_candidate": Value::Null,
        "diagnostics": analysis.diagnostics,
    });
    if let Some(inv) = &analysis.inverse {
        block["inverse"] = round9(inv);
    }
    if analysis.status == "indeterminate" {
        if let Some(inv) = opt.chosen_inverse {
            block["inverse"] = round9(inv);
            block["resolved_candidate"] = json!(opt.chosen_candidate_id);
            block["diagnostics"]
                .as_array_mut()
                .unwrap()
                .push(json!(format!(
                    "人工裁定选择候选 {}；其余候选保留未采纳记录。",
                    opt.chosen_candidate_id.unwrap_or("?")
                )));
        }
    }
    block
}
