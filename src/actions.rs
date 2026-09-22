//! 所有产生事件的操作：乐观锁、失效范围计算、机器可读判定依据。
use crate::engine::{stable_id, State};
use crate::evaluate::Eval;
use crate::model::*;
use crate::store::{tx, RowEvent};
use rusqlite::{params, Connection};
use serde_json::{json, Value};

pub struct AppError(pub String, pub u16);
impl From<rusqlite::Error> for AppError {
    fn from(e: rusqlite::Error) -> Self {
        AppError(e.to_string(), 500)
    }
}
impl From<serde_json::Error> for AppError {
    fn from(e: serde_json::Error) -> Self {
        AppError(e.to_string(), 400)
    }
}
impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

pub type AppResult<T> = Result<T, AppError>;

pub struct Attempt {
    pub branch: String,
    pub expected_rev: Option<i64>,
}

fn check_rev(t: &rusqlite::Transaction, at: &Attempt) -> AppResult<i64> {
    let tip = crate::store_ops::branch_tip(t, &at.branch)?;
    if let Some(exp) = at.expected_rev {
        if exp != tip {
            // 记录冲突观察（不写入分支账本，不污染历史结论；单独日志）
            t.execute(
                "INSERT INTO conflict_log(branch,observed_seq,expected_rev,attempted_rev,diff_json,at_unix)
                 VALUES(?1,?2,?3,?4,?5,strftime('%s','now'))",
                params![
                    at.branch,
                    tip,
                    exp,
                    tip + 1,
                    serde_json::to_string(&json!({
                        "expected": exp, "observed": tip,
                        "rule": "同版本并发修改：后提交者必须先看到双方差异",
                    }))
                    .unwrap(),
                ],
            )?;
            return Err(AppError(
                serde_json::to_string(&json!({
                    "error": "version-conflict",
                    "expected_rev": exp,
                    "current_rev": tip,
                    "message": "分支已被另一会话修改，请读回双方差异后重试",
                }))
                .unwrap(),
                409,
            ));
        }
    }
    Ok(tip)
}

/// 追加材料并自动登记诊断（奇异/近奇异、极端信号）与失效范围。
/// 整个操作在一个事务内，任何校验失败整体回滚。
pub fn append_material(
    conn: &Connection,
    at: &Attempt,
    material: Material,
) -> AppResult<Value> {
    validate_material(&material)?;
    tx(conn, |t| {
        let tip = check_rev(t, at)?;
        if !crate::store_ops::branch_exists(t, &at.branch) {
            return Err(AppError(format!("分支不存在: {}", at.branch), 404));
        }
        let mut seq = tip;
        let kind = material.kind.clone();
        let id = material.id.clone();
        let version = material.version;

        // 同一 (kind,id,version) 内容若一致即幂等；不一致则拒绝（不可覆盖来源）。
        if let Some(existing) = crate::store_ops::material_raw(t, &kind, &id, version)? {
            if existing != material.raw {
                return Err(AppError(
                    format!("{}:{}@v{} 已存在且内容不同；重新解释必须新建版本/分支", kind, id, version),
                    409,
                ));
            }
            return Ok(json!({"branch": at.branch, "rev": seq, "idempotent": true}));
        }

        crate::store_ops::insert_material(t, &material)?;
        seq += 1;
        let title = material.title.clone();
        crate::store_ops::insert_event(
            t,
            seq,
            &at.branch,
            evkind::IMPORT,
            &format!("{}:{}", kind, id),
            &json!({
                "kind": kind, "id": id, "version": version, "title": title,
                "name": material.raw.get("name").cloned().unwrap_or(Value::Null),
                "sha256": crate::store::sha256_hex(
                    serde_json::to_vec(&material.raw).unwrap_or_default()),
            }),
            &json!({
                "why": "版本化来源材料导入；来源与解释分开存储",
                "input_fragment": serde_json::to_string(&material.raw)
                    .map(|s| s.chars().take(400).collect::<String>())
                    .unwrap_or_default(),
            }),
            RULES_VERSION,
            Some((&kind, &id, version)),
            None,
        )?;

        // 重放到当前点，登记自动诊断
        let st = State::replay(t, &at.branch, Some(seq))?;
        if let Ok(ev) = Eval::build(t, st.clone()) {
            for d in &ev.diagnostics {
                seq += 1;
                crate::store_ops::insert_event(
                    t,
                    seq,
                    &at.branch,
                    evkind::VALUE_DIAG,
                    d.get("scope").and_then(|x| x.as_str()).unwrap_or("value"),
                    d,
                    &json!({
                        "rule": "无法转换的值留在原位置并附诊断，禁止为画图丢弃",
                        "rules_version": RULES_VERSION,
                    }),
                    RULES_VERSION,
                    None,
                    None,
                )?;
            }
            // 补偿诊断
            let comp_node = ev
                .nodes
                .values()
                .find(|n| n.node.kind == "compensation")
                .cloned();
            if let Some(cn) = comp_node {
                let cls = cn.data["classification"].as_str().unwrap_or("");
                if cls != "invertible" {
                    seq += 1;
                    crate::store_ops::insert_event(
                        t,
                        seq,
                        &at.branch,
                        evkind::COMP_DIAG,
                        cn.data["subject"].as_str().unwrap_or("compensation"),
                        &json!({
                            "classification": cls,
                            "determinant": cn.data["determinant"],
                            "condition_1": cn.data["condition_1"],
                            "rcond_1": cn.data["rcond_1"],
                            "threshold_rcond": cn.data["threshold_rcond"],
                            "candidate_ids": cn.data["candidates"]
                                .as_array().map(|a| a.iter()
                                    .map(|c| c["id"].clone()).collect::<Vec<_>>())
                                .unwrap_or_default(),
                            "affected_objects": affected_after_material(&kind, &id),
                            "input_fragment": json!({
                                "spillover_ref": cn.data["spillover_ref"],
                                "comp_ref": cn.basis["comp_ref"],
                            }),
                        }),
                        &json!({
                            "rule": "近奇异补偿矩阵不能被默默求逆；保留全部合法候选",
                            "rules_version": RULES_VERSION,
                        }),
                        RULES_VERSION,
                        Some((&kind, &id, version)),
                        None,
                    )?;
                }
            }
        }

        // 失效范围：与上一 tip 比较节点 cfg_hash，只给直接/间接下游打 stale
        let stale_events = compute_stale(t, &at.branch, tip, seq)?;
        for sub in stale_events {
            seq += 1;
            crate::store_ops::insert_event(
                t,
                seq,
                &at.branch,
                evkind::STALE,
                &sub.node,
                &json!({
                    "node_id": sub.node,
                    "old_cfg_hash": sub.old_hash,
                    "new_cfg_hash": sub.new_hash,
                    "cause_material": format!("{}:{}@v{}", kind, id, version),
                    "affected_scope": sub.scope,
                    "policy": "标记过期但不覆盖旧结果；接受新投影前历史结论保持可读",
                }),
                &json!({
                    "rule": "参数/参考版本变化后按依赖图计算失效范围，禁止全局替换旧结论",
                    "rules_version": RULES_VERSION,
                }),
                RULES_VERSION,
                None,
                None,
            )?;
        }

        Ok(json!({"branch": at.branch, "rev": seq}))
    })
}

fn validate_material(m: &Material) -> AppResult<()> {
    let check = |cond: bool, msg: &str| -> AppResult<()> {
        if cond {
            Ok(())
        } else {
            Err(AppError(msg.to_string(), 400))
        }
    };
    check(m.version >= 1, "材料 version 必须从 1 开始")?;
    check(!m.id.is_empty(), "材料 id 不能为空")?;
    match m.kind.as_str() {
        "events" => {
            let f: EventFile = serde_json::from_value(m.raw.clone())?;
            check(!f.channels.is_empty(), "events 缺少通道与单位声明")?;
            check(!f.events.is_empty(), "events 为空")?;
        }
        "spillover" => {
            let s: Spillover = serde_json::from_value(m.raw.clone())?;
            let n = s.channels.len();
            check(n > 0 && s.spillover.len() == n, "spillover 维度不一致")?;
            check(
                s.spillover.iter().all(|r| r.len() == n),
                "spillover 必须为方阵",
            )?;
        }
        "compensation" => {
            let c: Compensation = serde_json::from_value(m.raw.clone())?;
            check(!c.spillover_ref.id.is_empty(), "compensation 必须引用 spillover 版本")?;
        }
        "transform" => {
            let tf: Transform = serde_json::from_value(m.raw.clone())?;
            check(
                matches!(tf.method.as_str(), "linear" | "asinh" | "log10"),
                "transform.method 必须是 linear/asinh/log10",
            )?;
        }
        "gate" => {
            let g: Gate = serde_json::from_value(m.raw.clone())?;
            check(g.polygon.len() >= 3, "门多边形至少 3 个顶点")?;
        }
        "lineage" => {
            let l: Lineage = serde_json::from_value(m.raw.clone())?;
            check(!l.populations.is_empty(), "lineage 为空")?;
        }
        other => return Err(AppError(format!("未知材料类型 {}", other), 400)),
    }
    Ok(())
}

struct StaleSub {
    node: String,
    old_hash: String,
    new_hash: String,
    scope: Vec<String>,
}

/// 比较 before/after 两个重放点的节点 cfg_hash；只返回发生变化及其传递下游。
fn compute_stale(
    t: &rusqlite::Transaction,
    branch: &str,
    before_seq: i64,
    after_seq: i64,
) -> AppResult<Vec<StaleSub>> {
    let before = State::replay(t, branch, Some(before_seq))?;
    let after = State::replay(t, branch, Some(after_seq))?;
    let ev_before = Eval::build(t, before).ok();
    let ev_after = Eval::build(t, after).map_err(|e| AppError(e, 500))?;
    let mut changed: Vec<(String, String, String)> = vec![];
    if let Some(eb) = ev_before {
        for (id, nv) in &eb.nodes {
            if let Some(nv2) = ev_after.nodes.get(id) {
                if nv.node.cfg_hash != nv2.node.cfg_hash {
                    changed.push((id.clone(), nv.node.cfg_hash.clone(), nv2.node.cfg_hash.clone()));
                }
            }
        }
    }
    // 传播：依赖闭包
    let mut scope: std::collections::BTreeSet<String> = changed.iter().map(|x| x.0.clone()).collect();
    let mut growth = true;
    while growth {
        growth = false;
        for nv in ev_after.nodes.values() {
            if scope.contains(&nv.node.id) {
                continue;
            }
            if nv.node.deps.iter().any(|d| scope.contains(d)) {
                scope.insert(nv.node.id.clone());
                growth = true;
            }
        }
    }
    let mut out = vec![];
    let mut seen = std::collections::BTreeSet::new();
    for node in &scope {
        if seen.insert(node.clone()) {
            let (old, new) = changed
                .iter()
                .find(|x| x.0 == *node)
                .map(|x| (x.1.clone(), x.2.clone()))
                .unwrap_or_else(|| {
                    (
                        ev_before
                            .as_ref()
                            .and_then(|eb| eb.nodes.get(node).map(|n| n.node.cfg_hash.clone()))
                            .unwrap_or_else(|| "none".into()),
                        ev_after
                            .nodes
                            .get(node)
                            .map(|n| n.node.cfg_hash.clone())
                            .unwrap_or_else(|| "none".into()),
                    )
                });
            let affected: Vec<String> = ev_after
                .nodes
                .values()
                .filter(|nv| nv.node.id == *node || nv.node.deps.contains(node))
                .map(|nv| nv.node.id.clone())
                .collect();
            out.push(StaleSub { node: node.clone(), old_hash: old, new_hash: new, scope: affected });
        }
    }
    Ok(out)
}

/// 依赖图上某材料的直接影响对象（供诊断事件使用）。
fn affected_after_material(kind: &str, id: &str) -> Vec<String> {
    let key = format!("{}:{}", kind, id);
    match kind {
        "events" => vec![
            format!("raw({})", key),
            "xform(*)".into(),
            "gate(*)".into(),
            "population(*)".into(),
        ],
        "spillover" | "compensation" => vec![
            format!("comp({})", key),
            "xform(FLUO)".into(),
            "gate(FLUO)".into(),
            "population(*)".into(),
        ],
        "transform" => vec![format!("xform({})", key), "gate(transformed)".into(), "population(*)" .into()],
        "gate" => vec![format!("gate({})", key), "population(descendants)".into()],
        "lineage" => vec![format!("population({})", key)],
        _ => vec![],
    }
}

/// 人工裁定：选择补偿候选（或拒绝全部）。
pub fn decide_compensation(
    conn: &Connection,
    at: &Attempt,
    subject: &str,
    candidate: &str,
    note: Option<&str>,
) -> AppResult<Value> {
    tx(conn, |t| {
        let tip = check_rev(t, at)?;
        let st = State::replay(t, &at.branch, Some(tip))?;
        let ev = Eval::build(t, st).map_err(|e| AppError(e, 409))?;
        let comp = ev
            .nodes
            .values()
            .find(|n| n.node.kind == "compensation")
            .ok_or_else(|| AppError("无补偿节点".into(), 409))?;
        let legal: Vec<String> = comp.data["candidates"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter(|c| c["legal"].as_bool() == Some(true))
                    .map(|c| c["id"].as_str().unwrap_or("").to_string())
                    .collect()
            })
            .unwrap_or_default();
        if candidate != "reject-all" && !legal.iter().any(|x| x == candidate) {
            return Err(AppError(
                serde_json::to_string(&json!({
                    "error": "illegal-candidate",
                    "legal_candidates": legal,
                }))
                .unwrap(),
                409,
            ));
        }
        let seq = tip + 1;
        crate::store_ops::insert_event(
            t,
            seq,
            &at.branch,
            evkind::COMP_DECISION,
            subject,
            &json!({
                "candidate": candidate,
                "decided_revision": seq,
                "note": note,
                "legal_candidates": legal,
            }),
            &json!({
                "rule": "多候选显式保存；裁定钉住候选身份与规则版本",
                "classification": comp.data["classification"],
                "rcond_1": comp.data["rcond_1"],
                "rules_version": RULES_VERSION,
            }),
            RULES_VERSION,
            None,
            None,
        )?;
        t.execute(
            "INSERT INTO decisions(branch,subject,active,candidate,decided_seq)
             VALUES(?1,?2,1,?3,?4)
             ON CONFLICT(branch,subject) DO UPDATE SET active=1,candidate=excluded.candidate,decided_seq=excluded.decided_seq",
            params![at.branch, subject, candidate, seq],
        )?;
        Ok(json!({"branch": at.branch, "rev": seq}))
    })
}

/// 撤销最近一次裁定（产生新事件；旧分支结论继续可读）。
pub fn undo_decision(conn: &Connection, at: &Attempt, subject: &str) -> AppResult<Value> {
    tx(conn, |t| {
        let tip = check_rev(t, at)?;
        let last: Option<(String, i64)> = t
            .query_row(
                "SELECT candidate,decided_seq FROM decisions WHERE branch=?1 AND subject=?2 AND active=1",
                params![at.branch, subject],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let (old_cand, old_seq) = last.ok_or_else(|| AppError("没有可撤销的生效裁定".into(), 409))?;
        let seq = tip + 1;
        crate::store_ops::insert_event(
            t,
            seq,
            &at.branch,
            evkind::UNDO,
            subject,
            &json!({
                "undo_subject": subject,
                "removed_candidate": old_cand,
                "removed_event_seq": old_seq,
            }),
            &json!({"rule": "裁定可撤销并恢复前一分支；历史事件不被覆盖", "rules_version": RULES_VERSION}),
            RULES_VERSION,
            None,
            None,
        )?;
        t.execute("DELETE FROM decisions WHERE branch=?1 AND subject=?2", params![at.branch, subject])?;
        Ok(json!({"branch": at.branch, "rev": seq}))
    })
}

trait OptionalExt<T> {
    fn optional(self) -> Result<Option<T>, rusqlite::Error>;
}
impl<T> OptionalExt<T> for rusqlite::Result<T> {
    fn optional(self) -> Result<Option<T>, rusqlite::Error> {
        match self {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

/// 接受某节点的新投影（刷新）：只对明确选中的节点生效，其它过期节点保留旧结论。
pub fn refresh_node(
    conn: &Connection,
    at: &Attempt,
    node_id: &str,
    cfg_hash: &str,
) -> AppResult<Value> {
    tx(conn, |t| {
        let tip = check_rev(t, at)?;
        let st = State::replay(t, &at.branch, Some(tip))?;
        let ev = Eval::build(t, st).map_err(|e| AppError(e, 409))?;
        let nv = ev
            .nodes
            .get(node_id)
            .ok_or_else(|| AppError(format!("节点 {} 不存在", node_id), 404))?;
        if nv.node.cfg_hash != cfg_hash {
            return Err(AppError(
                serde_json::to_string(&json!({
                    "error": "cfg-mismatch",
                    "current_cfg": nv.node.cfg_hash,
                }))
                .unwrap(),
                409,
            ));
        }
        let seq = tip + 1;
        crate::store_ops::insert_event(
            t,
            seq,
            &at.branch,
            evkind::REFRESH,
            node_id,
            &json!({
                "node_id": node_id,
                "cfg_hash": cfg_hash,
                "label": nv.node.label,
                "accepted_at_revision": seq,
            }),
            &json!({
                "rule": "修订上游门后下游结果标记过期而不覆盖；显式接受才钉住新版本",
                "rules_version": RULES_VERSION,
            }),
            RULES_VERSION,
            None,
            None,
        )?;
        // 派生物快照（cfg_hash 为主键，历史投影永久保留）
        t.execute(
            "INSERT INTO derived_results(branch,node_id,cfg_hash,seq,status,data_json,basis_json)
             VALUES(?1,?2,?3,?4,'accepted',?5,?6)
             ON CONFLICT(branch,node_id,cfg_hash) DO UPDATE SET seq=excluded.seq,status='accepted'",
            params![
                at.branch,
                node_id,
                cfg_hash,
                seq,
                serde_json::to_string(&nv.data).unwrap(),
                serde_json::to_string(&nv.basis).unwrap(),
            ],
        )?;
        Ok(json!({"branch": at.branch, "rev": seq}))
    })
}

/// 从旧事件点或另一分支创建分支：复制截止 seq 的事件，旧分支、裁定与原始字节继续可读。
pub fn fork_branch(
    conn: &Connection,
    parent: &str,
    new_name: &str,
    title: &str,
    at_seq: Option<i64>,
) -> AppResult<Value> {
    tx(conn, |t| {
        if crate::store_ops::branch_exists(t, new_name) {
            return Err(AppError(format!("分支 {} 已存在", new_name), 409));
        }
        if !crate::store_ops::branch_exists(t, parent) {
            return Err(AppError(format!("父分支 {} 不存在", parent), 404));
        }
        let rows = crate::store_ops::list_events_until(t, parent, at_seq)?;
        let fork_seq = rows.last().map(|r| r.seq).unwrap_or(0);
        crate::store_ops::create_branch(t, new_name, title, Some(parent), Some(fork_seq), 0)?;
        for (i, r) in rows.iter().enumerate() {
            crate::store_ops::insert_event(
                t,
                (i + 1) as i64,
                new_name,
                &r.kind,
                &r.subject,
                &r.payload,
                &r.basis,
                &r.basis_rules,
                r.mat_kind.as_deref().zip(r.mat_id.as_deref()).and_then(|(k, id)| {
                    r.mat_version.map(|v| (k, id, v as u32))
                }).as_ref().map(|x| (x.0, x.1, x.2)),
                Some((r.seq, parent)),
            )?;
        }
        // 复制生效裁定
        let mut stmt = t.prepare(
            "SELECT subject,candidate,decided_seq FROM decisions WHERE branch=?1",
        )?;
        let decs: Vec<(String, String, i64)> = stmt
            .query_map([parent], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<_>>()?;
        drop(stmt);
        let next = fork_seq + 1;
        for (subj, cand, dseq) in decs {
            if dseq <= fork_seq {
                t.execute(
                    "INSERT INTO decisions(branch,subject,active,candidate,decided_seq)
                     VALUES(?1,?2,1,?3,?4)",
                    params![new_name, subj, cand, dseq],
                )?;
            }
        }
        crate::store_ops::insert_event(
            t,
            next,
            new_name,
            evkind::FORK,
            new_name,
            &json!({
                "parent": parent, "fork_seq": fork_seq,
                "preserved_raw_bytes": true,
            }),
            &json!({
                "rule": "对同一份材料重新解释时新建分支；旧分支、人工裁定与原始字节可继续读取",
                "rules_version": RULES_VERSION,
            }),
            RULES_VERSION,
            None,
            None,
        )?;
        Ok(json!({"branch": new_name, "parent": parent, "fork_seq": fork_seq, "rev": next}))
    })
}

/// 记录一次失败导入（诊断留痕，但材料/事件整体回滚）。
pub fn failed_import(conn: &Connection, branch: &str, reason: &str, fragment: &str) -> AppResult<()> {
    tx(conn, |t| {
        let tip = crate::store_ops::branch_tip(t, branch)?;
        crate::store_ops::insert_event(
            t,
            tip + 1,
            branch,
            evkind::ROLLBACK,
            "import",
            &json!({
                "reason": reason,
                "input_fragment": fragment.chars().take(400).collect::<String>(),
                "rolled_back": true,
            }),
            &json!({"rule": "导入失败只回滚本次变更，不留下半份记录", "rules_version": RULES_VERSION}),
            RULES_VERSION,
            None,
            None,
        )?;
        Ok(())
    })
}

/// 导出整库为 bundle（身份、顺序、来源关系保持原样）。
pub fn export_bundle(conn: &Connection) -> AppResult<Value> {
    let c = conn;
    let mut branches: Vec<Value> = vec![];
    {
        let mut stmt = c.prepare("SELECT name,title,parent,fork_seq FROM branches ORDER BY name")?;
        let rows = stmt.query_map([], |r| {
            Ok(json!({
                "name": r.get::<_, String>(0)?,
                "title": r.get::<_, String>(1)?,
                "parent": r.get::<_, Option<String>>(2)?,
                "fork_seq": r.get::<_, Option<i64>>(3)?,
            }))
        })?;
        for r in rows {
            branches.push(r?);
        }
    }
    let mut materials: Vec<Value> = vec![];
    {
        let mut stmt =
            c.prepare("SELECT kind,id,version,title,raw_json,sha256,source_file FROM materials ORDER BY kind,id,version")?;
        let rows = stmt.query_map([], |r| {
            let raw: String = r.get(4)?;
            Ok(json!({
                "kind": r.get::<_, String>(0)?,
                "id": r.get::<_, String>(1)?,
                "version": r.get::<_, i64>(2)?,
                "title": r.get::<_, String>(3)?,
                "raw": serde_json::from_str::<Value>(&raw).unwrap_or(Value::Null),
                "sha256": r.get::<_, String>(5)?,
                "source_file": r.get::<_, Option<String>>(6)?,
            }))
        })?;
        for r in rows {
            materials.push(r?);
        }
    }
    let mut events: Vec<Value> = vec![];
    {
        let mut stmt = c.prepare(
            "SELECT branch,seq,kind,subject,payload,basis,basis_rules,
                    mat_kind,mat_id,mat_version,origin_seq,origin_branch
             FROM events ORDER BY branch,seq",
        )?;
        let rows = stmt.query_map([], |r| {
            let payload: String = r.get(4)?;
            let basis: String = r.get(5)?;
            Ok(json!({
                "branch": r.get::<_, String>(0)?,
                "seq": r.get::<_, i64>(1)?,
                "kind": r.get::<_, String>(2)?,
                "subject": r.get::<_, String>(3)?,
                "payload": serde_json::from_str::<Value>(&payload).unwrap_or(Value::Null),
                "basis": serde_json::from_str::<Value>(&basis).unwrap_or(Value::Null),
                "basis_rules": r.get::<_, String>(6)?,
                "mat_kind": r.get::<_, Option<String>>(7)?,
                "mat_id": r.get::<_, Option<String>>(8)?,
                "mat_version": r.get::<_, Option<i64>>(9)?,
                "origin_seq": r.get::<_, Option<i64>>(10)?,
                "origin_branch": r.get::<_, Option<String>>(11)?,
            }))
        })?;
        for r in rows {
            events.push(r?);
        }
    }
    let bundle = json!({
        "bundle": "flowline-case",
        "rules_version": RULES_VERSION,
        "branches": branches,
        "materials": materials,
        "events": events,
    });
    Ok(bundle)
}

/// 导入 bundle：整体事务，任何不一致全部回滚。
pub fn import_bundle(conn: &Connection, bundle: &Value) -> AppResult<Value> {
    tx(conn, |t| {
        if bundle.get("bundle").and_then(|x| x.as_str()) != Some("flowline-case") {
            return Err(AppError("不是 flowline-case bundle".into(), 400));
        }
        let n_branches = t.query_row("SELECT COUNT(*) FROM branches", [], |r| r.get::<_, i64>(0))?;
        if n_branches > 0 {
            return Err(AppError("bundle 只能导入空数据库（删除本地数据库后再导入）".into(), 409));
        }
        for m in bundle["materials"].as_array().ok_or("bundle.materials 缺失")? {
            let raw = &m["raw"];
            let raw_s = serde_json::to_vec(raw).unwrap_or_default();
            let sha = crate::store::sha256_hex(&raw_s);
            if let Some(expected) = m.get("sha256").and_then(|x| x.as_str()) {
                if expected != sha {
                    return Err(AppError(
                        format!("材料 {}:{}@v{} 原始字节 sha256 不一致，拒绝导入",
                            m["kind"].as_str().unwrap_or("?"),
                            m["id"].as_str().unwrap_or("?"),
                            m["version"].as_i64().unwrap_or(-1)),
                        400,
                    ));
                }
            }
            t.execute(
                "INSERT INTO materials(kind,id,version,title,raw_json,sha256,source_file)
                 VALUES(?1,?2,?3,?4,?5,?6,?7)",
                params![
                    m["kind"].as_str().unwrap_or(""),
                    m["id"].as_str().unwrap_or(""),
                    m["version"].as_i64().unwrap_or(0),
                    m["title"].as_str().unwrap_or(""),
                    serde_json::to_string(raw).unwrap(),
                    sha,
                    m["source_file"].as_str(),
                ],
            )?;
        }
        for b in bundle["branches"].as_array().ok_or("bundle.branches 缺失")? {
            crate::store_ops::create_branch(
                t,
                b["name"].as_str().unwrap(),
                b["title"].as_str().unwrap(),
                b["parent"].as_str(),
                b["fork_seq"].as_i64(),
                0,
            )?;
        }
        let mut decisions: Vec<(String, String, String, i64)> = vec![];
        for e in bundle["events"].as_array().ok_or("bundle.events 缺失")? {
            let mat = e["mat_kind"].as_str().zip(e["mat_id"].as_str()).and_then(|(k, id)| {
                e["mat_version"].as_i64().map(|v| (k, id, v as u32))
            });
            let origin = e["origin_seq"].as_i64().zip(e["origin_branch"].as_str());
            crate::store_ops::insert_event(
                t,
                e["seq"].as_i64().unwrap(),
                e["branch"].as_str().unwrap(),
                e["kind"].as_str().unwrap(),
                e["subject"].as_str().unwrap(),
                &e["payload"],
                &e["basis"],
                e["basis_rules"].as_str().unwrap_or(RULES_VERSION),
                mat.as_ref().map(|x| (x.0, x.1, x.2)),
                origin,
            )?;
            if e["kind"].as_str() == Some(evkind::COMP_DECISION) {
                if let (Some(sub), Some(cand)) =
                    (e["subject"].as_str(), e["payload"]["candidate"].as_str())
                {
                    decisions.push((
                        e["branch"].as_str().unwrap().to_string(),
                        sub.to_string(),
                        cand.to_string(),
                        e["seq"].as_i64().unwrap(),
                    ));
                }
            }
        }
        for (br, sub, cand, seq) in decisions {
            t.execute(
                "INSERT INTO decisions(branch,subject,active,candidate,decided_seq)
                 VALUES(?1,?2,1,?3,?4)",
                params![br, sub, cand, seq],
            )?;
        }
        Ok(json!({"imported": true,
                  "materials": bundle["materials"].as_array().map(|a| a.len()).unwrap_or(0),
                  "events": bundle["events"].as_array().map(|a| a.len()).unwrap_or(0)}))
    })
}

#[allow(dead_code)]
fn keep(_: RowEvent) {
    let _ = stable_id;
}
