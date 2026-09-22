//! 复核视图：分支状态、散点、门树、补偿前后差异、分母来源；支持任意事件 as-of。

use super::pipeline::{current_refs, topo_order};
use crate::domain::{EntityKind, EntityVersion, LineageSet, PolygonGate};
use crate::service::derive::{spill_from_version, transform_from_version, ParsedTube};
use crate::store::Repo;
use rusqlite::OptionalExtension;
use serde_json::json;

fn main_tube_id(repo: &Repo, branch_id: &str) -> crate::store::StoreResult<String> {
    repo.get_config(branch_id, "main_tube")?
        .ok_or_else(|| crate::store::err("NO_TUBE", "分支未设置主管材料"))
}

fn version_row(r: &rusqlite::Row) -> rusqlite::Result<EntityVersion> {
    let ds: String = r.get(4)?;
    Ok(EntityVersion {
        id: r.get(0)?,
        kind: EntityKind::parse(&r.get::<_, String>(1)?).unwrap(),
        entity_id: r.get(2)?,
        version: r.get(3)?,
        data: serde_json::from_str(&ds).unwrap(),
        source_material_id: r.get(5)?,
        branch_id: r.get(6)?,
        parent_version_id: r.get(7)?,
        created_at: r.get(8)?,
        note: r.get(9)?,
    })
}

/// 上面 LIKE 占位需要把具体版本 id 放入；改为逐版本检查。
fn version_as_of_exact(
    conn: &rusqlite::Connection,
    branch_id: &str,
    kind: EntityKind,
    entity_id: &str,
    max_seq: Option<i64>)
-> crate::store::StoreResult<Option<EntityVersion>> {
    let all: Vec<EntityVersion> = {
        let mut s = conn.prepare(
            "SELECT id,kind,entity_id,version,data,source_material_id,branch_id,parent_version_id,created_at,note
             FROM entity_versions WHERE branch_id=?1 AND kind=?2 AND entity_id=?3 ORDER BY version DESC",
        )?;
        let rows = s.query_map(rusqlite::params![branch_id, kind.as_str(), entity_id], version_row)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    for v in all {
        let exists: i64 = conn.query_row(
            "SELECT COUNT(*) FROM events
             WHERE branch_id=?1 AND seq<=COALESCE(?2, 9223372036854775807)
               AND payload LIKE ?3",
            rusqlite::params![
                branch_id,
                max_seq,
                format!("%\"version_id\":\"{}%", v.id)
            ],
            |r| r.get(0),
        )?;
        if exists > 0 {
            return Ok(Some(v));
        }
    }
    Ok(None)
}

/// 当前（或历史 as-of）复核视图。
pub fn review_state(
    repo: &Repo,
    branch_id: &str,
    as_of_seq: Option<i64>,
) -> crate::store::StoreResult<serde_json::Value> {
    let conn = &repo.conn;
    let tube_mat = main_tube_id(repo, branch_id)?;
    let spill_v = version_as_of_exact(conn, branch_id, EntityKind::Spillover, "spill_main", as_of_seq)?
        .ok_or_else(|| crate::store::err("NO_SPILL", "历史点尚无 spillover"))?;
    let xform_v = version_as_of_exact(conn, branch_id, EntityKind::Transform, "xform_main", as_of_seq)?
        .ok_or_else(|| crate::store::err("NO_TRANSFORM", "历史点尚无 transform"))?;
    let lineage_v = version_as_of_exact(conn, branch_id, EntityKind::Lineage, "tree_main", as_of_seq)?
        .ok_or_else(|| crate::store::err("NO_LINEAGE", "历史点尚无 lineage"))?;
    let lineage: LineageSet = serde_json::from_value(lineage_v.data.clone()).unwrap();
    // 历史点存在的门。
    let mut gates_v = Vec::new();
    for link in &lineage.links {
        if let Some(v) = version_as_of_exact(conn, branch_id, EntityKind::Gate, &link.gate_id, as_of_seq)? {
            gates_v.push(v);
        }
    }
    let refs = super::pipeline::CurrentRefs {
        spill: spill_v.clone(),
        transform: xform_v.clone(),
        gates: gates_v.clone(),
        lineage: lineage_v.clone(),
    };

    // as-of 时的主管材料可能是旧的：SourceMaterialChanged 事件决定。
    let tube_material_id = effective_tube_at(conn, branch_id, &tube_mat, as_of_seq)?;

    let spill = spill_from_version(&spill_v)?;
    let xform = transform_from_version(&xform_v)?;
    let chosen = accepted_choice_at(conn, branch_id, &spill_v.id, as_of_seq)?;
    let tube = super::pipeline::tube_from_conn(conn, &tube_material_id)?;
    let derived = {
        let fluor = super::pipeline::FLUOR_CHANNELS.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        crate::service::derive::derive(crate::service::derive::DerivedInputs {
            tube: &tube,
            spill: &spill,
            spill_version_id: &spill_v.id,
            chosen_index: chosen,
            transform: &xform,
            fluor_channels: &fluor,
        })
    };

    let gates: Vec<PolygonGate> = gates_v
        .iter()
        .map(|v| serde_json::from_value(v.data.clone()).unwrap())
        .collect();
    let order = topo_order(&gates, &lineage.links);

    // 每个门：成员（实时派生）+ 最近 <=seq 的记录结果（带分母来源）。
    let mut populations = Vec::new();
    let n_total = derived.derived.len() as i64;
    let universe: Vec<i64> = (0..n_total).collect();
    let mut members_map: std::collections::HashMap<String, Vec<i64>> = std::collections::HashMap::new();

    for gid in order {
        let gv = gates_v.iter().find(|v| {
            serde_json::from_value::<PolygonGate>(v.data.clone()).map(|g| g.gate_id==gid).unwrap_or(false)
        }).unwrap();
        let gate: PolygonGate = serde_json::from_value(gv.data.clone()).unwrap();
        let parent = lineage.links.iter().find(|l| l.gate_id==gid).and_then(|l| l.parent_gate_id.clone());
        let (parent_members, denominator_live, parent_result_id) = match &parent {
            None => (universe.clone(), n_total, None),
            Some(p) => (
                members_map.get(p).cloned().unwrap_or_default(),
                members_map.get(p).map(|m| m.len() as i64).unwrap_or(0),
                latest_result_id_at(conn, branch_id, p, as_of_seq)?,
            ),
        };
        let uses_fluor = super::pipeline::FLUOR_CHANNELS.iter().any(|f| gate.basis.x_channel==*f || gate.basis.y_channel==*f);
        let blocked = uses_fluor && derived.outcome.chosen_inverse.is_none();
        let transformed: Vec<Vec<Option<f64>>> = derived.derived.iter().map(|d| d.transformed.clone()).collect();
        let (inc, bi, be, live_count, live_percent) = if blocked {
            (vec![], vec![], vec![], -1, -1.0)
        } else {
            let m = crate::compute::evaluate_gate(&gate, &gv.id, &transformed, &tube.channel_names, &parent_members)
                .map_err(|e| crate::store::err("GATE_EVAL", e))?;
            let cnt = m.included_event_indices.len() as i64;
            let pct = if denominator_live==0 {0.0} else {100.0*cnt as f64/denominator_live as f64};
            let inc=m.included_event_indices.clone();
            (inc, m.boundary_included, m.boundary_excluded, cnt, pct)
        };
        members_map.insert(gid.clone(), inc.clone());

        // 历史记录结果（可能 stale/blocked），其分母必须钉住当时父群版本。
        let recorded = latest_result_at(conn, branch_id, &gid, as_of_seq)?;
        populations.push(json!({
            "gate_id": gid,
            "gate_version_id": gv.id,
            "name": gate.name,
            "parent_gate_id": parent,
            "parent_gate_version_id": recorded.as_ref().and_then(|r| r["parent_gate_version_id"].as_str()),
            "basis": gate.basis,
            "points": gate.points,
            "boundary_rule": gate.boundary_rule,
            "live": {
                "blocked": blocked,
                "count": live_count,
                "denominator": denominator_live,
                "percent": live_percent,
                "included": inc,
                "boundary_included": bi,
                "boundary_excluded": be,
            },
            "denominator_source": match &parent_result_id {
                None => json!({"kind":"root","event_count": n_total}),
                Some(rid) => json!({"kind":"parent_result","result_id": rid, "pinned": true}),
            },
            "recorded": recorded,
        }));
    }

    Ok(json!({
        "branch_id": branch_id,
        "as_of_seq": as_of_seq,
        "tube_material_id": tube_material_id,
        "raw_channels": tube.channel_names,
        "raw_units": tube.channel_units,
        "raw_ranges": tube.channel_ranges,
        "event_count": tube.events.len(),
        "diagnostics": tube.diagnostics,
        "spillover": {
            "version_id": spill_v.id,
            "version": spill_v.version,
            "matrix": spill,
            "verdict": derived.outcome.verdict,
            "active_choice": chosen,
        },
        "transform": {"version_id": xform_v.id, "version": xform_v.version, "params": xform.data},
        "lineage": {"version_id": lineage_v.id, "links": lineage.links},
        "events_for_scatter": derived.derived,
        "populations": populations,
    }))
}

fn events_until(conn: &rusqlite::Connection, branch_id: &str, max_seq: Option<i64>)
-> crate::store::StoreResult<Vec<crate::domain::EventRecord>> {
    let mut s = conn.prepare(
        "SELECT id,seq,branch_id,timestamp,actor,payload,prev_event_id FROM events
         WHERE branch_id=?1 AND seq<=COALESCE(?2, 9223372036854775807) ORDER BY seq")?;
    let rows = s.query_map(rusqlite::params![branch_id, max_seq], |r| {
        let ps: String = r.get(5)?;
        Ok(crate::domain::EventRecord {
            id: r.get(0)?, seq: r.get(1)?, branch_id: r.get(2)?, timestamp: r.get(3)?,
            actor: r.get(4)?,
            payload: serde_json::from_str(&ps).unwrap(),
            prev_event_id: r.get(6)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// 按 SourceMaterialChanged 事件回放到 max_seq，得到当时主管材料。
fn effective_tube_at(
    conn: &rusqlite::Connection,
    branch_id: &str,
    current_tube: &str,
    max_seq: Option<i64>,
) -> crate::store::StoreResult<String> {
    let mut tube = current_tube.to_string();
    for e in events_until(conn, branch_id, max_seq)? {
        if let crate::domain::EventPayload::SourceMaterialChanged { new_material_id, .. } = &e.payload {
            tube = new_material_id.clone();
        }
    }
    Ok(tube)
}

/// 回放 Adjudicated / AdjudicationUndone，决定 as-of 时是否有活动接受。
fn accepted_choice_at(
    conn: &rusqlite::Connection,
    branch_id: &str,
    spill_version_id: &str,
    max_seq: Option<i64>,
) -> crate::store::StoreResult<Option<usize>> {
    // 良态自动选择不需要决定。这里仅反映人工接受的候选。
    let mut active: Option<Option<i64>> = None;
    for e in events_until(conn, branch_id, max_seq)? {
        match &e.payload {
            crate::domain::EventPayload::Adjudicated { target_version_id, decision, chosen_index }
                if target_version_id == spill_version_id =>
            {
                active = Some(if decision == "accepted" { *chosen_index } else { None });
            }
            crate::domain::EventPayload::AdjudicationUndone { .. } => {
                active = None;
            }
            _ => {}
        }
    }
    Ok(active.flatten().map(|v| v as usize))
}

fn result_json(r: &crate::store::results::GateResultRow) -> serde_json::Value {
    json!({
        "result_id": r.id,
        "gate_version_id": r.gate_version_id,
        "gate_entity_id": r.gate_entity_id,
        "lineage_version_id": r.lineage_version_id,
        "transform_version_id": r.transform_version_id,
        "spill_version_id": r.spill_version_id,
        "parent_gate_version_id": r.parent_gate_version_id,
        "denominator_result_id": r.denominator_result_id,
        "count": r.count,
        "denominator": r.denominator,
        "percent": r.percent,
        "stale": r.stale,
        "stale_reason": r.stale_reason,
        "blocked": r.blocked,
        "blocked_reason": r.blocked_reason,
        "boundary_included": r.boundary_included,
        "boundary_excluded": r.boundary_excluded,
        "created_at": r.created_at,
    })
}

fn latest_result_at(
    conn: &rusqlite::Connection,
    branch_id: &str,
    gate_entity_id: &str,
    max_seq: Option<i64>,
) -> crate::store::StoreResult<Option<serde_json::Value>> {
    // 结果由 GateResultRecorded 事件产生：找 <=seq 的最新 result_id。
    let mut found: Option<(i64, String)> = None;
    for e in events_until(conn, branch_id, max_seq)? {
        if let crate::domain::EventPayload::GateResultRecorded { result_id, gate_version_id, .. } = &e.payload {
            // gate_version_id -> gate_entity_id 需要版本表。
            let gid: Option<String> = conn.query_row(
                "SELECT entity_id FROM entity_versions WHERE id=?1",
                rusqlite::params![gate_version_id], |r| r.get(0)).optional()?;
            if gid.as_deref() == Some(gate_entity_id) {
                found = Some((e.seq, result_id.clone()));
            }
        }
    }
    let Some((_, rid)) = found else { return Ok(None) };
    let row = conn.query_row(
        "SELECT id,gate_version_id,gate_entity_id,branch_id,lineage_version_id,transform_version_id,
                spill_version_id,parent_gate_version_id,denominator_result_id,count,denominator,
                percent,included_indices,boundary_included,boundary_excluded,stale,stale_reason,
                blocked,blocked_reason,created_at
         FROM gate_results WHERE id=?1",
        rusqlite::params![rid],
        |r| {
            let parse = |idx: usize| -> rusqlite::Result<Vec<i64>> {
                let s: String = r.get(idx)?;
                Ok(if s.is_empty() { vec![] } else { s.split(',').filter_map(|x| x.parse().ok()).collect() })
            };
            Ok(crate::store::results::GateResultRow {
                id: r.get(0)?, gate_version_id: r.get(1)?, gate_entity_id: r.get(2)?,
                branch_id: r.get(3)?, lineage_version_id: r.get(4)?, transform_version_id: r.get(5)?,
                spill_version_id: r.get(6)?, parent_gate_version_id: r.get(7)?,
                denominator_result_id: r.get(8)?, count: r.get(9)?, denominator: r.get(10)?,
                percent: r.get(11)?,
                included_indices: parse(12)?, boundary_included: parse(13)?, boundary_excluded: parse(14)?,
                stale: r.get::<_,i64>(15)?!=0, stale_reason: r.get(16)?,
                blocked: r.get::<_,i64>(17)?!=0, blocked_reason: r.get(18)?, created_at: r.get(19)?,
            })
        },
    )?;
    Ok(Some(result_json(&row)))
}

fn latest_result_id_at(
    conn: &rusqlite::Connection,
    branch_id: &str,
    gate_entity_id: &str,
    max_seq: Option<i64>,
) -> crate::store::StoreResult<Option<String>> {
    Ok(latest_result_at(conn, branch_id, gate_entity_id, max_seq)?
        .and_then(|v| v["result_id"].as_str().map(|s| s.to_string())))
}

/// 事件时间线（机器可读判定）。
pub fn timeline(repo: &Repo, branch_id: &str) -> crate::store::StoreResult<Vec<crate::domain::EventRecord>> {
    repo.list_events(branch_id)
}

/// 两个草案并排：返回两个分支的完整视图。
pub fn side_by_side(
    repo: &Repo,
    left_branch: &str,
    right_branch: &str,
) -> crate::store::StoreResult<serde_json::Value> {
    Ok(json!({
        "left": review_state(repo, left_branch, None)?,
        "right": review_state(repo, right_branch, None)?,
    }))
}

#[allow(dead_code)]
fn _refs(_r: CurrentRefs, _t: ParsedTube) { let _ = (current_refs, topo_order); }
