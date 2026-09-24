//! 推导规则：
//! - 近奇异补偿矩阵不能被默默求逆（归一化行列式阈值守卫）
//! - 门的百分比分母必须钉住父群版本
//! - 修订上游门/参数后，下游结果标记过期而不覆盖
//! - 边界点按门的 edge_rule 判定并留下机器可读事件

use rusqlite::{params, Connection};
use serde_json::{json, Value};

use crate::db;

pub const NEAR_SINGULAR_EPS: f64 = 1e-6;

/// 归一化行列式：|det| / ∏‖行‖。越接近 0 越病态。
pub fn normalized_det(m: &[Vec<f64>]) -> f64 {
    let n = m.len();
    let mut a: Vec<Vec<f64>> = m.to_vec();
    let mut det = 1.0f64;
    for col in 0..n {
        let mut piv = col;
        for r in col..n {
            if a[r][col].abs() > a[piv][col].abs() {
                piv = r;
            }
        }
        if a[piv][col].abs() < 1e-300 {
            return 0.0;
        }
        if piv != col {
            a.swap(piv, col);
            det = -det;
        }
        det *= a[col][col];
        for r in (col + 1)..n {
            let f = a[r][col] / a[col][col];
            for c in col..n {
                a[r][c] -= f * a[col][c];
            }
        }
    }
    let norm: f64 = m
        .iter()
        .map(|row| row.iter().map(|x| x * x).sum::<f64>().sqrt())
        .product();
    if norm == 0.0 {
        return 0.0;
    }
    det.abs() / norm
}

pub fn is_near_singular(m: &[Vec<f64>]) -> bool {
    normalized_det(m) < NEAR_SINGULAR_EPS
}

/// 阻尼最小二乘逆（2x2）：(AᵀA + λI)⁻¹Aᵀ。仅供裁定采纳后显式调用。
pub fn damped_inverse_2x2(m: &[Vec<f64>], lambda: f64) -> Vec<Vec<f64>> {
    let (a, b, c, d) = (m[0][0], m[0][1], m[1][0], m[1][1]);
    let ata00 = a * a + c * c + lambda;
    let ata01 = a * b + c * d;
    let ata11 = b * b + d * d + lambda;
    let det = ata00 * ata11 - ata01 * ata01;
    let inv00 = ata11 / det;
    let inv01 = -ata01 / det;
    let inv11 = ata00 / det;
    // inv(ATA) * Aᵀ
    vec![
        vec![inv00 * a + inv01 * b, inv00 * c + inv01 * d],
        vec![inv01 * a + inv11 * b, inv01 * c + inv11 * d],
    ]
}

/// 精确逆（2x2），仅在通过奇异性守卫后允许。
pub fn invert_2x2(m: &[Vec<f64>]) -> Option<Vec<Vec<f64>>> {
    let det = m[0][0] * m[1][1] - m[0][1] * m[1][0];
    if det.abs() < 1e-300 {
        return None;
    }
    Some(vec![
        vec![m[1][1] / det, -m[0][1] / det],
        vec![-m[1][0] / det, m[0][0] / det],
    ])
}

/// 射线法点在多边形内判定；同时报告是否落在边上（边界点）。
pub fn point_in_polygon(x: f64, y: f64, poly: &[[f64; 2]]) -> (bool, bool) {
    let mut inside = false;
    let mut on_edge = false;
    let n = poly.len();
    for i in 0..n {
        let (x1, y1) = (poly[i][0], poly[i][1]);
        let (x2, y2) = (poly[(i + 1) % n][0], poly[(i + 1) % n][1]);
        // 边界检测：点到线段距离 ≈ 0 且投影在线段内
        let dx = x2 - x1;
        let dy = y2 - y1;
        let len2 = dx * dx + dy * dy;
        if len2 > 0.0 {
            let t = ((x - x1) * dx + (y - y1) * dy) / len2;
            if (0.0..=1.0).contains(&t) {
                let px = x1 + t * dx;
                let py = y1 + t * dy;
                if (x - px).abs() < 1e-9 && (y - py).abs() < 1e-9 {
                    on_edge = true;
                }
            }
        }
        if (y1 > y) != (y2 > y) && x < (x2 - x1) * (y - y1) / (y2 - y1) + x1 {
            inside = !inside;
        }
    }
    (inside, on_edge)
}

pub fn matrix_from_json(v: &Value) -> Vec<Vec<f64>> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|r| r.as_array().unwrap().iter().map(|x| x.as_f64().unwrap()).collect())
        .collect()
}

/// 评估补偿：近奇异 → 拒绝静默求逆，产出候选与机器可读事件。
pub fn evaluate_compensation(conn: &Connection, branch: &str) -> rusqlite::Result<Value> {
    let (spill_ver, spill_id, spill) = db::latest(conn, "spillover")?
        .ok_or_else(|| rusqlite::Error::InvalidQuery)?;
    let matrix = matrix_from_json(&spill["matrix"]);
    let ndet = normalized_det(&matrix);
    if is_near_singular(&matrix) {
        let cand_id = db::new_id(conn, "cand")?;
        let seq = db::next_seq(conn, "global")?;
        let options = json!([
            {"option": "damped_pseudoinverse", "lambda": 1e-3,
             "desc": "阻尼最小二乘逆，须审阅者显式采纳"},
            {"option": "uncompensated_fallback", "desc": "退回未补偿数据继续审阅"},
            {"option": "request_new_matrix", "desc": "要求重新测定单染对照"}
        ]);
        conn.execute(
            "INSERT INTO candidates(id,subject,options,status,branch,created_seq)
             VALUES(?1,?2,?3,'pending',?4,?5)",
            params![cand_id, "comp:near-singular", options.to_string(), branch, seq],
        )?;
        let (evt_seq, evt_id) = db::emit_event(
            conn,
            "comp.near_singular_rejected",
            branch,
            json!({
                "rule": "near_singular_no_silent_inverse",
                "spillover_id": spill_id,
                "spillover_version": spill_ver,
                "normalized_det": ndet,
                "threshold": NEAR_SINGULAR_EPS,
                "candidate_id": cand_id,
                "basis": "normalized_det = |det| / prod(row_norms)"
            }),
        )?;
        return Ok(json!({
            "status": "rejected_near_singular",
            "normalized_det": ndet,
            "candidate_id": cand_id,
            "event": {"seq": evt_seq, "id": evt_id}
        }));
    }
    let inv = invert_2x2(&matrix).expect("守卫已排除奇异");
    let ver = db::put_version(conn, "comp_matrix", "comp:auto", Some(&spill_id), &json!({
        "status": "ruled_auto", "matrix": inv, "detectors": spill["metadata"]["detectors"],
        "derived_from": format!("{}@v{}", spill_id, spill_ver)
    }), branch)?;
    let (evt_seq, evt_id) = db::emit_event(conn, "comp.applied", branch, json!({
        "comp_version": ver, "normalized_det": ndet
    }))?;
    Ok(json!({"status": "applied", "comp_version": ver, "event": {"seq": evt_seq, "id": evt_id}}))
}

struct NormEvent {
    id: String,
    values: std::collections::HashMap<String, f64>,
}

/// 派生层规范化：原始表示已在导入时保留；此处解析失败只附诊断，不改原值。
fn normalize_events(conn: &Connection, raw: &Value) -> rusqlite::Result<Vec<NormEvent>> {
    let mut out = Vec::new();
    for ev in raw["events"].as_array().unwrap() {
        let id = ev["id"].as_str().unwrap().to_string();
        let mut values = std::collections::HashMap::new();
        for (ch, cell) in ev["values"].as_object().unwrap() {
            let raw_str = cell["raw"].as_str().unwrap();
            match raw_str.parse::<f64>() {
                Ok(v) => {
                    values.insert(ch.clone(), v);
                }
                Err(_) => {
                    db::add_diagnostic(
                        conn,
                        &format!("{}:{}", id, ch),
                        raw_str,
                        "无法转换为数值；原值保留在 raw_channels 存储中，派生计算跳过该通道",
                    )?;
                }
            }
        }
        out.push(NormEvent { id, values });
    }
    Ok(out)
}

fn gate_pass(gate: &Value, ev: &NormEvent) -> (bool, bool) {
    if let Some(poly) = gate.get("polygon") {
        let chs = gate["channels"].as_array().unwrap();
        let (c1, c2) = (chs[0].as_str().unwrap(), chs[1].as_str().unwrap());
        let (Some(&x), Some(&y)) = (ev.values.get(c1), ev.values.get(c2)) else {
            return (false, false);
        };
        let poly: Vec<[f64; 2]> = poly
            .as_array()
            .unwrap()
            .iter()
            .map(|p| [p[0].as_f64().unwrap(), p[1].as_f64().unwrap()])
            .collect();
        let (inside, on_edge) = point_in_polygon(x, y, &poly);
        let pass = if on_edge {
            gate["edge_rule"].as_str().unwrap_or("exclusive") == "inclusive"
        } else {
            inside
        };
        (pass, on_edge)
    } else {
        let t = &gate["threshold"];
        let ch = t["channel"].as_str().unwrap();
        let Some(&v) = ev.values.get(ch) else {
            return (false, false);
        };
        let limit = t["value"].as_f64().unwrap();
        let on_edge = (v - limit).abs() < 1e-9;
        let pass = match t["op"].as_str().unwrap() {
            ">=" => v >= limit,
            ">" => v > limit,
            _ => false,
        };
        (pass, on_edge)
    }
}

/// 计算群体：分母钉住父群版本；旧版本群体行永不覆盖。
pub fn compute_populations(conn: &Connection, branch: &str) -> rusqlite::Result<Value> {
    let (raw_ver, raw_id, raw) = db::latest(conn, "raw_channels")?
        .ok_or(rusqlite::Error::InvalidQuery)?;
    let (gates_ver, gates_id, gates_doc) = db::latest(conn, "gate_polygons")?
        .ok_or(rusqlite::Error::InvalidQuery)?;
    let (rels_ver, rels_id, rels_doc) = db::latest(conn, "gate_relations")?
        .ok_or(rusqlite::Error::InvalidQuery)?;
    let events = normalize_events(conn, &raw)?;
    let gates = gates_doc["gates"].as_array().unwrap().clone();
    let rels = rels_doc["relations"].as_array().unwrap().clone();

    let mut parent_of: std::collections::HashMap<String, String> = Default::default();
    for r in &rels {
        parent_of.insert(
            r["child"].as_str().unwrap().to_string(),
            r["parent"].as_str().unwrap().to_string(),
        );
    }
    // 拓扑序：根（无父）优先
    let mut order: Vec<&Value> = Vec::new();
    let mut remaining: Vec<&Value> = gates.iter().collect();
    while !remaining.is_empty() {
        let before = remaining.len();
        remaining.retain(|g| {
            let gid = g["gate_id"].as_str().unwrap();
            let ready = match parent_of.get(gid) {
                None => true,
                Some(p) => order.iter().any(|o| o["gate_id"].as_str().unwrap() == p),
            };
            if ready {
                order.push(g);
            }
            !ready
        });
        if remaining.len() == before {
            return Err(rusqlite::Error::InvalidQuery); // 环
        }
    }

    let mut membership: std::collections::HashMap<String, Vec<String>> = Default::default();
    let mut boundary_hits: Vec<Value> = Vec::new();
    let mut results: Vec<Value> = Vec::new();
    let mut pop_versions: std::collections::HashMap<String, i64> = Default::default();

    for gate in &order {
        let gid = gate["gate_id"].as_str().unwrap().to_string();
        let parent_members: Option<Vec<String>> = parent_of
            .get(&gid)
            .map(|p| membership.get(p).cloned().unwrap_or_default());
        let mut members = Vec::new();
        for ev in &events {
            if let Some(pm) = &parent_members {
                if !pm.contains(&ev.id) {
                    continue;
                }
            }
            let (pass, on_edge) = gate_pass(gate, ev);
            if on_edge {
                boundary_hits.push(json!({
                    "event_id": ev.id, "gate_id": gid,
                    "edge_rule": gate["edge_rule"].as_str().unwrap_or("exclusive"),
                    "counted": pass
                }));
            }
            if pass {
                members.push(ev.id.clone());
            }
        }
        membership.insert(gid.clone(), members.clone());

        let pop_id = format!("pop:{}", gid);
        let version: i64 = conn.query_row(
            "SELECT COALESCE(MAX(version),0)+1 FROM populations WHERE id=?1",
            params![pop_id],
            |r| r.get(0),
        )?;
        pop_versions.insert(gid.clone(), version);
        let (parent_pop_id, parent_pop_ver, denom, basis) = match parent_of.get(&gid) {
            Some(p) => {
                let ppid = format!("pop:{}", p);
                let pver = pop_versions.get(p).copied().unwrap_or(1);
                let pcount = membership.get(p).map(|m| m.len()).unwrap_or(0);
                (
                    Some(ppid.clone()),
                    Some(pver),
                    pcount,
                    format!("percent_of_parent@{}@v{}", ppid, pver),
                )
            }
            None => (
                None,
                None,
                events.len(),
                format!("percent_of_total@{}@v{}", raw_id, raw_ver),
            ),
        };
        let count = members.len();
        let percent = if denom > 0 {
            count as f64 / denom as f64 * 100.0
        } else {
            0.0
        };
        let seq = db::next_seq(conn, "global")?;
        conn.execute(
            "INSERT INTO populations(id,version,gate_id,gate_version,parent_pop_id,
               parent_pop_version,event_count,denominator_count,percent,status,basis,created_seq)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,'current',?10,?11)",
            params![
                pop_id, version, gid, gates_ver, parent_pop_id, parent_pop_ver,
                count as i64, denom as i64, percent, basis, seq
            ],
        )?;
        // 依赖：自身与全部祖先门 + 原始集 + 用到的荧光通道变换
        let mut dep_keys: Vec<(String, String, i64)> = vec![(
            "raw_channels".into(),
            raw_id.clone(),
            raw_ver,
        )];
        let mut cur = Some(gid.clone());
        while let Some(g) = cur {
            dep_keys.push(("gate_polygons".into(), format!("gate:{}", g.strip_prefix("gate:").unwrap_or(&g)), gates_ver));
            let gate_doc = gates.iter().find(|x| x["gate_id"].as_str().unwrap() == g).unwrap();
            for ch in gate_doc["channels"].as_array().unwrap() {
                let ch = ch.as_str().unwrap();
                if ch.starts_with("FL") {
                    dep_keys.push(("transform_params".into(), format!("xform:{}", ch), 0));
                }
            }
            cur = parent_of.get(&g).cloned();
        }
        for (store, key, ver) in dep_keys {
            conn.execute(
                "INSERT INTO deps(dependent_id,source_store,source_key,source_version) VALUES(?1,?2,?3,?4)",
                params![pop_id, store, key, ver],
            )?;
        }
        results.push(json!({
            "pop_id": pop_id, "version": version, "gate_id": gid,
            "count": count, "denominator": denom, "percent": percent, "basis": basis
        }));
    }
    let (evt_seq, evt_id) = db::emit_event(conn, "populations.computed", branch, json!({
        "raw": format!("{}@v{}", raw_id, raw_ver),
        "gates": format!("{}@v{}", gates_id, gates_ver),
        "relations": format!("{}@v{}", rels_id, rels_ver),
        "populations": results,
        "boundary_hits": boundary_hits,
    }))?;
    if !boundary_hits.is_empty() {
        db::emit_event(conn, "gate.boundary_hit", branch, json!({
            "rule": "edge_rule_applied", "hits": boundary_hits
        }))?;
    }
    Ok(json!({"populations": results, "event": {"seq": evt_seq, "id": evt_id}}))
}

/// 按依赖键把受影响群体标记为 stale（不覆盖旧行）。
fn mark_stale(
    conn: &Connection,
    store: &str,
    keys: &[String],
    new_version: i64,
    branch: &str,
    cause: &str,
) -> rusqlite::Result<Vec<String>> {
    let mut affected: Vec<String> = Vec::new();
    for key in keys {
        let mut stmt = conn.prepare(
            "SELECT DISTINCT dependent_id FROM deps
             WHERE source_store=?1 AND source_key=?2 AND source_version < ?3",
        )?;
        let ids: Vec<String> = stmt
            .query_map(params![store, key, new_version], |r| r.get(0))?
            .filter_map(|x| x.ok())
            .collect();
        for id in ids {
            conn.execute(
                "UPDATE populations SET status='stale' WHERE id=?1 AND status='current'",
                params![id],
            )?;
            if !affected.contains(&id) {
                affected.push(id);
            }
        }
    }
    db::emit_event(conn, "lineage.stale_marked", branch, json!({
        "cause": cause,
        "source_store": store,
        "source_keys": keys,
        "new_version": new_version,
        "affected": affected,
        "policy": "mark_stale_not_overwrite"
    }))?;
    Ok(affected)
}

/// 修订门（局部源改动）：只使依赖该门的下游群体过期。
pub fn revise_gate(
    conn: &Connection,
    gate_id: &str,
    new_threshold: Option<f64>,
    branch: &str,
) -> rusqlite::Result<Value> {
    let (old_ver, old_id, mut doc) = db::latest(conn, "gate_polygons")?
        .ok_or(rusqlite::Error::InvalidQuery)?;
    let gates = doc["gates"].as_array_mut().unwrap();
    let gate = gates
        .iter_mut()
        .find(|g| g["gate_id"].as_str().unwrap() == gate_id)
        .ok_or(rusqlite::Error::InvalidQuery)?;
    if let Some(t) = new_threshold {
        gate["threshold"]["value"] = json!(t);
    }
    let new_ver = db::put_version(conn, "gate_polygons", &old_id, Some(&old_id), &doc, branch)?;
    let affected = mark_stale(
        conn,
        "gate_polygons",
        &[gate_id.to_string()],
        new_ver,
        branch,
        &format!("gate.revised:{}", gate_id),
    )?;
    db::emit_event(conn, "gate.revised", branch, json!({
        "gate_id": gate_id, "old_version": old_ver, "new_version": new_ver,
        "new_threshold": new_threshold, "affected": affected
    }))?;
    Ok(json!({"gate_id": gate_id, "new_version": new_ver, "affected": affected}))
}

/// 修订变换参数：只使依赖该通道变换的派生过期。
pub fn revise_transform(
    conn: &Connection,
    channel: &str,
    new_w: f64,
    branch: &str,
) -> rusqlite::Result<Value> {
    let (old_ver, old_id, mut doc) = db::latest(conn, "transform_params")?
        .ok_or(rusqlite::Error::InvalidQuery)?;
    let mut found = false;
    for t in doc["transforms"].as_array_mut().unwrap() {
        if t["channel"].as_str().unwrap() == channel {
            t["params"]["W"] = json!(new_w);
            found = true;
        }
    }
    if !found {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let new_ver = db::put_version(conn, "transform_params", &old_id, Some(&old_id), &doc, branch)?;
    let affected = mark_stale(
        conn,
        "transform_params",
        &[format!("xform:{}", channel)],
        new_ver,
        branch,
        &format!("transform.revised:{}", channel),
    )?;
    db::emit_event(conn, "transform.revised", branch, json!({
        "channel": channel, "old_version": old_ver, "new_version": new_ver,
        "param": "W", "new_value": new_w, "affected": affected
    }))?;
    Ok(json!({"channel": channel, "new_version": new_ver, "affected": affected}))
}

/// 门嵌套改名：身份（gate_id）不变，子关系按 id 续接，不触发过期。
pub fn rename_gate(
    conn: &Connection,
    gate_id: &str,
    new_name: &str,
    branch: &str,
) -> rusqlite::Result<Value> {
    let (old_ver, old_id, mut doc) = db::latest(conn, "gate_polygons")?
        .ok_or(rusqlite::Error::InvalidQuery)?;
    let gates = doc["gates"].as_array_mut().unwrap();
    let gate = gates
        .iter_mut()
        .find(|g| g["gate_id"].as_str().unwrap() == gate_id)
        .ok_or(rusqlite::Error::InvalidQuery)?;
    let old_name = gate["name"].as_str().unwrap().to_string();
    gate["name"] = json!(new_name);
    let new_ver = db::put_version(conn, "gate_polygons", &old_id, Some(&old_id), &doc, branch)?;
    db::emit_event(conn, "gate.renamed", branch, json!({
        "gate_id": gate_id, "old_name": old_name, "new_name": new_name,
        "identity": "preserved", "old_version": old_ver, "new_version": new_ver,
        "note": "父子关系按 gate_id 续接，改名不产生过期"
    }))?;
    Ok(json!({"gate_id": gate_id, "old_name": old_name, "new_name": new_name}))
}

/// 人工裁定：接受/否定候选。同版本并发修改 → 冲突事件 + 双方差异。
pub fn apply_ruling(
    conn: &Connection,
    candidate_id: &str,
    decision: &Value,
    expected_seq: i64,
    branch: &str,
) -> rusqlite::Result<Result<Value, Value>> {
    let row = conn.query_row(
        "SELECT status, options, ruling, created_seq, decided_seq FROM candidates WHERE id=?1",
        params![candidate_id],
        |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, Option<i64>>(4)?,
            ))
        },
    );
    let Ok((status, options, ruling, created_seq, decided_seq)) = row else {
        return Ok(Err(json!({"error": "candidate_not_found", "candidate_id": candidate_id})));
    };
    if status != "pending" || decided_seq.is_some() || expected_seq < created_seq && decided_seq.is_some() {
        let attempted = decision.clone();
        let current: Value = ruling
            .as_deref()
            .map(|s| serde_json::from_str(s).unwrap())
            .unwrap_or(Value::Null);
        db::emit_event(conn, "ruling.conflict", branch, json!({
            "candidate_id": candidate_id,
            "reason": "concurrent_same_version_edit",
            "current_ruling": current,
            "attempted_ruling": attempted,
            "diff": {"status": status, "decided_seq": decided_seq, "expected_seq": expected_seq}
        }))?;
        return Ok(Err(json!({
            "conflict": true,
            "candidate_id": candidate_id,
            "current_ruling": current,
            "attempted_ruling": attempted,
            "message": "版本冲突：双方差异已记录为 ruling.conflict 事件"
        })));
    }
    let seq = db::next_seq(conn, "global")?;
    conn.execute(
        "UPDATE candidates SET status=?1, ruling=?2, decided_seq=?3, branch=?4 WHERE id=?5",
        params![
            if decision["action"].as_str().unwrap_or("") == "accept" { "accepted" } else { "rejected" },
            serde_json::to_string(decision).unwrap(),
            seq,
            branch,
            candidate_id
        ],
    )?;
    // 采纳阻尼伪逆：显式生成新 comp_matrix 版本（非静默）
    if decision["action"].as_str() == Some("accept")
        && decision["option"].as_str() == Some("damped_pseudoinverse")
    {
        let (_, spill_id, spill) = db::latest(conn, "spillover")?.unwrap();
        let matrix = matrix_from_json(&spill["matrix"]);
        let inv = damped_inverse_2x2(&matrix, 1e-3);
        let opts: Value = serde_json::from_str(&options).unwrap();
        let _ = opts;
        db::put_version(conn, "comp_matrix", "comp:damped", Some(&spill_id), &json!({
            "status": "ruled_damped_pseudoinverse", "lambda": 1e-3, "matrix": inv,
            "detectors": spill["metadata"]["detectors"],
            "ruled_by": candidate_id
        }), branch)?;
    }
    db::emit_event(conn, "ruling.applied", branch, json!({
        "candidate_id": candidate_id, "decision": decision, "expected_seq": expected_seq
    }))?;
    Ok(Ok(json!({"status": "applied", "candidate_id": candidate_id, "decided_seq": seq})))
}

/// 撤销裁定：恢复候选到 pending，记录事件（历史行不删除）。
pub fn undo_ruling(conn: &Connection, candidate_id: &str, branch: &str) -> rusqlite::Result<Value> {
    let ruling: Option<String> = conn.query_row(
        "SELECT ruling FROM candidates WHERE id=?1",
        params![candidate_id],
        |r| r.get(1_usize.saturating_sub(1)),
    ).unwrap_or(None);
    conn.execute(
        "UPDATE candidates SET status='pending', ruling=NULL, decided_seq=NULL WHERE id=?1",
        params![candidate_id],
    )?;
    db::emit_event(conn, "ruling.reverted", branch, json!({
        "candidate_id": candidate_id,
        "restored_to": "pending",
        "previous_ruling": ruling,
        "note": "撤销恢复前一分支；历史事件与版本行保留可回读"
    }))?;
    Ok(json!({"status": "reverted", "candidate_id": candidate_id}))
}
