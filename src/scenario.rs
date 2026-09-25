//! 场景引擎：六种固定情形 + 裁定/分支/重算/撤销/并发冲突。
//! 所有操作写追加式事件；过期只标记，不用新结果覆盖历史。
use crate::comp::{Matrix, Transform};
use crate::derive::{self, dep, parse_tree, TreeInfo};
use crate::polygon::Gate;
use crate::state::WorldState;
use crate::store::{AdjRow, Store};
use serde_json::json;

#[derive(Debug)]
pub struct Conflict {
    pub expected_head: i64,
    pub actual_head: i64,
    pub incoming: serde_json::Value,
    pub existing: serde_json::Value,
}

#[derive(Debug)]
pub struct ScenarioOutcome {
    pub message: String,
    pub detail: serde_json::Value,
}

fn entity_latest(store: &Store, kind: &str, id: &str) -> anyhow::Result<serde_json::Value> {
    store
        .latest_version(kind, id)?
        .map(|v| v.json)
        .ok_or_else(|| anyhow::anyhow!("{} {} 尚不存在", kind, id))
}

fn current_tree(store: &Store) -> anyhow::Result<(TreeInfo, i64, serde_json::Value)> {
    let v = store.latest_version("tree", "tree:main")?.ok_or_else(|| anyhow::anyhow!("tree:main 缺失"))?;
    Ok((parse_tree(&v.json), v.version, v.json))
}

fn current_matrix(store: &Store) -> anyhow::Result<Matrix> {
    let v = store.latest_version("matrix", "matrix:comp-main")?.ok_or_else(|| anyhow::anyhow!("matrix 缺失"))?;
    Ok(serde_json::from_value(v.json)?)
}
fn current_transform(store: &Store) -> anyhow::Result<Transform> {
    let v = store.latest_version("transform", "transform:asinh-main")?.ok_or_else(|| anyhow::anyhow!("transform 缺失"))?;
    Ok(serde_json::from_value(v.json)?)
}

pub const MAIN_FILE: &str = "file:fcs:sample-main";

/// 重新计算分支上的下游：补偿→变换→门成员→群体统计。
/// 过期行保留；新计算写入新版本。受阻断时下游保持 blocked。
pub fn recompute_branch(store: &Store, branch_id: &str, ws: &WorldState) -> anyhow::Result<serde_json::Value> {
    let matrix = current_matrix(store)?;
    let mv = store.latest_version_num("matrix", &matrix.matrix_id)?.unwrap_or(1);
    let xform = current_transform(store)?;
    let tv = store.latest_version_num("transform", &xform.transform_id)?.unwrap_or(1);
    let (tree, tree_v, _) = current_tree(store)?;
    let tree_ref = dep("tree", &tree.tree_id, tree_v);

    // 分支对补偿集合最新版本的裁定选择
    let comp_id = format!("derived:comp:{}", MAIN_FILE);
    let mut chosen_comp_ref: Option<(i64, i64)> = None; // (version, fallback_version)
    let latest_comp_v = store.latest_derived(&comp_id)?.map(|d| d.version).unwrap_or(1);
    let subject = format!("{}@v{}", comp_id, latest_comp_v);
    let adj = ws.active_adjudication(store, branch_id, &subject)?;

    let mut comp_ref = dep("derived", &comp_id, 1);
    if let Some(a) = adj.as_ref() {
        match a.decision.as_str() {
            "reject-compensation" => comp_ref.version = latest_comp_v, // blocked
            "use-previous-version" => {
                if let Some(fb) = a.params["fallback"].as_str() {
                    let (idp, verp) = fb.rsplit_once("@v").unwrap();
                    comp_ref = dep("derived", idp, verp.parse()?);
                }
            }
            "damped-pseudoinverse" => comp_ref.version = latest_comp_v,
            _ => {}
        }
        let _ = chosen_comp_ref;
    }
    // 若最新 comp ready，直接用最新
    if let Some(latest) = store.get_derived(&comp_id, latest_comp_v)? {
        if latest.status == "ready" {
            comp_ref = dep("derived", &comp_id, latest_comp_v);
        }
    }

    // 变换：始终写新版本（依赖 cofactor 当前版本；comp_ref 可能钉住旧 comp 版本）
    let xf = derive::compute_transformed(
        store, MAIN_FILE, &xform, dep("transform", &xform.transform_id, tv),
        comp_ref.clone(), 0, branch_id, ws,
    )?;
    let xf_evt = store.record_event("derived.computed", &json!({
        "derived_id": xf.derived_id, "version": xf.version, "status": xf.status,
        "deps": xf.deps, "kind": xf.kind, "branch": branch_id, "recompute": true,
    }))?;
    let mut xf = xf;
    xf.event = xf_evt;
    store.insert_derived(&xf)?;

    let xform_ref = dep("derived", &xf.derived_id, xf.version);

    // 门成员：树节点钉住的门版本
    for node in &tree.nodes {
        if node.gate_id == tree.root {
            continue;
        }
        let gj = entity_latest(store, "gate", &node.gate_id)?;
        let gate: Gate = serde_json::from_value(gj)?;
        let gv = store.latest_version_num("gate", &gate.gate_id)?.unwrap_or(node.gate_version);
        // 树节点记录门身份；实际评估使用当前可见的最新门版本（历史版本由 popstat 行钉住）
        let gj = store.get_version("gate", &node.gate_id, gv)?.unwrap().json;
        let gate: Gate = serde_json::from_value(gj)?;
        let mem = derive::compute_gate_membership(
            store, &gate, dep("gate", &gate.gate_id, gv),
            xform_ref.clone(), tree_ref.clone(), 0,
        )?;
        let e = store.record_event("derived.computed", &json!({
            "derived_id": mem.derived_id, "version": mem.version, "status": mem.status,
            "deps": mem.deps, "kind": mem.kind, "branch": branch_id, "recompute": true,
            "boundary_rows": mem.payload["boundary_rows"],
        }))?;
        let mut mem = mem;
        mem.event = e;
        store.insert_derived(&mem)?;
    }

    // 群体统计
    let stats = derive::compute_popstats(store, ws, branch_id, &tree, xform_ref.clone(), 0)?;
    let mut produced = Vec::new();
    for (st, pin) in stats {
        let e = store.record_event("popstat.computed", &json!({
            "stat_id": st.stat_id, "version": st.version, "gate_id": st.gate_id,
            "gate_version": st.gate_version, "count": st.count, "denom_count": st.denom_count,
            "pct": st.pct, "status": st.status, "parent_stat_id": st.parent_stat_id,
            "parent_stat_version": st.parent_stat_version, "deps": st.deps, "branch": branch_id,
            "recompute": true, "denominator_judgment": pin,
        }))?;
        let mut st = st;
        st.event = e;
        store.insert_popstat(&st)?;
        produced.push(json!({"stat_id": st.stat_id, "version": st.version, "count": st.count,
                             "denom": st.denom_count, "pct": st.pct, "status": st.status}));
    }
    Ok(json!({"branch": branch_id, "xform": {"id": xf.derived_id, "version": xf.version,
        "status": xf.status}, "popstats": produced}))
}

fn head_of(ws: &WorldState, branch: &str) -> i64 {
    ws.branches.get(branch).map(|b| b.head_event).unwrap_or(0)
}

pub fn create_branch(store: &Store, branch_id: &str, parent: &str, ws: &WorldState) -> anyhow::Result<i64> {
    if ws.branches.contains_key(branch_id) {
        anyhow::bail!("分支 {} 已存在", branch_id);
    }
    let base = head_of(ws, parent);
    let e = store.record_event("branch.created", &json!({
        "branch_id": branch_id, "parent": parent, "base_event": base,
    }))?;
    // 分叉：拷贝父分支的群体统计（身份换成新分支；父群分母引用改指向新分支自己的版本链）
    let tree_ids: Vec<String> = store.all_popstat_ids()?.into_iter()
        .filter(|s| s.ends_with(&format!(":{}", parent)))
        .map(|s| s[..s.rfind(':').unwrap()].to_string())
        .collect();
    for sid in &tree_ids {
        let new_sid = format!("{}:{}", sid, branch_id);
        if let Some(row) = store.latest_popstat(&format!("{}:{}", sid, parent))? {
            let mut nr = row.clone();
            nr.stat_id = new_sid.clone();
            nr.version = 1;
            if let Some(p) = nr.parent_stat_id.as_mut() {
                if p.ends_with(&format!(":{}", parent)) {
                    *p = format!("{}:{}", &p[..p.rfind(':').unwrap()], branch_id);
                }
            }
            nr.parent_stat_version = Some(1);
            for d in nr.deps.iter_mut() {
                if d.kind == "popstat" && d.id.ends_with(&format!(":{}", parent)) {
                    d.id = format!("{}:{}", &d.id[..d.id.rfind(':').unwrap()], branch_id);
                    d.version = 1;
                }
            }
            let ev = store.record_event("popstat.computed", &json!({
                "stat_id": nr.stat_id, "version": 1, "gate_id": nr.gate_id,
                "gate_version": nr.gate_version, "count": nr.count, "denom_count": nr.denom_count,
                "pct": nr.pct, "status": nr.status, "branch": branch_id, "branch_fork": true,
                "forked_from": {"branch": parent, "base_event": base},
            }))?;
            nr.event = ev;
            store.insert_popstat(&nr)?;
        }
    }
    let h = store.record_event("branch.head", &json!({"branch_id": branch_id, "head_event": e}))?;
    Ok(h)
}

pub fn adjudicate(
    store: &Store, branch_id: &str, subject_id: &str, decision: &str,
    params: serde_json::Value, expected_head: Option<i64>, label: &str,
) -> anyhow::Result<Result<ScenarioOutcome, Conflict>> {
    let events = store.all_events()?;
    let ws = crate::state::fold_events(&events);
    let actual = head_of(&ws, branch_id);
    if let Some(exp) = expected_head {
        if exp != actual {
            // 查找该分支上 head>exp 的既有裁定，作为冲突对方
            let existing = store
                .adjudications()?
                .into_iter()
                .filter(|a| a.branch_id == branch_id && a.event > exp)
                .last()
                .map(|a| json!({"adj_id": a.adj_id, "decision": a.decision,
                                "params": a.params, "event": a.event}))
                .unwrap_or(json!({"head_event": actual}));
            let incoming = json!({"branch_id": branch_id, "subject_id": subject_id,
                "decision": decision, "params": params, "label": label, "expected_head": exp});
            store.record_event("conflict.detected", &json!({
                "rule": "concurrency.same_branch_conflict",
                "branch_id": branch_id, "expected_head": exp, "actual_head": actual,
                "existing": existing, "incoming": incoming,
                "resolution": "后提交者不会覆盖；请刷新或另开分支后重试",
            }))?;
            return Ok(Err(Conflict { expected_head: exp, actual_head: actual, incoming, existing }));
        }
    }
    let e0 = store.record_event("adjudication.recorded", &json!({
        "branch_id": branch_id, "subject_id": subject_id, "decision": decision,
        "params": params, "label": label,
    }))?;
    let adj_id = format!("adj:{}", e0);
    store.insert_adjudication(&AdjRow {
        adj_id: adj_id.clone(),
        branch_id: branch_id.into(),
        subject_id: subject_id.into(),
        decision: decision.into(),
        params: params.clone(),
        event: e0,
    })?;
    let ws = crate::state::fold_events(&store.all_events()?);
    let rec = recompute_branch(store, branch_id, &ws)?;
    let last = store.record_event("branch.head", &json!({"branch_id": branch_id}))?;
    Ok(Ok(ScenarioOutcome {
        message: format!("分支 {} 已采纳候选 {}（{}），head={}", branch_id, subject_id, decision, last),
        detail: json!({"adj_id": adj_id, "expected_head": actual, "recomputed": rec}),
    }))
}

pub fn revert_adjudication(store: &Store, branch_id: &str, adj_id: &str) -> anyhow::Result<ScenarioOutcome> {
    let events = store.all_events()?;
    let ws = crate::state::fold_events(&events);
    if ws.superseded_adjs.contains(adj_id) {
        anyhow::bail!("裁定 {} 已撤销", adj_id);
    }
    store.record_event("adjudication.reverted", &json!({
        "adj_id": adj_id, "branch_id": branch_id,
        "rule": "adjudication.revertible.restores_previous_branch_state",
    }))?;
    let ws = crate::state::fold_events(&store.all_events()?);
    let rec = recompute_branch(store, branch_id, &ws)?;
    let last = store.record_event("branch.head", &json!({"branch_id": branch_id}))?;
    Ok(ScenarioOutcome {
        message: format!("分支 {} 已撤销 {}，恢复前一分支结论，head={}", branch_id, adj_id, last),
        detail: rec,
    })
}
