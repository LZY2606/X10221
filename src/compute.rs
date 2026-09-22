//! 从回放状态计算：补偿坐标、门控成员、群体结果与失效范围。

use crate::domain::{asinh_transform, point_in_polygon, CompAnalysis};
use crate::engine::{canonical_sha, State};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet};

/// 列内容指纹：只对指定荧光通道的原始值做哈希，
/// 这样修改 FITC 列不会让只依赖散色通道的 live 门失效。
pub fn column_fingerprint(state: &State, cols: &[String]) -> String {
    let mut v = json!({});
    for id in &state.order {
        if let Some(ev) = state.raw_events.get(id) {
            let mut picked = serde_json::Map::new();
            for c in cols {
                if let Some(x) = ev["raw"].get(c) {
                    picked.insert(c.clone(), x.clone());
                }
            }
            v[id] = Value::Object(picked);
        }
    }
    canonical_sha(&v)
}

pub fn active_comp_strategy(state: &State) -> Option<String> {
    let matrix = state.artifacts.get("comp_matrix:active")?;
    let eval_id = state
        .evals
        .iter()
        .find(|(_, e)| e["matrix_sha"] == matrix.sha256)
        .map(|(k, _)| k.clone())?;
    let strategy = state.decisions.get(&eval_id)?;
    // exact 之外的策略才需要显式应用；良态矩阵的坐标直接由算子表达。
    Some(strategy.clone())
}

#[allow(dead_code)]
fn _unused(state: &State) {
    let _ = state;
}

fn matrix_for(state: &State) -> Option<(Vec<String>, Vec<Vec<f64>>, CompAnalysis)> {
    let a = state.artifacts.get("comp_matrix:active")?;
    let channels = a.content["channels"].as_array()?.iter().map(|v| v.as_str().unwrap_or("").to_string()).collect::<Vec<_>>();
    let rows = a
        .content["rows"]
        .as_array()?
        .iter()
        .map(|r| r.as_array().unwrap().iter().map(|x| x.as_f64().unwrap()).collect())
        .collect::<Vec<_>>();
    let analysis: CompAnalysis = state
        .evals
        .values()
        .find(|e| e["matrix_sha"] == a.sha256)
        .and_then(|e| serde_json::from_value(e["analysis"].clone()).ok())?;
    Some((channels, rows, analysis))
}

/// 单个事件的评估坐标（散色=raw，荧光=变换后的 asinh 索引），含补偿诊断。
pub fn coordinates(
    state: &State,
    event_id: &str,
    strategy_override: Option<&str>,
) -> Option<BTreeMap<String, f64>> {
    let ev = state.raw_events.get(event_id)?;
    let mut coords = BTreeMap::new();
    for ch in &state.channels {
        let id = ch["id"].as_str()?;
        let raw = ev["raw"].get(id)?.as_f64()?;
        coords.insert(id.to_string(), raw);
    }
    if let Some((channels, _rows, analysis)) = matrix_for(state) {
        let chosen = strategy_override
            .map(|s| s.to_string())
            .or_else(|| active_comp_strategy(state));
        if let Some(strategy) = chosen {
            let cand = analysis.candidates.iter().find(|c| c.strategy == strategy && c.admissible);
            if let Some(cand) = cand {
                apply_candidate(&mut coords, state, ev, &channels, cand.operator.as_ref());
            }
        }
    }
    apply_transforms(&mut coords, state);
    Some(coords)
}

fn apply_candidate(
    coords: &mut BTreeMap<String, f64>,
    _state: &State,
    ev: &Value,
    channels: &[String],
    operator: Option<&Vec<Vec<f64>>>,
) {
    let y: Vec<f64> = channels.iter().filter_map(|c| ev["raw"].get(c).and_then(|v| v.as_f64())).collect();
    let x = match operator {
        None => y,
        Some(op) => (0..op.len())
            .map(|r| op[r].iter().zip(&y).map(|(a, b)| a * b).sum())
            .collect(),
    };
    for (c, v) in channels.iter().zip(x) {
        coords.insert(c.clone(), v);
    }
}

fn apply_transforms(coords: &mut BTreeMap<String, f64>, state: &State) {
    let Some(xform) = state.artifacts.get("transform:active") else { return };
    for item in xform.content.as_array().cloned().unwrap_or_default() {
        let ch = item["channel"].as_str().unwrap_or("");
        let cofactor = item["cofactor"].as_f64().unwrap_or(1000.0);
        if let Some(v) = coords.get(ch).copied() {
            coords.insert(ch.to_string(), asinh_transform(v, cofactor));
        }
    }
}

fn gates(state: &State) -> Vec<Value> {
    state
        .artifacts
        .get("gates:active")
        .and_then(|a| a.content.as_array().cloned())
        .unwrap_or_default()
}

fn links(state: &State) -> Vec<Value> {
    state
        .artifacts
        .get("links:active")
        .and_then(|a| a.content.as_array().cloned())
        .unwrap_or_default()
}

fn gate_map(gates: &[Value]) -> HashMap<String, Value> {
    gates.iter().map(|g| (g["id"].as_str().unwrap_or("").to_string(), g.clone())).collect()
}

fn gate_dep_columns(g: &Value) -> Vec<String> {
    g["axes"]
        .as_array()
        .map(|a| a.iter().map(|ax| ax["channel"].as_str().unwrap_or("").to_string()).collect())
        .unwrap_or_default()
}

fn in_gate(x: f64, y: f64, g: &Value) -> bool {
    let poly: Vec<[f64; 2]> = g["polygon"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|p| [p[0].as_f64().unwrap(), p[1].as_f64().unwrap()])
                .collect()
        })
        .unwrap_or_default();
    let inclusive = g["boundary_rule"].as_str() == Some("inclusive");
    point_in_polygon(x, y, &poly, inclusive)
}

/// 依赖图：材料列 -> 受影响的门（沿父子关系向上传播）。
pub fn invalidation_scope(
    state: &State,
    changed_columns: &HashSet<String>,
) -> Vec<String> {
    let gs = gates(state);
    let ls = links(state);
    let mut directly: HashSet<String> = HashSet::new();
    for g in &gs {
        let cols: HashSet<String> = gate_dep_columns(g).into_iter().collect();
        if !cols.is_disjoint(changed_columns) {
            directly.insert(g["id"].as_str().unwrap_or("").to_string());
        }
    }
    let mut affected = directly.clone();
    // 沿父->子边反复传播：任一祖先受影响则下游结果过期。
    let mut changed = true;
    while changed {
        changed = false;
        for l in &ls {
            let gate = l["gate"].as_str().unwrap_or("");
            let parent = l["parent"].as_str().unwrap_or("");
            if !parent.is_empty() && affected.contains(parent) && affected.insert(gate.to_string()) {
                changed = true;
            }
        }
    }
    let mut out: Vec<String> = affected.into_iter().map(|g| format!("pop-{g}")).collect();
    out.sort();
    out
}

/// 重算全部门。分母钉住父群结果：每个百分比携带 denominator_result_id
/// 与 denominator_event_id（父群结果由哪次重算事件产出）。
pub fn recompute_populations(
    state: &State,
    event_id: &str,
    strategy_override: Option<&str>,
) -> Value {
    let gs = gates(state);
    let ls = links(state);
    let gmap = gate_map(&gs);
    let total = state.order.len() as i64;
    let mut members: HashMap<String, HashSet<String>> = HashMap::new();
    let mut pops = Vec::new();
    let mut produced: HashMap<String, (String, i64)> = HashMap::new();

    for l in &ls {
        let gid = l["gate"].as_str().unwrap_or("").to_string();
        let parent = l["parent"].as_str().map(|s| s.to_string());
        let g = match gmap.get(&gid) {
            Some(g) => g.clone(),
            None => continue,
        };
        let dep_cols = gate_dep_columns(&g);
        let deps = json!({
            "raw_columns_fp": column_fingerprint(state, &dep_cols),
            "comp_matrix_sha": state.artifacts.get("comp_matrix:active").map(|a| a.sha256.clone()),
            "transform_sha": state.artifacts.get("transform:active").map(|a| a.sha256.clone()),
            "gate_sha": canonical_sha(&g),
            "parent_result": l["parent_result"].clone(),
            "columns": dep_cols,
        });
        let candidate_pool: Vec<&String> = parent
            .as_ref()
            .and_then(|p| members.get(p))
            .map(|s| s.iter().collect())
            .unwrap_or_else(|| state.order.iter().collect());
        let mut inside = HashSet::new();
        for eid in candidate_pool {
            if let Some(coord) = coordinates(state, eid, strategy_override) {
                let axes = g["axes"].as_array().cloned().unwrap_or_default();
                let x = axes.get(0).and_then(|a| coord.get(a["channel"].as_str().unwrap_or("")).copied());
                let y = axes.get(1).and_then(|a| coord.get(a["channel"].as_str().unwrap_or("")).copied());
                if let (Some(x), Some(y)) = (x, y) {
                    if in_gate(x, y, &g) {
                        inside.insert(eid.clone());
                    }
                }
            }
        }
        members.insert(gid.clone(), inside.clone());
        let count = inside.len() as i64;
        let (denom_count, denom_result, denom_event) = match &parent {
            None => (total, None, None),
            Some(p) => {
                let (deid, dc) = produced.get(p).cloned().unwrap_or((event_id.to_string(), 0));
                (dc, Some(format!("pop-{p}")), Some(deid))
            }
        };
        let percent = if denom_count == 0 { 0.0 } else { 100.0 * count as f64 / denom_count as f64 };
        let result_id = format!("pop-{gid}");
        produced.insert(gid.clone(), (event_id.to_string(), count));
        pops.push(json!({
            "result_id": result_id,
            "gate_logical": gid,
            "name": g["name"],
            "count": count,
            "percent": percent,
            "parent": parent,
            "denominator_result_id": denom_result,
            "denominator_event_id": denom_event,
            "denominator_count": denom_count,
            "members": inside.iter().collect::<Vec<_>>(),
            "deps": deps,
            "status": "current",
            "stale": false,
            "computed_event_id": event_id,
        }));
    }
    json!({ "populations": pops })
}

/// 溢出/超量程诊断：保留原值，不做不可逆裁剪。
pub fn overflow_diagnostics(state: &State) -> Vec<Value> {
    let mut out = Vec::new();
    for ch in &state.channels {
        let id = ch["id"].as_str().unwrap_or("");
        let max = ch["raw_range"].as_array().and_then(|a| a.get(1)).and_then(|v| v.as_f64());
        for eid in &state.order {
            if let Some(v) = state.raw_events.get(eid).and_then(|e| e["raw"].get(id)).and_then(|v| v.as_f64()) {
                if let Some(max) = max {
                    if v > max {
                        out.push(json!({
                            "event_id": eid, "channel": id, "value": v,
                            "unit": ch["raw_unit"], "range_max": max,
                            "diagnostic": "overflow: 超采集量程，保留原始值，不裁剪、不丢弃"
                        }));
                    }
                }
            }
        }
    }
    out
}
