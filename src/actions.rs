//! 引擎动作：把领域操作翻译成有序事件。所有写操作都在单事务内完成，
//! 失败回滚；冲突在落库前检查，保证同一分支不会被同 base_version 的后提交悄悄覆盖。

use crate::comp::{self, InverseVerdict, Matrix};
use crate::engine::{fold, json_diff};
use crate::fcs;
use crate::gate::{self, Membership};
use crate::model::*;
use crate::store::Store;
use base64::Engine;
use rusqlite::Connection;
use serde_json::{json, Value};

#[derive(Debug)]
pub struct ActionError(pub String);
impl std::fmt::Display for ActionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl std::error::Error for ActionError {}
impl From<crate::store::StoreError> for ActionError {
    fn from(e: crate::store::StoreError) -> Self {
        ActionError(e.0)
    }
}
impl From<serde_json::Error> for ActionError {
    fn from(e: serde_json::Error) -> Self {
        ActionError(e.to_string())
    }
}

/// 提交冲突（同版本并发修改）：携带双方差异，不落库本次尝试。
#[derive(Debug, serde::Serialize)]
pub struct CommitConflict {
    pub code: &'static str,
    pub artifact: String,
    pub branch: String,
    pub base_version: u64,
    pub head_version: u64,
    pub current: Value,
    pub attempted: Value,
    pub diff: Vec<Value>,
}

fn load_events(tx: &Connection) -> Result<Vec<Event>, ActionError> {
    let mut stmt = tx
        .prepare("SELECT seq, id, ts, actor, branch, kind, payload FROM events ORDER BY seq")?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, Option<String>>(4)?,
            r.get::<_, String>(5)?,
            r.get::<_, String>(6)?,
        ))
    })?;
    let mut out = Vec::new();
    for item in rows {
        let (seq, id, ts, actor, branch, kind_s, payload_s) = item?;
        let kind = EventKind::from_str(&kind_s)
            .ok_or_else(|| ActionError(format!("未知事件类型 {kind_s}")))?;
        out.push(Event {
            seq: seq as u64,
            id,
            ts,
            actor,
            branch: branch.unwrap_or_else(|| "main".to_string()),
            kind,
            payload: serde_json::from_str(&payload_s)?,
        });
    }
    Ok(out)
}

fn fold_tx(tx: &Connection) -> Result<State, ActionError> {
    Ok(fold(&load_events(tx)?))
}

pub struct NewArtifact {
    pub id: String,
    pub kind: ArtifactKind,
    pub branch: String,
    pub payload: Value,
    pub raw_b64: Option<String>,
    pub base_version: Option<u64>,
}

/// 冲突预检：返回 (当前 head 版本, 当前 payload, diff)。
pub fn check_conflict(
    store: &mut Store,
    id: &str,
    branch: &str,
    base_version: Option<u64>,
    attempted: &Value,
) -> Option<CommitConflict> {
    let events = store.load_events().ok()?;
    let st = fold(&events);
    let head = st.head(id, branch);
    let head_version = head.map(|h| h.version).unwrap_or(0);
    let expected = base_version.unwrap_or(0);
    if expected != head_version {
        return Some(CommitConflict {
            code: "stale_base_version",
            artifact: id.to_string(),
            branch: branch.to_string(),
            base_version: expected,
            head_version,
            current: head.map(|h| h.payload.clone()).unwrap_or(Value::Null),
            attempted: attempted.clone(),
            diff: json_diff(
                attempted,
                &head.map(|h| h.payload.clone()).unwrap_or(Value::Null),
            ),
        });
    }
    None
}

/// 提交一个新版本，同时记录失效范围事件。
pub fn commit_artifact(
    store: &mut Store,
    a: NewArtifact,
    actor: &str,
) -> Result<Event, CommitConflict> {
    // 先在事务外读取一次（失败冲突也要留档 ConflictRecorded，由调用方决定）。
    store
        .transaction(|tx| {
            let st = fold_tx(tx).map_err(|e| ActionErrorTx::Inner(e.0))?;
            let head_version = st
                .heads
                .get(&(a.id.clone(), a.branch.clone()))
                .copied()
                .unwrap_or(0);
            let expected = a.base_version.unwrap_or(0);
            if expected != head_version {
                let head_payload = st
                    .head(&a.id, &a.branch)
                    .map(|h| h.payload.clone())
                    .unwrap_or(Value::Null);
                let diff = json_diff(&a.payload, &head_payload);
                return Err(ActionErrorTx::Conflict {
                    head_version,
                    current: head_payload,
                    diff,
                });
            }
            let next_version = head_version + 1;
            let artifact = Artifact {
                id: a.id.clone(),
                version: next_version,
                kind: a.kind,
                branch: a.branch.clone(),
                payload: a.payload.clone(),
                raw_b64: a.raw_b64.clone(),
                created_by: String::new(),
            };
            let ev = Store::append_event_tx(
                tx,
                EventKind::ArtifactCommitted,
                &a.branch,
                actor,
                json!({"artifact": artifact, "base_version": expected}),
            )
            .map_err(|e| ActionErrorTx::Inner(e.0))?;
            // created_by 回填
            tx.execute(
                "UPDATE artifacts SET created_by = ?1 WHERE id = ?2 AND version = ?3",
                rusqlite::params![ev.id, a.id, next_version as i64],
            )
            .map_err(|e| ActionErrorTx::Inner(e.to_string()))?;

            // 计算失效范围：旧 head 钉住的派生结果在新 head 出现后变过期。
            let affected = compute_affected(&st, &a.id, &a.branch);
            if !affected.is_empty() {
                Store::append_event_tx(
                    tx,
                    EventKind::Invalidated,
                    &a.branch,
                    actor,
                    json!({
                        "id": format!("inval-{:06}", ev.seq),
                        "cause_event": ev.id,
                        "cause_artifact": a.id,
                        "new_version": next_version,
                        "affected": affected,
                        "scope": "dependency-tracking",
                        "policy": "mark-stale-no-overwrite",
                    }),
                )
                .map_err(|e| ActionErrorTx::Inner(e.0))?;
            }
            Ok(ev)
        })
        .map_err(|e| match e {
            ActionErrorTx::Conflict { head_version, current, diff } => CommitConflict {
                code: "stale_base_version",
                artifact: a.id.clone(),
                branch: a.branch.clone(),
                base_version: expected_base(&a),
                head_version,
                current,
                attempted: a.payload.clone(),
                diff,
            },
            ActionErrorTx::Inner(msg) => CommitConflict {
                code: "internal_error",
                artifact: a.id.clone(),
                branch: a.branch.clone(),
                base_version: 0,
                head_version: 0,
                current: Value::Null,
                attempted: json!({"error": msg}),
                diff: vec![],
            },
        })
}

fn expected_base(a: &NewArtifact) -> u64 {
    a.base_version.unwrap_or(0)
}

enum ActionErrorTx {
    Inner(String),
    Conflict { head_version: u64, current: Value, diff: Vec<Value> },
}
impl From<rusqlite::Error> for ActionErrorTx {
    fn from(e: rusqlite::Error) -> Self {
        ActionErrorTx::Inner(e.to_string())
    }
}

/// 找到把给定 artifact 钉作输入、当前仍 Valid 的派生结果（含传递依赖的上层）。
fn compute_affected(st: &State, artifact_id: &str, branch: &str) -> Vec<String> {
    let mut affected = Vec::new();
    let mut frontier: Vec<String> = st
        .derived
        .values()
        .filter(|d| d.branch == branch && d.inputs.iter().any(|(id, _)| id == artifact_id))
        .map(|d| d.id.clone())
        .collect();
    while let Some(id) = frontier.pop() {
        if affected.contains(&id) {
            continue;
        }
        affected.push(id.clone());
        for up in st
            .derived
            .values()
            .filter(|d| d.branch == branch && d.inputs.iter().any(|(dep, _)| dep == &id))
        {
            frontier.push(up.id.clone());
        }
    }
    affected.sort();
    affected.dedup();
    affected
}

/// 记录一次并发冲突（第二个窗口保存时）。
pub fn record_conflict(
    store: &mut Store,
    branch: &str,
    actor: &str,
    c: &CommitConflict,
) -> Result<Event, ActionError> {
    let ev = store.append_event(
        EventKind::ConflictRecorded,
        branch,
        actor,
        json!({
            "id": format!("conflict-{}", c.artifact),
            "artifact": c.artifact,
            "branch": c.branch,
            "base_version": c.base_version,
            "head_version": c.head_version,
            "current_payload": c.current,
            "attempted_payload": c.attempted,
            "diff": c.diff,
            "resolved": false,
        }),
    )?;
    Ok(ev)
}

// ---------- 派生：补偿 ----------

fn st_get<'a>(st: &'a State, id: &str, branch: &str) -> Result<&'a Artifact, ActionError> {
    st.head(id, branch)
        .ok_or_else(|| ActionError(format!("缺少必需的版本化对象 {id}（分支 {branch}）")))
}

fn diag_id(code: &str, seq: u64) -> String {
    format!("diag-{code}-{seq:06}")
}

/// 尝试补偿：近奇异则不落 Derivation，只落 Diagnostic（全部候选）。
pub fn derive_compensation(store: &mut Store, branch: &str, actor: &str) -> Result<Event, ActionError> {
    store.transaction(|tx| derive_compensation_tx(tx, branch, actor))
}

fn derive_compensation_tx(tx: &Connection, branch: &str, actor: &str) -> Result<Event, ActionError> {
    {
        let mut st = fold_tx(tx)?;
        let raw = st_get(&st, "raw-channels", branch)?.clone();
        let matrix_art = st_get(&st, "comp-matrix", branch)?.clone();
        let spill = st_get(&st, "spillover-meta", branch)?.clone();
        let trans = st_get(&st, "transform-params", branch)?.clone();

        let matrix_vals: Vec<Vec<f64>> =
            serde_json::from_value(matrix_art.payload["matrix"].clone())?;
        let m = Matrix::new(matrix_vals);
        let channels: Vec<String> =
            serde_json::from_value(matrix_art.payload["channels"].clone())?;

        let seq = Store::append_event_tx_placeholder_seq(tx)?;
        match comp::invert_checked(&m) {
            InverseVerdict::NearSingular {
                condition,
                min_pivot_ratio,
                threshold,
                policy,
                candidates,
            } => {
                let id = diag_id("nearsingularcompmatrix", seq);
                // 幂等：同一矩阵版本已存在开放诊断则直接返回旧事件
                if let Some(existing) = st.diagnostics.values().find(|d| {
                    d.code == "NearSingularCompMatrix"
                        && d.status == DiagStatus::Open
                        && d.subject == format!("comp-matrix@{}", matrix_art.version)
                }) {
                    return find_event(tx, &existing.id);
                }
                // 已裁定：重派生时复用人工选择的候选（来源链可追溯，非默默求逆）
                if let Some(resolved) = st.diagnostics.values().find(|d| {
                    d.code == "NearSingularCompMatrix"
                        && d.status == DiagStatus::Resolved
                        && d.subject == format!("comp-matrix@{}", matrix_art.version)
                }) {
                    if let Some(cid) =
                        resolved.resolution.as_ref().and_then(|r| r["candidate"].as_str())
                    {
                        if let Some(cand) =
                            resolved.candidates.iter().find(|c| c["id"] == cid)
                        {
                            let chosen: Matrix = serde_json::from_value(cand["inverse"].clone())?;
                            return write_comp_derived(
                                tx, &mut st, branch, actor, raw, matrix_art, spill, trans,
                                &channels, &chosen, condition,
                                Some(format!("{}:{}", resolved.id, cid)),
                            );
                        }
                    }
                }
                let cand_json: Vec<Value> = candidates
                    .iter()
                    .map(|c| {
                        json!({
                            "id": c.id,
                            "label": c.label,
                            "method": c.method,
                            "param": c.param,
                            "inverse": c.inverse,
                            "residual": c.residual,
                        })
                    })
                    .collect();
                let ev = Store::append_event_tx(
                    tx,
                    EventKind::DiagnosticRaised,
                    branch,
                    actor,
                    json!({
                        "diagnostic": {
                            "id": id,
                            "code": "NearSingularCompMatrix",
                            "subject": format!("comp-matrix@{}", matrix_art.version),
                            "detail": {
                                "condition": condition,
                                "min_pivot_ratio": min_pivot_ratio,
                                "threshold": threshold,
                                "policy": policy,
                                "channels": channels,
                            },
                            "candidates": cand_json,
                            "status": "open",
                            "resolution": null,
                        }
                    }),
                )?;
                Ok(ev)
            }
            InverseVerdict::Ok { inverse, condition } => {
                let ev = write_comp_derived(
                    tx, &mut st, branch, actor, raw, matrix_art, spill, trans, &channels,
                    &inverse, condition, None,
                )?;
                Ok(ev)
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn write_comp_derived(
    tx: &Connection,
    st: &mut State,
    branch: &str,
    actor: &str,
    raw: &Artifact,
    matrix_art: &Artifact,
    spill: &Artifact,
    trans: &Artifact,
    channels: &[String],
    inverse: &Matrix,
    condition: f64,
    adopted_from: Option<String>,
) -> Result<Event, ActionError> {
    let case_rows = raw.payload["events"].as_array().cloned().unwrap_or_default();
    let channel_defs = raw.payload["channels"].as_array().cloned().unwrap_or_default();
    let col_idx: Vec<usize> = channels
        .iter()
        .map(|c| {
            channel_defs
                .iter()
                .position(|d| d["name"] == c.as_str())
                .unwrap_or(0)
        })
        .collect();

    let cofactors: Vec<f64> = channels
        .iter()
        .map(|c| {
            trans.payload["channels"]
                .get(c)
                .and_then(|v| v["cofactor"].as_f64())
                .unwrap_or(150.0)
        })
        .collect();

    let mut diagnostics = Vec::new();
    let mut rows = Vec::new();
    for (ei, row) in case_rows.iter().enumerate() {
        let vals: Vec<f64> = col_idx.iter().map(|&i| row[i]["raw_value"].as_f64().unwrap_or(0.0)).collect();
        let comp = comp::apply_row(inverse, &vals);
        // 极端信号：只在派生层标记，原始位置保留
        for (ci, c) in channels.iter().enumerate() {
            let range = channel_defs[col_idx[ci]]["range"].as_f64();
            let v = vals[ci];
            if v < 0.0 || range.map(|r| v > r).unwrap_or(false) {
                diagnostics.push(json!({
                    "event_index": ei, "channel": c, "value": v, "range": range,
                    "code": "ExtremeSignal",
                    "policy": "keep-in-place-exclude-from-normalized",
                    "unit": channel_defs[col_idx[ci]]["unit"],
                }));
            }
        }
        // 变换（派生层规范化）：极端值不参与 logicle 规范化
        let transformed: Vec<Value> = comp
            .iter()
            .enumerate()
            .map(|(ci, v)| {
                let raw_v = vals[ci];
                let range = channel_defs[col_idx[ci]]["range"].as_f64();
                let extreme = raw_v < 0.0 || range.map(|r| raw_v > r).unwrap_or(false);
                if extreme {
                    json!({"value": null, "basis": "untransformed-extreme", "raw_value": v})
                } else {
                    let t = (v / cofactors[ci]).asinh();
                    json!({
                        "value": t,
                        "basis": format!("logicle(cofactor={})", cofactors[ci]),
                        "raw_value": v,
                        "unit": format!("{} (compensated)", channel_defs[col_idx[ci]]["unit"]),
                    })
                }
            })
            .collect();
        let raw_all: Vec<f64> = row
            .as_array()
            .map(|a| a.iter().map(|q| q["raw_value"].as_f64().unwrap_or(0.0)).collect())
            .unwrap_or_default();
        rows.push(json!({
            "event_index": ei,
            "raw": vals,
            "raw_all": raw_all,
            "compensated": comp,
            "transformed": transformed,
        }));
    }

    // 无法转换的通道（单位 unknown）：留在原位置并附诊断，不丢弃
    for d in &channel_defs {
        if d["role"] == "unconvertible" {
            diagnostics.push(json!({
                "channel": d["name"], "unit": d["unit"], "code": "UnconvertibleChannel",
                "policy": "keep-raw-with-diagnostic",
            }));
        }
    }

    let next_version = st
        .derived_heads
        .get(&("comp-events".to_string(), branch.to_string()))
        .copied()
        .unwrap_or(0)
        + 1;
    let payload = json!({
        "channels": channels,
        "inverse": inverse,
        "matrix_version": matrix_art.version,
        "condition": condition,
        "adopted_from": adopted_from,
        "rows": rows,
        "diagnostics": diagnostics,
    });
    let derived = Derived {
        id: "comp-events".to_string(),
        version: next_version,
        kind: "compensated_events".to_string(),
        branch: branch.to_string(),
        inputs: vec![
            ("raw-channels".to_string(), raw.version),
            ("spillover-meta".to_string(), spill.version),
            ("comp-matrix".to_string(), matrix_art.version),
            ("transform-params".to_string(), trans.version),
        ],
        status: DerivedStatus::Valid,
        payload,
    };
    Store::append_event_tx(
        tx,
        EventKind::DerivationComputed,
        branch,
        actor,
        json!({"derived": derived}),
    )
}

fn find_event(tx: &Connection, diag_id: &str) -> Result<Event, ActionError> {
    let ev = tx.query_row(
        "SELECT seq, id, ts, actor, branch, kind, payload FROM events
         WHERE kind = 'diagnostic_raised'
           AND json_extract(payload, '$.diagnostic.id') = ?1",
        rusqlite::params![diag_id],
        |r| row_to_event(r),
    )?;
    Ok(ev)
}

pub fn row_to_event(r: &rusqlite::Row<'_>) -> rusqlite::Result<Event> {
    let kind_s: String = r.get(5)?;
    let payload_s: String = r.get(6)?;
    Ok(Event {
        seq: r.get::<_, i64>(0)? as u64,
        id: r.get(1)?,
        ts: r.get(2)?,
        actor: r.get(3)?,
        branch: r.get::<_, Option<String>>(4)?.unwrap_or_else(|| "main".into()),
        kind: EventKind::from_str(&kind_s).unwrap_or(EventKind::CaseImported),
        payload: serde_json::from_str(&payload_s).unwrap_or(Value::Null),
    })
}

// ---------- 派生：细胞群 ----------

fn derived_version(st: &State, id: &str, branch: &str) -> u64 {
    st.derived_heads
        .get(&(id.to_string(), branch.to_string()))
        .copied()
        .unwrap_or(0)
}

/// 计算细胞群：百分比分母钉住父群版本；边界点产生全部合法候选。
pub fn derive_populations(store: &mut Store, branch: &str, actor: &str) -> Result<Vec<Event>, ActionError> {
    store.transaction(|tx| derive_populations_tx(tx, branch, actor))
}

fn derive_populations_tx(tx: &Connection, branch: &str, actor: &str) -> Result<Vec<Event>, ActionError> {
    let st = fold_tx(tx)?;
    derive_populations_with_state(tx, st, branch, actor)
}

fn derive_populations_with_state(
    tx: &Connection,
    mut st: State,
    branch: &str,
    actor: &str,
) -> Result<Vec<Event>, ActionError> {
    {
        let comp = st
            .derived_head("comp-events", branch)
            .ok_or_else(|| ActionError("补偿事件尚未派生".into()))?
            .clone();
        let raw_channels = st_get(&st, "raw-channels", branch)?.clone();
        let channel_defs = raw_channels.payload["channels"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let transform_version = st_get(&st, "transform-params", branch)?.version;
        let hierarchy = st_get(&st, "gate-hierarchy", branch)?.clone();
        let order: Vec<String> = serde_json::from_value(hierarchy.payload["order"].clone())?;

        // 已采纳的边界裁定（只看本分支、且该裁定针对当前层级版本）
        let boundary_policy = |gate: &str| -> Option<String> {
            st.diagnostics.values().find_map(|d| {
                if d.code == "BoundaryMembership"
                    && d.status == DiagStatus::Resolved
                    && d.subject == gate
                    && d.detail["hierarchy_version"] == hierarchy.version
                {
                    d.resolution
                        .as_ref()
                        .and_then(|r| r.get("boundary_policy").and_then(|v| v.as_str()).map(String::from))
                } else {
                    None
                }
            })
        };

        // 上一轮种群版本（用于钉父群版本）
        let prev_heads: std::collections::BTreeMap<String, u64> = st
            .derived_heads
            .iter()
            .filter(|((id, b), _)| *b == branch && id.starts_with("pop:") && id != "comp-events")
            .map(|((id, _), v)| (id.clone(), *v))
            .collect();

        let mut out_events = Vec::new();
        // parent_members: gate -> 本轮确定在父门内的事件索引集合
        let mut populations: std::collections::BTreeMap<String, Value> = Default::default();
        let n_events = comp.payload["rows"].as_array().map(|a| a.len()).unwrap_or(0);
        let all: Vec<usize> = (0..n_events).collect();

        for gate_id in &order {
            if gate_id == "ALL" {
                continue;
            }
            let gate_art = st_get(&st, &format!("gate:{gate_id}"), branch)?.clone();
            let parent_gate: Option<String> =
                serde_json::from_value(gate_art.payload["parent"].clone())?;
            let poly: Vec<(f64, f64)> = serde_json::from_value::<Vec<[f64; 2]>>(
                gate_art.payload["polygon"].clone(),
            )?
            .into_iter()
            .map(|p| (p[0], p[1]))
            .collect();
            let basis_x = gate_art.payload["x_basis"].as_str().unwrap_or("linear");
            let basis_y = gate_art.payload["y_basis"].as_str().unwrap_or("linear");
            let xch = gate_art.payload["x_channel"].as_str().unwrap_or("");
            let ych = gate_art.payload["y_channel"].as_str().unwrap_or("");

            let scope = match &parent_gate {
                None => all.clone(),
                Some(p) => populations
                    .get(&format!("members:{p}"))
                    .and_then(|v| serde_json::from_value::<Vec<usize>>(v.clone()).ok())
                    .unwrap_or_default(),
            };

            let rows = comp.payload["rows"].as_array().cloned().unwrap_or_default();
            let mut inside: Vec<usize> = Vec::new();
            let mut boundary: Vec<usize> = Vec::new();
            let mut points_out: Vec<usize> = Vec::new();
            for ei in &scope {
                let p = read_point(
                    &rows[*ei],
                    &channel_defs,
                    xch,
                    ych,
                    basis_x,
                    basis_y,
                    &comp.payload["channels"],
                );
                match gate::classify(&poly, p) {
                    Membership::In => inside.push(*ei),
                    Membership::Boundary => boundary.push(*ei),
                    Membership::Out => points_out.push(*ei),
                }
            }

            let policy = boundary_policy(gate_id);
            let decided_members = match policy.as_deref() {
                Some("include") => {
                    let mut v = inside.clone();
                    v.extend(boundary.iter().copied());
                    v
                }
                Some("exclude") | Some(_) => inside.clone(),
                None => {
                    if boundary.is_empty() {
                        inside.clone()
                    } else {
                        // 无法唯一决定：不能默默归属，成员取确定在门内的，边界另存候选
                        inside.clone()
                    }
                }
            };
            populations.insert(format!("members:{gate_id}"), json!(decided_members));

            // 分母钉住父群版本
            let denominator = match &parent_gate {
                None => json!({
                    "gate": "ALL", "population_version": null,
                    "count": n_events,
                    "pinned": true,
                }),
                Some(p) => {
                    let parent_ver = prev_heads
                        .get(&format!("pop:{p}"))
                        .copied()
                        .unwrap_or_else(|| derived_version(&st, &format!("pop:{p}"), branch));
                    let parent_payload = st
                        .derived
                        .get(&(format!("pop:{p}"), parent_ver))
                        .map(|d| d.payload.clone())
                        .unwrap_or_else(|| populations.get(&format!("members:{p}")).cloned().unwrap_or(json!(null)));
                    let parent_count = parent_payload
                        .get("count")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(scope.len() as u64);
                    json!({
                        "gate": p,
                        "population_version": parent_ver,
                        "count": parent_count,
                        "pinned": true,
                    })
                }
            };

            let count_in = inside.len() as u64;
            let count_boundary = boundary.len() as u64;
            let denom_count = denominator["count"].as_u64().unwrap_or(1).max(1);
            let candidates = if boundary.is_empty() {
                vec![]
            } else {
                vec![
                    json!({"id": "cand-exclude", "boundary_policy": "exclude",
                           "count": count_in,
                           "fraction": count_in as f64 / denom_count as f64}),
                    json!({"id": "cand-include", "boundary_policy": "include",
                           "count": count_in + count_boundary,
                           "fraction": (count_in + count_boundary) as f64 / denom_count as f64}),
                ]
            };

            let needs_decision = boundary.is_empty() == false && policy.is_none();
            let status = if needs_decision {
                DerivedStatus::Blocked
            } else {
                DerivedStatus::Valid
            };
            let final_count = match policy.as_deref() {
                Some("include") => count_in + count_boundary,
                _ => count_in,
            };

            let next_version = derived_version(&st, &format!("pop:{gate_id}"), branch) + 1;
            let gate_name = gate_art.payload["name"].as_str().unwrap_or(gate_id).to_string();
            let inputs = vec![
                ("comp-events".to_string(), comp.version),
                ("gate-hierarchy".to_string(), hierarchy.version),
                (format!("gate:{gate_id}"), gate_art.version),
                ("transform-params".to_string(), transform_version),
            ];
            let payload = json!({
                "gate": gate_id,
                "gate_name": gate_name,
                "parent": parent_gate,
                "count": final_count,
                "count_definite_in": count_in,
                "count_boundary": count_boundary,
                "boundary_event_indices": boundary,
                "outside_event_indices": points_out,
                "member_event_indices": decided_members,
                "denominator": denominator,
                "fraction": final_count as f64 / denom_count as f64,
                "candidates": candidates,
                "coordinate_basis": {"x": basis_x, "y": basis_y,
                                     "x_channel": xch, "y_channel": ych},
                "unit": "event-count",
            });
            let derived = Derived {
                id: format!("pop:{gate_id}"),
                version: next_version,
                kind: "population".to_string(),
                branch: branch.to_string(),
                inputs,
                status,
                payload,
            };
            let ev = Store::append_event_tx(
                tx,
                EventKind::DerivationComputed,
                branch,
                actor,
                json!({"derived": derived}),
            )?;
            out_events.push(ev.clone());
            // 同事务内立即更新折叠视图，供后续门使用
            st.derived.insert((format!("pop:{gate_id}"), next_version), Derived {
                id: format!("pop:{gate_id}"),
                version: next_version,
                kind: "population".to_string(),
                branch: branch.to_string(),
                inputs: vec![],
                status,
                payload: payload.clone(),
            });
            st.derived_heads
                .insert((format!("pop:{gate_id}"), branch.to_string()), next_version);

            if needs_decision {
                let seq = Store::next_seq(tx)?;
                Store::append_event_tx(
                    tx,
                    EventKind::DiagnosticRaised,
                    branch,
                    actor,
                    json!({"diagnostic": {
                        "id": diag_id("boundarymembership", seq),
                        "code": "BoundaryMembership",
                        "subject": gate_id.clone(),
                        "detail": {
                            "hierarchy_version": hierarchy.version,
                            "gate_version": gate_art.version,
                            "boundary_event_indices": boundary,
                            "rule": "boundary-points-have-two-legal-memberships",
                        },
                        "candidates": candidates.clone(),
                        "status": "open",
                        "resolution": null,
                    }}),
                )?;
            }
        }
        Ok(out_events)
    }
}

/// 从派生事件读取门坐标；补偿基准则读 compensated，线性基准则读原始 raw。
#[allow(clippy::too_many_arguments)]
fn read_point(
    row: &Value,
    channel_defs: &[Value],
    xch: &str,
    ych: &str,
    bx: &str,
    by: &str,
    comp_channels: &Value,
) -> (f64, f64) {
    let comp_list: Vec<String> =
        comp_channels.as_array().cloned().unwrap_or_default().iter()
            .map(|v| v.as_str().unwrap_or("").to_string()).collect();
    let one = |ch: &str, basis: &str| -> f64 {
        if let Some(ci) = comp_list.iter().position(|c| c == ch) {
            if basis.starts_with("compensated") {
                row["compensated"][ci].as_f64().unwrap_or(0.0)
            } else {
                row["raw"][ci].as_f64().unwrap_or(0.0)
            }
        } else {
            // 非补偿通道（散点）：从 raw-channels 索引取原始值
            let idx = channel_defs.iter().position(|d| d["name"] == ch).unwrap_or(0);
            row.get("raw_all")
                .and_then(|v| v.get(idx))
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0)
        }
    };
    (one(xch, bx), one(ych, by))
}

// ---------- 人工裁定：接受 / 否定 / 参数调整；可撤销 ----------

pub struct Adjudication {
    pub diagnostic_id: String,
    /// accept | reject | adjust
    pub action: String,
    pub candidate_id: Option<String>,
    pub params: Option<Value>,
    pub rationale: String,
}

pub fn adjudicate(
    store: &mut Store,
    branch: &str,
    actor: &str,
    req: Adjudication,
) -> Result<Vec<Event>, ActionError> {
    store.transaction(|tx| {
        let st = fold_tx(tx)?;
        let diag = st
            .diagnostics
            .get(&req.diagnostic_id)
            .ok_or_else(|| ActionError(format!("诊断 {} 不存在", req.diagnostic_id)))?;
        if diag.status == DiagStatus::Resolved {
            return Err(ActionError(format!("诊断 {} 已裁定；请先撤销", req.diagnostic_id)));
        }

        let mut out = Vec::new();
        let mut produced: Vec<(String, u64)> = Vec::new();
        let decision;

        match diag.code.as_str() {
            "NearSingularCompMatrix" => {
                if req.action == "reject" {
                    decision = json!({
                        "action": "reject",
                        "rule": "near-singular-compensation",
                        "rationale": req.rationale,
                    });
                } else {
                    let cand = diag
                        .candidates
                        .iter()
                        .find(|c| c["id"] == req.candidate_id.as_deref().unwrap_or(""))
                        .ok_or_else(|| ActionError("请指定一个合法候选 id".into()))?;
                    let inverse: Matrix =
                        serde_json::from_value(cand["inverse"].clone())?;
                    let method = cand["method"].as_str().unwrap_or("tikhonov");
                    let param = cand["param"].clone();
                    decision = json!({
                        "action": req.action,
                        "candidate": cand["id"],
                        "method": method,
                        "param": param,
                        "params": req.params,
                        "rule": "near-singular-compensation",
                        "rationale": req.rationale,
                    });
                    let mut st2 = fold_tx(tx)?;
                    let raw = st_get(&st2, "raw-channels", branch)?;
                    let matrix_art = st_get(&st2, "comp-matrix", branch)?;
                    let spill = st_get(&st2, "spillover-meta", branch)?;
                    let trans = st_get(&st2, "transform-params", branch)?;
                    let channels: Vec<String> =
                        serde_json::from_value(matrix_art.payload["channels"].clone())?;
                    let ev = write_comp_derived(
                        tx,
                        &mut st2,
                        branch,
                        actor,
                        raw,
                        matrix_art,
                        spill,
                        trans,
                        &channels,
                        &inverse,
                        diag.detail["condition"].as_f64().unwrap_or(0.0),
                        Some(format!("{}:{}", diag.id, cand["id"].as_str().unwrap_or(""))),
                    )?;
                    produced.push(("comp-events".to_string(), extract_version(&ev)));
                    out.push(ev);
                }
            }
            "BoundaryMembership" => {
                if req.action == "reject" {
                    return Err(ActionError("边界成员必须在全部合法候选中选择，不能空过".into()));
                }
                let policy = match req.candidate_id.as_deref() {
                    Some("cand-include") => "include",
                    Some("cand-exclude") => "exclude",
                    other => return Err(ActionError(format!("非法边界候选 {other:?}"))),
                };
                decision = json!({
                    "action": "accept",
                    "candidate": req.candidate_id,
                    "boundary_policy": policy,
                    "rule": "boundary-membership",
                    "rationale": req.rationale,
                });
            }
            other => return Err(ActionError(format!("未知诊断类型 {other}"))),
        }

        // 记录 previous_heads，供撤销时恢复前一分支指针
        let mut previous = serde_json::Map::new();
        for (id, _v) in &produced {
            let h = st.derived_heads.get(&(id.clone(), branch.to_string())).copied();
            if let Some(v) = h {
                previous.insert(id.clone(), json!(v));
            } else {
                previous.insert(id.clone(), Value::Null);
            }
        }

        let adj = Store::append_event_tx(
            tx,
            EventKind::Adjudicated,
            branch,
            actor,
            json!({
                "diagnostic": req.diagnostic_id,
                "decision": decision,
                "produced": produced.iter().map(|(id, v)| json!([id, v])).collect::<Vec<_>>(),
                "previous_heads": Value::Object(previous),
            }),
        )?;
        out.push(adj);

        // 边界裁定后立即重算细胞群（被 Blocked 的门得到确定结论）
        if req.diagnostic_id.contains("boundarymembership") && req.action != "reject" {
            let mut st3 = fold_tx(tx)?;
            // 先在折叠视图上应用本裁定，确保 boundary_policy 可见
            if let Some(d) = st3.diagnostics.get_mut(&req.diagnostic_id) {
                d.status = DiagStatus::Resolved;
                d.resolution = out
                    .iter()
                    .find(|e| e.kind == EventKind::Adjudicated)
                    .and_then(|e| e.payload["decision"].as_object().cloned())
                    .map(Value::Object);
            }
            let evs = derive_populations_with_state(tx, st3, branch, actor)?;
            out.extend(evs);
        }
        Ok(out)
    })
}

fn extract_version(ev: &Event) -> u64 {
    ev.payload["derived"]["version"].as_u64().unwrap_or(0)
}

/// 撤销一次裁定：诊断回到 Open，head 指针恢复；派生历史保留可读，但不再是头版本。
pub fn undo_adjudication(
    store: &mut Store,
    branch: &str,
    actor: &str,
    adjudication_event_id: &str,
) -> Result<Event, ActionError> {
    store.transaction(|tx| {
        let st = fold_tx(tx)?;
        let _ = &st;
        let target = load_events(tx)?
            .iter()
            .find(|e| e.id == adjudication_event_id && e.kind == EventKind::Adjudicated)
            .cloned()
            .ok_or_else(|| ActionError("找不到指定的裁定事件".into()))?;
        // 已经撤销过则拒绝重复撤销
        let already: bool = load_events(tx)?.iter().any(|e| {
            e.kind == EventKind::AdjudicationUndone
                && e.payload["adjudication_event_id"] == adjudication_event_id
        });
        if already {
            return Err(ActionError("该裁定已经撤销".into()));
        }
        Store::append_event_tx(
            tx,
            EventKind::AdjudicationUndone,
            branch,
            actor,
            json!({"adjudication_event_id": target.id}),
        )
    })
}

/// 创建分支：复制父分支 head 指针（artifact/derived 版本对象共享），不复制字节。
pub fn create_branch(
    store: &mut Store,
    new_branch: &str,
    parent: &str,
    actor: &str,
) -> Result<Event, ActionError> {
    store.transaction(|tx| {
        let st = fold_tx(tx)?;
        let exists = load_events(tx)?.iter().any(|e| {
            e.kind == EventKind::BranchCreated
                && e.payload["branch"] == new_branch
        }) || new_branch == "main";
        if exists {
            return Err(ActionError(format!("分支 {new_branch} 已存在")));
        }
        let heads: serde_json::Map<String, Value> = st
            .heads
            .iter()
            .filter(|((_, b), _)| b == parent)
            .map(|((id, _), v)| (id.clone(), json!(v)))
            .collect();
        let derived_heads: serde_json::Map<String, Value> = st
            .derived_heads
            .iter()
            .filter(|((_, b), _)| b == parent)
            .map(|((id, _), v)| (id.clone(), json!(v)))
            .collect();
        Store::append_event_tx(
            tx,
            EventKind::BranchCreated,
            new_branch,
            actor,
            json!({
                "branch": new_branch,
                "parent": parent,
                "heads": Value::Object(heads),
                "derived_heads": Value::Object(derived_heads),
            }),
        )
    })
}
