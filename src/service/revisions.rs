//! 边界情形触发：参数修订、近奇异/奇异矩阵、门几何/改名、局部源改动、
//! 上游过期传播、同版本并发（commit_version 已处理）。

use super::pipeline::{compute_in_tx, current_refs, CurrentRefs};
use crate::compute::stale_scope;
use crate::domain::{EntityKind, EntityVersion, EventPayload, PolygonGate};
use crate::service::orchestrate::read_case;
use crate::store::Repo;
use rusqlite::OptionalExtension;
use serde_json::json;

fn main_tube_id(repo: &Repo, branch_id: &str) -> crate::store::StoreResult<String> {
    repo.get_config(branch_id, "main_tube")?
        .ok_or_else(|| crate::store::err("NO_TUBE", "分支未设置主管材料"))
}

fn current_single(repo: &Repo, branch_id: &str, kind: EntityKind, entity_id: &str)
    -> crate::store::StoreResult<EntityVersion> {
    repo.current_version(branch_id, kind, entity_id)?
        .ok_or_else(|| crate::store::err("NO_VERSION", format!("{entity_id} 无版本")))
}

fn current_gate_map(repo: &Repo, branch_id: &str)
    -> crate::store::StoreResult<std::collections::HashMap<String, EntityVersion>> {
    Ok(repo
        .list_current_versions(branch_id, EntityKind::Gate)?
        .into_iter()
        .map(|v| {
            let g: PolygonGate = serde_json::from_value(v.data.clone()).unwrap();
            (g.gate_id, v)
        })
        .collect())
}

fn current_lineage_links(repo: &Repo, branch_id: &str)
    -> crate::store::StoreResult<Vec<crate::domain::GateLink>> {
    let v = current_single(repo, branch_id, EntityKind::Lineage, "tree_main")?;
    Ok(serde_json::from_value(v.data).unwrap())
}

/// 计算当前最新结果中，受影响门对应的“最新结果 id”。
fn latest_result_ids_for_gates(
    tx: &rusqlite::Transaction,
    branch_id: &str,
    gate_ids: &[String],
) -> crate::store::StoreResult<Vec<String>> {
    let mut out = Vec::new();
    for gid in gate_ids {
        let id: Option<String> = tx
            .query_row(
                "SELECT id FROM gate_results WHERE branch_id=?1 AND gate_entity_id=?2
                 ORDER BY created_at DESC,rowid DESC LIMIT 1",
                rusqlite::params![branch_id, gid],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(id) = id {
            out.push(id);
        }
    }
    Ok(out)
}

/// 把受影响门的最新结果标记 stale，并写 MarkedStale 事件。
fn mark_scope_stale(
    tx: &rusqlite::Transaction,
    branch_id: &str,
    gate_ids: &[String],
    changed_version_id: &str,
    reason: &str,
) -> crate::store::StoreResult<()> {
    let ids = latest_result_ids_for_gates(tx, branch_id, gate_ids)?;
    if !ids.is_empty() {
        Repo::mark_stale_tx(tx, &ids, reason)?;
        Repo::append_event_tx(
            tx,
            branch_id,
            "reviewer",
            &EventPayload::MarkedStale {
                stale_result_ids: ids,
                reason: reason.into(),
                changed_version_id: changed_version_id.into(),
            },
        )?;
    }
    Ok(())
}

fn import_case_material(
    tx: &rusqlite::Transaction,
    case_path: &str,
    filename: &str,
    kind: &str,
    label: &str,
) -> crate::store::StoreResult<String> {
    let bytes = read_case(case_path)
        .ok_or_else(|| crate::store::err("CASE_FILE_MISSING", case_path))?
        .to_vec();
    let (mat, _) = Repo::insert_material_tx(tx, filename, kind, &bytes, label)?;
    if kind == "fcs" {
        let tube = crate::service::parse_tube(&bytes);
        for (i, d) in tube.diagnostics.iter().enumerate() {
            Repo::insert_diagnostic_tx(tx, &mat.id, i as i64, &d.code, &d.detail)?;
        }
    }
    Ok(mat.id)
}

fn apply_new_spillover_file(
    repo: &mut Repo,
    branch_id: &str,
    case_path: &str,
    filename: &str,
    label: &str,
    note: &str,
) -> crate::store::StoreResult<EntityVersion> {
    let expected = repo.current_version_number(branch_id, EntityKind::Spillover, "spill_main")?;
    let tx = repo.conn.transaction()?;
    let mat_id = import_case_material(&tx, case_path, filename, "spillover", label)?;
    let doc: serde_json::Value =
        serde_json::from_slice(read_case(case_path).unwrap()).unwrap();
    let v = Repo::add_version_tx(
        &tx,
        branch_id,
        crate::store::versions::ProposedVersion {
            kind: EntityKind::Spillover,
            entity_id: "spill_main",
            data: &doc,
            source_material_id: Some(&mat_id),
            note,
            expected_version: expected,
            actor: "reviewer",
        },
    )?;
    // spillover 变化影响所有荧光门及门链上的所有结果（坐标/成员依赖补偿）。
    let gates = repo_versions_in_tx(&tx, branch_id, EntityKind::Gate);
    let links = links_in_tx(&tx, branch_id)?;
    let scope = stale_scope(EntityKind::Spillover, "", &gates, &links);
    mark_scope_stale(&tx, branch_id, &scope, &v.id, "spillover 修订：补偿/荧光成员依赖失效")?;
    tx.commit()?;
    Ok(v)
}

/// 第二分支：加载近奇异补偿矩阵（基线事件导入后调用）。
pub fn revise_near_singular(repo: &mut Repo, branch_id: &str) -> crate::store::StoreResult<EntityVersion> {
    apply_new_spillover_file(
        repo,
        branch_id,
        "revisions/spillover.v2_near_singular.json",
        "spillover.v2_near_singular.json",
        "补偿矩阵 v2（近奇异）",
        "单染重新估计：近奇异矩阵，保留全部求逆候选",
    )
}

/// 冲突的人工决定：提交精确奇异矩阵。
pub fn revise_singular(repo: &mut Repo, branch_id: &str) -> crate::store::StoreResult<EntityVersion> {
    apply_new_spillover_file(
        repo,
        branch_id,
        "revisions/spillover.v3_singular.json",
        "spillover.v3_singular.json",
        "补偿矩阵 v3（奇异）",
        "冲突人工决定：第3行改为第1行两倍，矩阵精确奇异",
    )
}

/// 一次参数修订（asinh cofactor 150 -> 250），所有门结果标记过期。
pub fn revise_transforms(repo: &mut Repo, branch_id: &str) -> crate::store::StoreResult<EntityVersion> {
    let expected = repo.current_version_number(branch_id, EntityKind::Transform, "xform_main")?;
    let tx = repo.conn.transaction()?;
    let mat_id = import_case_material(
        &tx,
        "revisions/transforms.v2.json",
        "transforms.v2.json",
        "transforms",
        "变换参数 v2 cofactor=250",
    )?;
    let doc: serde_json::Value = serde_json::from_slice(read_case("revisions/transforms.v2.json").unwrap()).unwrap();
    let params = doc.get("params").cloned().unwrap_or(serde_json::Value::Null);
    let v = Repo::add_version_tx(
        &tx,
        branch_id,
        crate::store::versions::ProposedVersion {
            kind: EntityKind::Transform,
            entity_id: "xform_main",
            data: &params,
            source_material_id: Some(&mat_id),
            note: "参数修订 cofactor 150->250",
            expected_version: expected,
            actor: "reviewer",
        },
    )?;
    let gates = repo_versions_in_tx(&tx, branch_id, EntityKind::Gate);
    let links = links_in_tx(&tx, branch_id)?;
    let scope = stale_scope(EntityKind::Transform, "", &gates, &links);
    mark_scope_stale(&tx, branch_id, &scope, &v.id, "变换参数修订：门坐标与成员失效")?;
    tx.commit()?;
    Ok(v)
}

fn repo_versions_in_tx(
    tx: &rusqlite::Transaction,
    branch_id: &str,
    kind: EntityKind,
) -> Vec<EntityVersion> {
    let mut s = tx
        .prepare(
            "SELECT id,kind,entity_id,version,data,source_material_id,branch_id,parent_version_id,created_at,note
             FROM entity_versions e
             WHERE branch_id=?1 AND kind=?2
               AND version=(SELECT MAX(version) FROM entity_versions
                            WHERE branch_id=e.branch_id AND kind=e.kind AND entity_id=e.entity_id)",
        )
        .unwrap();
    let rows = s
        .query_map(rusqlite::params![branch_id, kind.as_str()], |r| {
            let data_s: String = r.get(4)?;
            let kind_s: String = r.get(1)?;
            Ok(EntityVersion {
                id: r.get(0)?,
                kind: EntityKind::parse(&kind_s).unwrap(),
                entity_id: r.get(2)?,
                version: r.get(3)?,
                data: serde_json::from_str(&data_s).unwrap(),
                source_material_id: r.get(5)?,
                branch_id: r.get(6)?,
                parent_version_id: r.get(7)?,
                created_at: r.get(8)?,
                note: r.get(9)?,
            })
        })
        .unwrap();
    rows.collect::<rusqlite::Result<Vec<_>>>().unwrap()
}

fn links_in_tx(
    tx: &rusqlite::Transaction,
    branch_id: &str,
) -> crate::store::StoreResult<Vec<crate::domain::GateLink>> {
    let data: String = tx.query_row(
        "SELECT data FROM entity_versions WHERE branch_id=?1 AND kind='lineage'
         ORDER BY version DESC LIMIT 1",
        rusqlite::params![branch_id],
        |r| r.get(0),
    )?;
    Ok(serde_json::from_str(&data).unwrap())
}

/// 门嵌套改名：从检入的 gates.v2_rename.json 整体提交新版本。
/// gate_id 保持稳定；仅名称变化不产生数值失效（只更新显示版本）。
/// 返回（新版本列表, 改名的门 id, 几何变化的门 id）。
pub fn rename_gates(
    repo: &mut Repo,
    branch_id: &str,
) -> crate::store::StoreResult<(Vec<String>, Vec<String>, Vec<String>)> {
    let doc: serde_json::Value =
        serde_json::from_slice(read_case("revisions/gates.v2_rename.json").unwrap()).unwrap();
    let new_gates: Vec<PolygonGate> =
        serde_json::from_value(doc.get("gates").cloned().unwrap_or(json!([]))).unwrap();
    let old = current_gate_map(repo, branch_id)?;
    let links = current_lineage_links(repo, branch_id)?;

    let tx = repo.conn.transaction()?;
    let mut new_ids = Vec::new();
    let mut renamed = Vec::new();
    let mut geom_changed = Vec::new();
    for g in &new_gates {
        let old_v = old.get(&g.gate_id).expect("改名草案只引用既有 gate_id");
        let old_gate: PolygonGate = serde_json::from_value(old_v.data.clone()).unwrap();
        let expected = old_v.version;
        let mut data = serde_json::to_value(g).unwrap();
        if let Some(b) = data.pointer_mut("/basis") {
            if b.get("transform_version_id").and_then(|v| v.as_str()) == Some("RESOLVED_AT_IMPORT") {
                let xform = repo_versions_in_tx(&tx, branch_id, EntityKind::Transform)
                    .into_iter().next().unwrap();
                b["transform_version_id"] = json!(xform.id);
            }
        }
        let new_gate: PolygonGate = serde_json::from_value(data.clone()).unwrap();
        let rename_only = old_gate.points == new_gate.points
            && old_gate.basis == new_gate.basis
            && old_gate.name != new_gate.name;
        let note = if rename_only {
            renamed.push(g.gate_id.clone());
            format!("门嵌套改名：{} -> {}（gate_id 稳定，几何不变）", old_gate.name, new_gate.name)
        } else {
            geom_changed.push(g.gate_id.clone());
            "门几何/坐标基准变更".to_string()
        };
        let v = Repo::add_version_tx(
            &tx,
            branch_id,
            crate::store::versions::ProposedVersion {
                kind: EntityKind::Gate,
                entity_id: &g.gate_id,
                data: &data,
                source_material_id: old_v.source_material_id.as_deref(),
                note: &note,
                expected_version: expected,
                actor: "reviewer",
            },
        )?;
        new_ids.push(v.id.clone());
        if !rename_only {
            let scope = stale_scope(EntityKind::Gate, &g.gate_id, &repo_versions_in_tx(&tx, branch_id, EntityKind::Gate), &links);
            mark_scope_stale(&tx, branch_id, &scope, &v.id, "门几何修订：本门及下游失效")?;
        }
    }
    tx.commit()?;
    Ok((new_ids, renamed, geom_changed))
}

/// 只影响局部依赖的源数据改动：导入修订试管，精确计算哪些门成员实际变化，
/// 仅这些门（及子孙）标记过期。
pub fn change_source_local(
    repo: &mut Repo,
    branch_id: &str,
) -> crate::store::StoreResult<(String, Vec<String>)> {
    let old_tube = main_tube_id(repo, branch_id)?;
    let refs = current_refs(repo, branch_id)?;
    // 导入新材料（先在事务里）。
    let tx = repo.conn.transaction()?;
    let new_mat = import_case_material(
        &tx,
        "revisions/main_tube_revised.fcs",
        "main_tube_revised.fcs",
        "fcs",
        "主管（局部源数据修订）",
    )?;
    // 用新旧材料分别计算成员集合，做精确差集（当前已采纳补偿/变换）。
    let affected = diff_affected_gates(repo, &tx, branch_id, &refs, &old_tube, &new_mat)?;
    // 标记失效范围。
    let ids = {
        let mut s = Vec::new();
        for gid in &affected {
            if let Some(id) = tx
                .query_row(
                    "SELECT id FROM gate_results WHERE branch_id=?1 AND gate_entity_id=?2
                     ORDER BY created_at DESC,rowid DESC LIMIT 1",
                    rusqlite::params![branch_id, gid],
                    |r| r.get::<_, String>(0),
                )
                .optional()?
            {
                s.push(id);
            }
        }
        s
    };
    Repo::mark_stale_tx(&tx, &ids, "局部源数据改动：仅成员实际变化的门及下游过期")?;
    Repo::append_event_tx(
        &tx,
        branch_id,
        "reviewer",
        &EventPayload::SourceMaterialChanged {
            old_material_id: Some(old_tube.clone()),
            new_material_id: new_mat.clone(),
            affected_gate_ids: affected.clone(),
        },
    )?;
    tx.execute(
        "UPDATE branch_config SET value=?1 WHERE branch_id=?2 AND key='main_tube'",
        rusqlite::params![new_mat, branch_id],
    )?;
    tx.commit()?;
    Ok((new_mat, affected))
}

use super::pipeline::{derived_in_tx, topo_order};
use crate::domain::{LineageSet};

fn memberships_with_tube(
    tx: &rusqlite::Transaction,
    branch_id: &str,
    refs: &CurrentRefs,
    tube_id: &str,
) -> crate::store::StoreResult<std::collections::HashMap<String, Vec<i64>>> {
    use crate::service::derive::{spill_from_version, transform_from_version};
    let spill = spill_from_version(&refs.spill)?;
    let xform = transform_from_version(&refs.transform)?;
    // 事务内读取已接受候选（基线为 None，良态自动选直接逆）。
    let chosen = tx
        .query_row(
            "SELECT chosen_index FROM decisions
             WHERE branch_id=?1 AND target_version_id=?2 AND active=1 AND decision='accepted'
             ORDER BY created_at DESC,rowid DESC LIMIT 1",
            rusqlite::params![branch_id, refs.spill.id],
            |r| r.get::<_, Option<i64>>(0),
        )
        .optional()
        .ok()
        .flatten()
        .flatten()
        .map(|v| v as usize);
    let d = derived_in_tx(tx, tube_id, &spill, &refs.spill.id, chosen, &xform)?;
    let tube = super::pipeline::tube_from_conn(tx, tube_id)?;
    let lineage: LineageSet = serde_json::from_value(refs.lineage.data.clone()).unwrap();
    let gates: Vec<PolygonGate> = refs
        .gates
        .iter()
        .map(|v| serde_json::from_value(v.data.clone()).unwrap())
        .collect();
    let order = topo_order(&gates, &lineage.links);
    let universe: Vec<i64> = (0..d.derived.len() as i64).collect();
    let mut members: std::collections::HashMap<String, Vec<i64>> = std::collections::HashMap::new();
    let mut out = std::collections::HashMap::new();
    for gid in order {
        let gv = refs
            .gates
            .iter()
            .find(|v| {
                serde_json::from_value::<PolygonGate>(v.data.clone())
                    .map(|g| g.gate_id == gid)
                    .unwrap_or(false)
            })
            .unwrap();
        let gate: PolygonGate = serde_json::from_value(gv.data.clone()).unwrap();
        let parent = lineage.links.iter().find(|l| l.gate_id == gid).and_then(|l| l.parent_gate_id.clone());
        let parent_members = parent.as_ref().map(|p| members.get(p).cloned().unwrap_or_default()).unwrap_or_else(|| universe.clone());
        let uses_fluor = super::pipeline::FLUOR_CHANNELS
            .iter()
            .any(|f| gate.basis.x_channel == *f || gate.basis.y_channel == *f);
        let inc = if uses_fluor && d.outcome.chosen_inverse.is_none() {
            // 阻断：无法决定成员，视为变化以保守标记。
            None
        } else {
            let transformed: Vec<Vec<Option<f64>>> = d.derived.iter().map(|x| x.transformed.clone()).collect();
            let m = crate::compute::evaluate_gate(&gate, &gv.id, &transformed, &tube.channel_names, &parent_members)
                .map_err(|e| crate::store::err("GATE_EVAL", e))?;
            Some(m.included_event_indices)
        };
        match inc {
            Some(v) => {
                members.insert(gid.clone(), v.clone());
                out.insert(gid, Some(v));
            }
            None => {
                members.insert(gid.clone(), vec![]);
                out.insert(gid, None::<Vec<i64>>);
            }
        }
    }
    // 该函数返回“是否变化”由调用方比较；None 表示阻断。
    Ok(out.into_iter().filter_map(|(k, v)| v.map(|v| (k, v))).collect())
}

fn diff_affected_gates(
    _repo: &Repo,
    tx: &rusqlite::Transaction,
    branch_id: &str,
    refs: &CurrentRefs,
    old_tube: &str,
    new_tube: &str,
) -> crate::store::StoreResult<Vec<String>> {
    let oldm = memberships_with_tube(tx, branch_id, refs, old_tube)?;
    let newm = memberships_with_tube(tx, branch_id, refs, new_tube)?;
    // 父子传播：任一父门变化，子孙也失效。
    let lineage: LineageSet = serde_json::from_value(refs.lineage.data.clone()).unwrap();
    let mut changed = std::collections::HashSet::new();
    for (gid, nv) in &newm {
        if oldm.get(gid) != Some(nv) {
            changed.insert(gid.clone());
        }
    }
    // 传递闭包。
    let mut frontier: Vec<String> = changed.iter().cloned().collect();
    while let Some(g) = frontier.pop() {
        for l in &lineage.links {
            if l.parent_gate_id.as_deref() == Some(g.as_str()) && changed.insert(l.gate_id.clone()) {
                frontier.push(l.gate_id.clone());
            }
        }
    }
    let mut out: Vec<String> = changed.into_iter().collect();
    out.sort();
    Ok(out)
}

/// 依据当前版本重算所有过期结果（生成新结果，不覆盖旧结果）。
pub fn recompute(repo: &mut Repo, branch_id: &str) -> crate::store::StoreResult<usize> {
    let refs = current_refs(repo, branch_id)?;
    let tube = main_tube_id(repo, branch_id)?;
    let tx = repo.conn.transaction()?;
    let rows = compute_in_tx(&tx, branch_id, &refs, &tube, "reviewer")?;
    tx.commit()?;
    Ok(rows.len())
}

/// 人工裁定：接受某个求逆候选（near_singular）或拒绝（保持阻断/保留候选）。
pub fn adjudicate(
    repo: &mut Repo,
    branch_id: &str,
    spill_version_id: &str,
    decision: &str,
    chosen_index: Option<i64>,
) -> crate::store::StoreResult<String> {
    // 校验目标存在。
    let v = repo.get_version(spill_version_id)?;
    if v.branch_id != branch_id || v.kind != EntityKind::Spillover {
        return Err(crate::store::err("BAD_TARGET", "裁定目标必须是本分支的 spillover 版本"));
    }
    if decision == "accepted" {
        // 候选索引合法性由 verdict 内容校验。
        let verdicts = repo.list_verdicts(branch_id)?;
        let vd = verdicts
            .iter()
            .find(|j| j["spill_version_id"] == json!(spill_version_id))
            .ok_or_else(|| crate::store::err("NO_VERDICT", "尚无补偿判定"))?;
        let count = vd["candidates"].as_array().map(|a| a.len()).unwrap_or(0) as i64;
        let idx = chosen_index.ok_or_else(|| crate::store::err("NO_CANDIDATE", "接受时必须指定候选"))?;
        if idx < 0 || idx >= count {
            return Err(crate::store::err("BAD_CANDIDATE", format!("候选索引 {idx} 越界（共 {count} 个）")));
        }
    }
    let tx = repo.conn.transaction()?;
    let ev = Repo::append_event_tx(
        &tx,
        branch_id,
        "reviewer",
        &EventPayload::Adjudicated {
            target_version_id: spill_version_id.into(),
            decision: decision.into(),
            chosen_index,
        },
    )?;
    Repo::add_decision_tx(&tx, branch_id, spill_version_id, decision, chosen_index, &ev.id)?;
    // 接受后：把使用该 spill 的现有结果标过期，等待重算（不覆盖）。
    if decision == "accepted" {
        let ids = latest_result_ids_all(&tx, branch_id)?;
        Repo::mark_stale_tx(&tx, &ids, "人工采纳求逆候选：补偿下游结果失效，等待重算")?;
        Repo::append_event_tx(
            &tx,
            branch_id,
            "reviewer",
            &EventPayload::MarkedStale {
                stale_result_ids: ids,
                reason: "采纳补偿候选".into(),
                changed_version_id: spill_version_id.into(),
            },
        )?;
    }
    tx.commit()?;
    Ok(ev.id)
}

fn latest_result_ids_all(tx: &rusqlite::Transaction, branch_id: &str) -> crate::store::StoreResult<Vec<String>> {
    let mut s = tx.prepare(
        "SELECT id FROM gate_results r WHERE branch_id=?1
         AND id=(SELECT id FROM gate_results WHERE branch_id=r.branch_id AND gate_entity_id=r.gate_entity_id
                ORDER BY created_at DESC,rowid DESC LIMIT 1)",
    )?;
    let rows = s.query_map(rusqlite::params![branch_id], |r| r.get::<_, String>(0))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// 撤销最近一次对该 spill 版本的裁定；恢复到“未裁定”，当前结果标记过期。
pub fn undo_adjudication(
    repo: &mut Repo,
    branch_id: &str,
    spill_version_id: &str,
) -> crate::store::StoreResult<String> {
    let tx = repo.conn.transaction()?;
    let undone = Repo::deactivate_latest_decision_tx(&tx, branch_id, spill_version_id)?
        .ok_or_else(|| crate::store::err("NOTHING_TO_UNDO", "没有活动裁定可撤销"))?;
    // 找到被撤销决定对应的事件 id。
    let undone_event_id: String = tx.query_row(
        "SELECT event_id FROM decisions WHERE id=?1",
        rusqlite::params![undone],
        |r| r.get(0),
    )?;
    let ev = Repo::append_event_tx(
        &tx,
        branch_id,
        "reviewer",
        &EventPayload::AdjudicationUndone {
            undone_event_id: undone_event_id.clone(),
        },
    )?;
    let ids = latest_result_ids_all(&tx, branch_id)?;
    Repo::mark_stale_tx(&tx, &ids, "撤销裁定：恢复到未裁定分支，结果过期")?;
    tx.commit()?;
    Ok(ev.id)
}
