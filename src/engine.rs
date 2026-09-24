//! 事件折叠引擎。状态完全由有序事件决定，因此回到任意旧事件即恢复当时的
//! 依赖版本与可见结论；上游版本变化只产生「过期」标记，绝不覆盖旧结论。

use crate::model::*;
use serde_json::{json, Value};

pub mod actions;

pub fn fold(events: &[Event]) -> State {
    let mut st = State::default();
    st.branches.push("main".to_string());

    for ev in events {
        match ev.kind {
            EventKind::CaseImported => {
                st.case = ev.payload.get("case").cloned();
            }
            EventKind::BranchCreated => {
                if let Some(b) = ev.payload.get("branch").and_then(|v| v.as_str()) {
                    if !st.branches.contains(&b.to_string()) {
                        st.branches.push(b.to_string());
                        // 从父分支快照复制 head 指针；历史版本对象共享，身份不变。
                        if let Some(snap) = ev.payload.get("heads").and_then(|v| v.as_object()) {
                            for (id, ver) in snap {
                                if let Some(v) = ver.as_u64() {
                                    st.heads.insert((id.clone(), b.to_string()), v);
                                }
                            }
                        }
                        if let Some(snap) =
                            ev.payload.get("derived_heads").and_then(|v| v.as_object())
                        {
                            for (id, ver) in snap {
                                if let Some(v) = ver.as_u64() {
                                    st.derived_heads
                                        .insert((id.clone(), b.to_string()), v);
                                }
                            }
                        }
                    }
                }
            }
            EventKind::ArtifactCommitted => {
                if let Some(a) = ev.payload.get("artifact") {
                    if let Ok(artifact) = serde_json::from_value::<Artifact>(a.clone()) {
                        st.heads
                            .insert((artifact.id.clone(), artifact.branch.clone()), artifact.version);
                        st.artifacts.insert((artifact.id.clone(), artifact.version), artifact);
                    }
                }
            }
            EventKind::DerivationComputed => {
                if let Some(d) = ev.payload.get("derived") {
                    if let Ok(derived) = serde_json::from_value::<Derived>(d.clone()) {
                        st.derived_heads
                            .insert((derived.id.clone(), derived.branch.clone()), derived.version);
                        st.derived.insert((derived.id.clone(), derived.version), derived);
                    }
                }
            }
            EventKind::DiagnosticRaised => {
                if let Some(d) = ev.payload.get("diagnostic") {
                    if let Ok(diag) = serde_json::from_value::<Diagnostic>(d.clone()) {
                        st.diagnostics.insert(diag.id.clone(), diag);
                    }
                }
            }
            EventKind::Adjudicated => {
                let p = &ev.payload;
                if let Some(diag_id) = p.get("diagnostic").and_then(Value::as_str) {
                    if let Some(diag) = st.diagnostics.get_mut(diag_id) {
                        diag.status = DiagStatus::Resolved;
                        diag.resolution = p.get("decision").cloned();
                    }
                    if let Some(produced) = p.get("produced").and_then(Value::as_array) {
                        for item in produced {
                            let id = item[0].as_str().unwrap_or_default();
                            let version = item[1].as_u64().unwrap_or(1);
                            if id == "comp-events" || id.starts_with("pop:") {
                                st.derived_heads
                                    .insert((id.to_string(), ev.branch.clone()), version);
                            } else {
                                st.heads.insert((id.to_string(), ev.branch.clone()), version);
                            }
                        }
                    }
                }
            }
            EventKind::AdjudicationUndone => {
                let target = ev
                    .payload
                    .get("adjudication_event_id")
                    .and_then(Value::as_str)
                    .and_then(|id| events.iter().find(|e| e.id == id));
                if let Some(adj) = target {
                    if let Some(diag_id) = adj.payload.get("diagnostic").and_then(Value::as_str)
                    {
                        if let Some(diag) = st.diagnostics.get_mut(diag_id) {
                            diag.status = DiagStatus::Open;
                            diag.resolution = None;
                        }
                    }
                    if let Some(prev) = adj.payload.get("previous_heads").and_then(Value::as_object)
                    {
                        for (id, v) in prev {
                            let key = (id.clone(), adj.branch.clone());
                            if id == "comp-events" || id.starts_with("pop:") {
                                if let Some(ver) = v.as_u64() {
                                    st.derived_heads.insert(key, ver);
                                } else if v.is_null() {
                                    st.derived_heads.remove(&key);
                                }
                            } else if let Some(ver) = v.as_u64() {
                                st.heads.insert(key, ver);
                            } else if v.is_null() {
                                st.heads.remove(&key);
                            }
                        }
                    }
                }
            }
            EventKind::Invalidated => {
                if let Ok(inv) = serde_json::from_value::<Invalidation>(ev.payload.clone()) {
                    st.invalidations.push(inv);
                }
            }
            EventKind::ConflictRecorded => {
                if let Ok(c) = serde_json::from_value::<Conflict>(ev.payload.clone()) {
                    st.conflicts.push(c);
                }
            }
        }
    }

    mark_staleness(&mut st);
    st
}

/// 用依赖关系计算失效范围：派生结果钉住的任一输入在同分支上出现更新版本即过期；
/// 传播通过派生结果之间的引用自然具有传递性（不动点）。只标记，不覆盖。
fn mark_staleness(st: &mut State) {
    let ids: Vec<(String, u64, String, Vec<(String, u64)>, DerivedStatus)> = st
        .derived
        .values()
        .map(|d| (d.id.clone(), d.version, d.branch.clone(), d.inputs.clone(), d.status))
        .collect();

    loop {
        let mut changed = false;
        for (id, version, branch, inputs, _orig) in &ids {
            let d = st.derived.get_mut(&(id.clone(), *version)).unwrap();
            if d.status == DerivedStatus::Blocked {
                continue;
            }
            let mut stale = false;
            for (dep_id, dep_ver) in inputs {
                if let Some(h) = st.heads.get(&(dep_id.clone(), branch.clone())) {
                    if *h > *dep_ver {
                        stale = true;
                    }
                }
                if let Some(h) = st.derived_heads.get(&(dep_id.clone(), branch.clone())) {
                    if *h > *dep_ver {
                        stale = true;
                    }
                }
            }
            if stale && d.status == DerivedStatus::Valid {
                d.status = DerivedStatus::Stale;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
}

/// 递归 JSON 差异：输出机器可读的路径级差异，供冲突双方对照。
pub fn json_diff(a: &Value, b: &Value) -> Vec<Value> {
    let mut out = Vec::new();
    diff_at("$", a, b, &mut out);
    out
}

fn diff_at(path: &str, a: &Value, b: &Value, out: &mut Vec<Value>) {
    if a == b {
        return;
    }
    match (a, b) {
        (Value::Object(am), Value::Object(bm)) => {
            for k in am.keys().chain(bm.keys()).collect::<std::collections::BTreeSet<_>>() {
                let p = format!("{path}.{k}");
                match (am.get(k), bm.get(k)) {
                    (Some(av), Some(bv)) => diff_at(&p, av, bv, out),
                    (Some(av), None) => out.push(json!({"path": p, "side": "incoming", "value": av})),
                    (None, Some(bv)) => out.push(json!({"path": p, "side": "current", "value": bv})),
                    (None, None) => {}
                }
            }
        }
        _ => out.push(json!({"path": path, "incoming": a, "current": b})),
    }
}

/// 供 UI/导出使用的 JSON 视图：把元组键展开成 "id@version" / "id|branch" 字符串键。
pub fn state_json(st: &State) -> Value {
    let artifacts: Vec<Value> = st
        .artifacts
        .values()
        .map(|a| serde_json::to_value(a).unwrap_or(Value::Null))
        .collect();
    let heads: serde_json::Map<String, Value> = st
        .heads
        .iter()
        .map(|((id, b), v)| (format!("{id}|{b}"), json!(v)))
        .collect();
    let derived: Vec<Value> = st
        .derived
        .values()
        .map(|d| serde_json::to_value(d).unwrap_or(Value::Null))
        .collect();
    let derived_heads: serde_json::Map<String, Value> = st
        .derived_heads
        .iter()
        .map(|((id, b), v)| (format!("{id}|{b}"), json!(v)))
        .collect();
    json!({
        "case": st.case,
        "artifacts": artifacts,
        "heads": Value::Object(heads),
        "derived": derived,
        "derived_heads": Value::Object(derived_heads),
        "diagnostics": st.diagnostics.values().collect::<Vec<_>>(),
        "conflicts": st.conflicts,
        "invalidations": st.invalidations,
        "branches": st.branches,
    })
}
