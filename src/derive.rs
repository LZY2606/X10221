//! 派生层：原始信号 → 补偿 → 变换 → 门成员 → 群体统计。
//! 派生结果分版本、不可变；过期只打标记，不用最新结果覆盖历史。
use crate::comp;
use crate::domain::{Candidate, DepRef};
use crate::polygon;
use crate::store::{DerivedRow, PopStatRow, Store};
use serde_json::json;

pub const COND_THRESHOLD: f64 = 1e12;
pub const GATE_EPS: f64 = 1e-6;

pub fn dep(kind: &str, id: &str, version: i64) -> DepRef {
    DepRef { kind: kind.into(), id: id.into(), version }
}

pub fn file_channels(store: &Store, file_id: &str) -> Vec<String> {
    store.list_fcs_params(file_id).map(|ps| ps.into_iter().map(|p| p.name).collect()).unwrap_or_default()
}

fn channel_index(channels: &[String], name: &str) -> Option<usize> {
    channels.iter().position(|c| c == name)
}

/// 计算补偿事件集。矩阵近奇异/奇异时不产出数值，只给候选，状态 blocked。
pub fn compute_compensated(
    store: &Store,
    file_id: &str,
    matrix: &comp::Matrix,
    matrix_ref: DepRef,
    event_id: i64,
) -> anyhow::Result<DerivedRow> {
    let channels = file_channels(store, file_id);
    let raw = store.raw_events(file_id)?;
    let derived_id = format!("derived:comp:{}", file_id);
    let version = store.latest_derived(&derived_id)?.map(|d| d.version + 1).unwrap_or(1);
    let idx: Vec<usize> = matrix.channels.iter().filter_map(|c| channel_index(&channels, c)).collect();
    let outcome = comp::invert(&matrix.rows, COND_THRESHOLD);
    if outcome.status != "ok" {
        // 近奇异：显式保存全部合法候选（阻尼伪逆结果 + 回退旧版本 + 拒绝），不默默求逆。
        let mut candidates = vec![
            Candidate {
                id: "use-previous-version".into(),
                label: "继续使用上一版补偿矩阵的结果".into(),
                detail: json!({"fallback": format!("derived:comp:{}@v{}", file_id, version - 1)}),
            },
            Candidate { id: "reject-compensation".into(), label: "拒绝本次补偿，相关群体保持 blocked".into(), detail: json!({}) },
        ];
        if let Some(inv_d) = comp::damped_inverse(&matrix.rows, 1e-6) {
            let vals: Vec<Vec<f64>> = raw
                .iter()
                .map(|ev| comp::compensate_row(&ev.values, &inv_d, &idx))
                .collect();
            candidates.insert(0, Candidate {
                id: "damped-pseudoinverse".into(),
                label: "阻尼伪逆（λ=1e-6），结果仅供审阅，不会自动采用".into(),
                detail: json!({"unit": "counts", "values": vals}),
            });
        }
        return Ok(DerivedRow {
            derived_id,
            version,
            kind: "compensated".into(),
            status: "blocked".into(),
            payload: json!({
                "file_id": file_id,
                "channels": channels,
                "unit": "counts",
                "basis": "spillover matrix M 的逆矩阵；补偿后 y = M^{-1} x",
                "judgment": {
                    "rule": "comp.near_singular.no_silent_inverse",
                    "status": outcome.status,
                    "cond_1": outcome.cond_1,
                    "min_pivot": outcome.min_pivot,
                    "threshold": COND_THRESHOLD,
                    "reason": outcome.reason,
                }
            }),
            deps: vec![matrix_ref, dep("file", file_id, 1)],
            candidates,
            event: event_id,
        });
    }
    let inv = outcome.inverse.unwrap();
    let values: Vec<Vec<f64>> = raw.iter().map(|ev| comp::compensate_row(&ev.values, &inv, &idx)).collect();
    Ok(DerivedRow {
        derived_id,
        version,
        kind: "compensated".into(),
        status: "ready".into(),
        payload: json!({
            "file_id": file_id,
            "channels": channels,
            "unit": "counts",
            "basis": "spillover matrix M 的逆矩阵；补偿后 y = M^{-1} x",
            "judgment": {"rule": "comp.inversion", "status": "ok", "cond_1": outcome.cond_1},
            "values": values,
        }),
        deps: vec![matrix_ref, dep("file", file_id, 1)],
        candidates: vec![],
        event: event_id,
    })
}

/// 计算变换事件集（asinh）。补偿集合 blocked 时本集合也 blocked，并继承候选。
pub fn compute_transformed(
    store: &Store,
    file_id: &str,
    xform: &comp::Transform,
    xform_ref: DepRef,
    comp_ref: DepRef,
    event_id: i64,
    branch_id: &str,
    ws: &crate::state::WorldState,
) -> anyhow::Result<DerivedRow> {
    let channels = file_channels(store, file_id);
    let derived_id = format!("derived:xform:{}", file_id);
    let version = store.latest_derived(&derived_id)?.map(|d| d.version + 1).unwrap_or(1);
    let comp_row = store.get_derived(&comp_ref.id, comp_ref.version)?
        .ok_or_else(|| anyhow::anyhow!("缺少补偿集合 {}@v{}", comp_ref.id, comp_ref.version))?;

    if comp_row.status == "blocked" {
        // 分支若已显式选择候选（阻尼伪逆），可继续；否则 blocked 并向上暴露候选。
        let subject = format!("{}@v{}", comp_row.derived_id, comp_row.version);
        if let Some(adj) = ws.active_adjudication(store, branch_id, &subject)? {
            if adj.decision == "damped-pseudoinverse" {
                return build_xform(store, file_id, xform, &comp_row, Some("damped-pseudoinverse"),
                    xform_ref, comp_ref, event_id, &derived_id, version, "ready");
            }
            if adj.decision == "reject-compensation" {
                return Ok(DerivedRow {
                    derived_id, version, kind: "transformed".into(), status: "blocked".into(),
                    payload: json!({"file_id": file_id, "blocked_by": subject,
                        "reason": "上游补偿被该分支拒绝（reject-compensation）"}),
                    deps: vec![xform_ref.clone(), comp_ref], candidates: vec![], event: event_id,
                });
            }
            if adj.decision == "use-previous-version" {
                if let Some(fallback) = adj.params["fallback"].as_str() {
                    return build_xform_with_fallback(store, file_id, xform, xform_ref, comp_ref,
                        event_id, &derived_id, version, fallback);
                }
            }
        }
        return Ok(DerivedRow {
            derived_id, version, kind: "transformed".into(), status: "blocked".into(),
            payload: json!({"file_id": file_id, "blocked_by": subject,
                "reason": "上游补偿集合 blocked，等待人工选择候选", "upstream_candidates": comp_row.candidates}),
            deps: vec![xform_ref, comp_ref], candidates: comp_row.candidates.clone(), event: event_id,
        });
    }
    build_xform(store, file_id, xform, &comp_row, None, xform_ref, comp_ref, event_id, &derived_id, version, "ready")
}

fn build_xform_with_fallback(
    store: &Store, file_id: &str, xform: &comp::Transform, xform_ref: DepRef, comp_ref: DepRef,
    event_id: i64, derived_id: &str, version: i64, fallback: &str,
) -> anyhow::Result<DerivedRow> {
    // fallback 形如 "derived:comp:file:fcs:x@v1"
    let (id_part, ver_part) = fallback.rsplit_once("@v").ok_or_else(|| anyhow::anyhow!("bad fallback"))?;
    let old_comp = store.get_derived(id_part, ver_part.parse()?)?
        .ok_or_else(|| anyhow::anyhow!("fallback 不存在: {}", fallback))?;
    build_xform(store, file_id, xform, &old_comp, Some("use-previous-version"),
        xform_ref, comp_ref, event_id, derived_id, version, "ready")
}

fn build_xform(
    store: &Store, file_id: &str, xform: &comp::Transform, comp_row: &DerivedRow,
    chosen: Option<&str>, xform_ref: DepRef, comp_ref: DepRef, event_id: i64,
    derived_id: &str, version: i64, status: &str,
) -> anyhow::Result<DerivedRow> {
    let channels = file_channels(store, file_id);
    let raw = store.raw_events(file_id)?;
    let n = raw.len();
    let mut comp_values: Vec<Vec<f64>> = comp_row
        .payload["values"]
        .as_array()
        .map(|a| a.iter().map(|row| row.as_array().unwrap().iter().map(|v| v.as_f64().unwrap_or(f64::NAN)).collect()).collect())
        .unwrap_or_else(|| raw.iter().map(|e| e.values.clone()).collect());
    if chosen == Some("damped-pseudoinverse") {
        let cand = comp_row.candidates.iter().find(|c| c.id == "damped-pseudoinverse");
        if let Some(vals) = cand.and_then(|c| c.detail["values"].as_array()) {
            comp_values = vals
                .iter()
                .map(|row| row.as_array().unwrap().iter().map(|v| v.as_f64().unwrap_or(f64::NAN)).collect())
                .collect();
        }
    }
    let fl: Vec<usize> = xform.channels.iter().filter_map(|c| channel_index(&channels, c)).collect();
    let mut values = comp_values.clone();
    for i in 0..n {
        for &c in &fl {
            values[i][c] = comp::asinh_transform(comp_values[i][c], xform.cofactor);
        }
    }
    Ok(DerivedRow {
        derived_id: derived_id.to_string(), version, kind: "transformed".into(),
        status: status.into(),
        payload: json!({
            "file_id": file_id, "channels": channels,
            "unit": json!({"fluorescence": "asinh(counts/cofactor)", "scatter": "counts raw passthrough"}),
            "basis": format!("asinh(x/{}) 仅作用于 {:?}；FSC/SSC 保持原始", xform.cofactor, xform.channels),
            "cofactor": xform.cofactor,
            "applied_channels": xform.channels,
            "upstream_choice": chosen,
            "values": values,
        }),
        deps: vec![xform_ref, comp_ref], candidates: vec![], event: event_id,
    })
}

/// 从变换集合取散点值（FL 已 asinh，FSC/SSC 原始透传）。
fn xform_values(store: &Store, xform_ref: &DepRef) -> anyhow::Result<Vec<Vec<f64>>> {
    let row = store.get_derived(&xform_ref.id, xform_ref.version)?
        .ok_or_else(|| anyhow::anyhow!("缺少变换集合 {}@v{}", xform_ref.id, xform_ref.version))?;
    let vals = row
        .payload["values"]
        .as_array()
        .map(|a| a.iter().map(|r| r.as_array().unwrap().iter().map(|v| v.as_f64().unwrap_or(f64::NAN)).collect()).collect())
        .unwrap_or_default();
    Ok(vals)
}

fn edge_distances(vertices: &[[f64; 2]], p: [f64; 2]) -> (f64, usize) {
    let n = vertices.len();
    let mut best = f64::MAX;
    let mut edge = 0;
    for i in 0..n {
        let d = crate::polygon::point_segment_distance(p, vertices[i], vertices[(i + 1) % n]);
        if d < best {
            best = d;
            edge = i;
        }
    }
    (best, edge)
}

/// 门成员集合：每个事件 inside/outside/boundary；边界点同时保留 include/exclude 候选。
pub fn compute_gate_membership(
    store: &Store, gate: &polygon::Gate, gate_ref: DepRef, xform_ref: DepRef,
    tree_ref: DepRef, event_id: i64,
) -> anyhow::Result<DerivedRow> {
    let derived_id = format!("derived:gate:{}", gate.gate_id);
    let version = store.latest_derived(&derived_id)?.map(|d| d.version + 1).unwrap_or(1);
    let xrow = store.get_derived(&xform_ref.id, xform_ref.version)?
        .ok_or_else(|| anyhow::anyhow!("缺少变换集合 {}@v{}", xform_ref.id, xform_ref.version))?;
    let channels: Vec<String> = xrow.payload["channels"]
        .as_array().unwrap().iter().map(|v| v.as_str().unwrap_or("").to_string()).collect();
    let xi = channel_index(&channels, &gate.x_channel)
        .ok_or_else(|| anyhow::anyhow!("门 {} 找不到通道 {}", gate.gate_id, gate.x_channel))?;
    let yi = channel_index(&channels, &gate.y_channel)
        .ok_or_else(|| anyhow::anyhow!("门 {} 找不到通道 {}", gate.gate_id, gate.y_channel))?;

    if xrow.status == "blocked" {
        return Ok(DerivedRow {
            derived_id, version, kind: "gate-membership".into(), status: "blocked".into(),
            payload: json!({"gate_id": gate.gate_id, "blocked_by": format!("{}@v{}", xform_ref.id, xform_ref.version)}),
            deps: vec![gate_ref, xform_ref, tree_ref], candidates: xrow.candidates.clone(), event: event_id,
        });
    }
    let values = xform_values(store, &xform_ref)?;
    let mut per_event = Vec::new();
    let mut boundary_rows = Vec::new();
    for (row, vals) in values.iter().enumerate() {
        let p = [vals[xi], vals[yi]];
        let m = polygon::classify(&gate.vertices, p, GATE_EPS);
        let (dist, edge) = edge_distances(&gate.vertices, p);
        let label = match m {
            polygon::Membership::Inside => "inside",
            polygon::Membership::Outside => "outside",
            polygon::Membership::Boundary => "boundary",
        };
        if m == polygon::Membership::Boundary {
            boundary_rows.push(row);
        }
        per_event.push(json!({
            "row": row, "membership": label, "distance": dist, "edge": edge,
            "point": [vals[xi], vals[yi]],
        }));
    }
    let candidates = if boundary_rows.is_empty() {
        vec![]
    } else {
        vec![
            Candidate { id: "include-boundary".into(), label: "边界点纳入该门".into(),
                detail: json!({"boundary_rows": boundary_rows, "rule": "gate.boundary.ambiguous",
                    "eps": GATE_EPS, "candidate_count_extra": boundary_rows.len()}) },
            Candidate { id: "exclude-boundary".into(), label: "边界点排除出该门".into(),
                detail: json!({"boundary_rows": boundary_rows, "rule": "gate.boundary.ambiguous",
                    "eps": GATE_EPS}) },
        ]
    };
    Ok(DerivedRow {
        derived_id, version, kind: "gate-membership".into(),
        status: if candidates.is_empty() { "ready".into() } else { "ambiguous".into() },
        payload: json!({
            "gate_id": gate.gate_id, "gate_version": gate.version,
            "x_channel": gate.x_channel, "y_channel": gate.y_channel,
            "basis": gate.basis, "eps": GATE_EPS,
            "per_event": per_event, "boundary_rows": boundary_rows,
        }),
        deps: vec![gate_ref, xform_ref, tree_ref], candidates, event: event_id,
    })
}

#[derive(Debug, Clone)]
pub struct TreeNode {
    pub gate_id: String,
    pub gate_version: i64,
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct TreeInfo {
    pub tree_id: String,
    pub version: i64,
    pub file_id: String,
    pub root: String,
    pub nodes: Vec<TreeNode>,
    pub edges: Vec<(String, String)>,
}

pub fn parse_tree(json: &serde_json::Value) -> TreeInfo {
    let nodes = json["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| TreeNode {
            gate_id: n["gate_id"].as_str().unwrap_or("").to_string(),
            gate_version: n["gate_version"].as_i64().unwrap_or(1),
            name: n["name"].as_str().unwrap_or("").to_string(),
        })
        .collect();
    let edges = json["edges"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| (e[0].as_str().unwrap_or("").to_string(), e[1].as_str().unwrap_or("").to_string()))
        .collect();
    TreeInfo {
        tree_id: json["tree_id"].as_str().unwrap_or("tree:main").to_string(),
        version: json["version"].as_i64().unwrap_or(1),
        file_id: json["file_id"].as_str().unwrap_or("").to_string(),
        root: json["root"].as_str().unwrap_or("").to_string(),
        nodes,
        edges,
    }
}

fn parent_of<'a>(tree: &'a TreeInfo, gate_id: &str) -> Option<&'a str> {
    tree.edges.iter().find(|(_, c)| c == gate_id).map(|(p, _)| p.as_str())
}

/// 沿整棵树计算群体统计。分母（denom_count）显式钉住父群 stat 的 id+version。
/// 返回 (stats, 每条 stat 的分母钉住判定 JSON)。
pub fn compute_popstats(
    store: &Store,
    ws: &crate::state::WorldState,
    branch_id: &str,
    tree: &TreeInfo,
    xform_ref: DepRef,
    event_id: i64,
) -> anyhow::Result<Vec<(PopStatRow, serde_json::Value)>> {
    let n_events = store.raw_events(&tree.file_id)?.len() as i64;
    let tree_ref = dep("tree", &tree.tree_id, tree.version);
    let mut out = Vec::new();
    // 父群接受的成员行集合（随遍历累积）
    let mut accepted: std::collections::HashMap<String, std::collections::BTreeSet<usize>> =
        std::collections::HashMap::new();

    for node in &tree.nodes {
        let stat_id = format!("popstat:{}:{}", node.gate_id, branch_id);
        let version = store.latest_popstat(&stat_id)?.map(|s| s.version + 1).unwrap_or(1);
        // 实际使用的门版本：当前可见的最新版本（树节点 gate_version 是编树时的记录）
        let gate_version = store
            .latest_version_num("gate", &node.gate_id)?
            .unwrap_or(node.gate_version);
        let gate_ref = dep("gate", &node.gate_id, gate_version);

        if node.gate_id == tree.root {
            let rows: std::collections::BTreeSet<usize> = (0..n_events as usize).collect();
            accepted.insert(node.gate_id.clone(), rows);
            out.push((
                PopStatRow {
                    stat_id: stat_id.clone(),
                    version,
                    gate_id: node.gate_id.clone(),
                    gate_version,
                    parent_stat_id: None,
                    parent_stat_version: None,
                    count: n_events,
                    denom_count: n_events,
                    pct: 1.0,
                    status: "ready".into(),
                    deps: vec![dep("file", &tree.file_id, 1), tree_ref.clone(), gate_ref],
                    event: event_id,
                },
                json!({
                    "rule": "popstat.denominator.pinned_to_parent_version",
                    "stat_id": stat_id, "version": version,
                    "denom_source": {"kind": "file", "id": tree.file_id, "version": 1, "count": n_events}
                }),
            ));
            continue;
        }

        let parent_gate = parent_of(tree, &node.gate_id)
            .ok_or_else(|| anyhow::anyhow!("门 {} 在树中找不到父节点", node.gate_id))?;
        let parent_stat_id = format!("popstat:{}:{}", parent_gate, branch_id);
        // 父群必须刚刚（或此前）计算过
        let parent_row = out
            .iter()
            .find(|(s, _)| s.stat_id == parent_stat_id)
            .map(|(s, _)| s.clone())
            .or_else(|| store.latest_popstat(&parent_stat_id).ok().flatten())
            .ok_or_else(|| anyhow::anyhow!("父群统计 {} 缺失", parent_stat_id))?;
        let parent_accepted = accepted
            .get(parent_gate)
            .cloned()
            .unwrap_or_default();

        let mem_id = format!("derived:gate:{}", node.gate_id);
        let mem = store
            .latest_derived(&mem_id)?
            .ok_or_else(|| anyhow::anyhow!("缺少门成员集合 {}", mem_id))?;
        let mut deps = vec![
            dep("gate", &node.gate_id, node.gate_version),
            tree_ref.clone(),
            xform_ref.clone(),
            dep("popstat", &parent_stat_id, parent_row.version),
            dep("derived", &mem_id, mem.version),
        ];

        if mem.status == "blocked" {
            out.push((
                PopStatRow {
                    stat_id: stat_id.clone(),
                    version,
                    gate_id: node.gate_id.clone(),
                    gate_version,
                    parent_stat_id: Some(parent_stat_id.clone()),
                    parent_stat_version: Some(parent_row.version),
                    count: 0,
                    denom_count: parent_row.count,
                    pct: 0.0,
                    status: "blocked".into(),
                    deps,
                    event: event_id,
                },
                json!({"rule": "popstat.denominator.pinned_to_parent_version",
                    "stat_id": stat_id, "version": version,
                    "denom_source": {"kind": "popstat", "id": parent_stat_id, "version": parent_row.version,
                                     "count": parent_row.count}}),
            ));
            accepted.insert(node.gate_id.clone(), std::collections::BTreeSet::new());
            continue;
        }

        let subject = format!("{}@v{}", mem_id, mem.version);
        let adj = ws.active_adjudication(store, branch_id, &subject)?;
        let decision = adj.as_ref().map(|a| a.decision.as_str());
        let per_event = mem.payload["per_event"].as_array().unwrap();
        let mut inside_set = std::collections::BTreeSet::new();
        let mut boundary_set = std::collections::BTreeSet::new();
        for e in per_event {
            let row = e["row"].as_u64().unwrap() as usize;
            match e["membership"].as_str().unwrap() {
                "inside" => { inside_set.insert(row); }
                "boundary" => { boundary_set.insert(row); }
                _ => {}
            }
        }
        let choose_include = decision == Some("include-boundary");
        let mut chosen: std::collections::BTreeSet<usize> = inside_set.clone();
        if choose_include {
            chosen.extend(boundary_set.iter().copied());
        }
        let final_set: std::collections::BTreeSet<usize> =
            chosen.intersection(&parent_accepted).copied().collect();
        accepted.insert(node.gate_id.clone(), final_set.clone());

        let count_in = inside_set.intersection(&parent_accepted).count() as i64;
        let count_bd = boundary_set.intersection(&parent_accepted).count() as i64;
        let candidates = if count_bd > 0 {
            vec![
                Candidate {
                    id: "include-boundary".into(),
                    label: format!("边界点纳入：{} / {}", count_in + count_bd, parent_row.count),
                    detail: json!({"count": count_in + count_bd, "pct": (count_in + count_bd) as f64 / parent_row.count.max(1) as f64}),
                },
                Candidate {
                    id: "exclude-boundary".into(),
                    label: format!("边界点排除：{} / {}", count_in, parent_row.count),
                    detail: json!({"count": count_in, "pct": count_in as f64 / parent_row.count.max(1) as f64}),
                },
            ]
        } else {
            vec![]
        };
        let status = if count_bd > 0 && decision.is_none() {
            "ambiguous"
        } else if count_bd > 0 {
            "ready"
        } else {
            "ready"
        };
        let count = final_set.len() as i64;
        if let Some(a) = adj.as_ref() {
            deps.push(dep("adjudication", &a.adj_id, 1));
        }
        out.push((
            PopStatRow {
                stat_id: stat_id.clone(),
                version,
                gate_id: node.gate_id.clone(),
                gate_version: node.gate_version,
                parent_stat_id: Some(parent_stat_id.clone()),
                parent_stat_version: Some(parent_row.version),
                count,
                denom_count: parent_row.count,
                pct: count as f64 / parent_row.count.max(1) as f64,
                status: status.into(),
                deps,
                event: event_id,
            },
            json!({
                "rule": "popstat.denominator.pinned_to_parent_version",
                "stat_id": stat_id, "version": version,
                "denom_source": {"kind": "popstat", "id": parent_stat_id,
                                 "version": parent_row.version, "count": parent_row.count},
                "boundary_events_in_parent": count_bd,
                "active_decision": decision.map(|s| s.to_string()),
                "adjudication_event": adj.as_ref().map(|a| a.event),
                "candidates": candidates,
            }),
        ));
    }
    Ok(out)
}

/// 失效报告：列出受影响对象与记录的事件 id。
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct InvalidationReport {
    pub affected_derived: Vec<(String, i64)>,
    pub affected_popstats: Vec<(String, i64)>,
    pub events: Vec<i64>,
}

/// 依赖驱动的失效：某个来源出现新版本后，把旧版本钉住的派生对象标记过期。
/// 只标记、不覆盖；沿依赖边递归（xform→门成员→群体统计；父群→子群）。
pub fn invalidate_entity(
    store: &Store,
    dep_kind: &str,
    dep_id: &str,
    new_version: i64,
    cause: &str,
) -> anyhow::Result<InvalidationReport> {
    let mut rep = InvalidationReport::default();
    let mut seen_d = std::collections::BTreeSet::new();
    let mut seen_p = std::collections::BTreeSet::new();
    let mut queue_d: Vec<(String, i64)> = Vec::new();
    let mut queue_p: Vec<(String, i64)> = Vec::new();

    for (did, dv, pinned) in store.derived_depending_on(dep_kind, dep_id)? {
        if pinned < new_version && seen_d.insert((did.clone(), dv)) {
            queue_d.push((did, dv));
        }
    }
    for (sid, sv, pinned) in store.popstats_depending_on(dep_kind, dep_id)? {
        if pinned < new_version && seen_p.insert((sid.clone(), sv)) {
            queue_p.push((sid, sv));
        }
    }

    while let Some((did, dv)) = queue_d.pop() {
        let eid = store.record_event(
            "derived.stale",
            &json!({
                "derived_id": did, "version": dv, "cause": cause,
                "changed_source": {"kind": dep_kind, "id": dep_id, "version": new_version},
                "rule": "derived.stale_marker_no_overwrite",
            }),
        )?;
        rep.events.push(eid);
        rep.affected_derived.push((did.clone(), dv));
        for (nid, nv, pinned) in store.derived_depending_on("derived", &did)? {
            if pinned == dv && seen_d.insert((nid.clone(), nv)) {
                queue_d.push((nid, nv));
            }
        }
        for (sid, sv, pinned) in store.popstats_depending_on("derived", &did)? {
            if pinned == dv && seen_p.insert((sid.clone(), sv)) {
                queue_p.push((sid, sv));
            }
        }
    }
    while let Some((sid, sv)) = queue_p.pop() {
        let eid = store.record_event(
            "popstat.stale",
            &json!({
                "stat_id": sid, "version": sv, "cause": cause,
                "changed_source": {"kind": dep_kind, "id": dep_id, "version": new_version},
                "rule": "popstat.stale_marker_no_overwrite",
            }),
        )?;
        rep.events.push(eid);
        rep.affected_popstats.push((sid.clone(), sv));
        // 父群统计过期 → 钉住该版本作为分母的子群统计一并过期
        for (cid, cv, pinned) in store.popstats_depending_on("popstat", &sid)? {
            if pinned == sv && seen_p.insert((cid.clone(), cv)) {
                queue_p.push((cid, cv));
            }
        }
    }

    let sum = store.record_event(
        "invalidation.propagated",
        &json!({
            "cause": cause,
            "source": {"kind": dep_kind, "id": dep_id, "new_version": new_version},
            "affected_derived": rep.affected_derived,
            "affected_popstats": rep.affected_popstats,
            "scope": "dependency-graph only; unrelated objects untouched",
        }),
    )?;
    rep.events.push(sum);
    Ok(rep)
}
