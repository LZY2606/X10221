//! 从“当前解释版本集合”构造派生数据与群体结果（写库，事务内）。

use crate::compute::evaluate_gate;
use crate::domain::{
    EntityKind, EntityVersion, EventPayload, GateLink, LineageSet, PolygonGate, TransformSet,
};
use crate::fcs::spill::SpillMatrix;
use crate::service::derive::{
    derive, parse_tube, spill_from_version, transform_from_version, DerivedInputs, DerivedTube,
    ParsedTube,
};
use crate::store::results::{GateResultRow, NewGateResult};
use crate::store::Repo;

pub const FLUOR_CHANNELS: [&str; 3] = ["FITC-A", "PE-A", "PerCP-A"];

pub fn load_derived(
    repo: &Repo,
    tube_material_id: &str,
    spill: &SpillMatrix,
    spill_version_id: &str,
    chosen_index: Option<usize>,
    transform: &TransformSet,
) -> crate::store::StoreResult<DerivedTube> {
    let bytes = repo.material_content(tube_material_id)?;
    let tube = parse_tube(&bytes);
    let fluor = FLUOR_CHANNELS.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    Ok(derive(DerivedInputs {
        tube: &tube,
        spill,
        spill_version_id,
        chosen_index,
        transform,
        fluor_channels: &fluor,
    }))
}

/// 从任意 rusqlite 连接/事务读取材料并解析。
pub fn tube_from_conn(
    conn: &impl std::ops::Deref<Target = rusqlite::Connection>,
    tube_material_id: &str,
) -> crate::store::StoreResult<ParsedTube> {
    let bytes: Vec<u8> = conn.query_row(
        "SELECT content FROM materials WHERE id=?1",
        rusqlite::params![tube_material_id],
        |r| r.get(0),
    )?;
    Ok(parse_tube(&bytes))
}

/// 事务内读取材料字节。
pub fn material_bytes_tx(
    tx: &rusqlite::Transaction,
    id: &str,
) -> crate::store::StoreResult<Vec<u8>> {
    Ok(tx.query_row(
        "SELECT content FROM materials WHERE id=?1",
        rusqlite::params![id],
        |r| r.get::<_, Vec<u8>>(0),
    )?)
}

/// 事务内派生（材料尚未提交也能读到）。
pub fn derived_in_tx(
    tx: &rusqlite::Transaction,
    tube_material_id: &str,
    spill: &SpillMatrix,
    spill_version_id: &str,
    chosen_index: Option<usize>,
    transform: &TransformSet,
) -> crate::store::StoreResult<DerivedTube> {
    let tube = tube_from_conn(tx, tube_material_id)?;
    let fluor = FLUOR_CHANNELS.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    Ok(derive(DerivedInputs {
        tube: &tube,
        spill,
        spill_version_id,
        chosen_index,
        transform,
        fluor_channels: &fluor,
    }))
}

fn chosen_in_tx(
    tx: &rusqlite::Transaction,
    branch_id: &str,
    spill_version_id: &str,
) -> Option<usize> {
    tx.query_row(
        "SELECT chosen_index FROM decisions
         WHERE branch_id=?1 AND target_version_id=?2 AND active=1 AND decision='accepted'
         ORDER BY created_at DESC,rowid DESC LIMIT 1",
        rusqlite::params![branch_id, spill_version_id],
        |r| r.get::<_, Option<i64>>(0),
    )
    .optional()
    .ok()
    .flatten()
    .flatten()
    .map(|v| v as usize)
}

/// 在事务内计算并记录全部群体结果（基线导入/重算共用）。
pub fn compute_in_tx(
    tx: &rusqlite::Transaction,
    branch_id: &str,
    refs: &CurrentRefs,
    tube_material_id: &str,
    actor: &str,
) -> crate::store::StoreResult<Vec<GateResultRow>> {
    let spill = spill_from_version(&refs.spill)?;
    let transform = transform_from_version(&refs.transform)?;
    let chosen = chosen_in_tx(tx, branch_id, &refs.spill.id);
    let derived = derived_in_tx(tx, tube_material_id, &spill, &refs.spill.id, chosen, &transform)?;

    let verdict = &derived.outcome.verdict;
    let rule = match verdict.status.as_str() {
        "singular" => "matrix_singular:no_direct_inverse:candidates_retained",
        "near_singular" => "matrix_near_singular:must_not_silently_invert:all_candidates_retained",
        _ => "well_conditioned:direct_inverse",
    };
    let ev = Repo::append_event_tx(
        tx,
        branch_id,
        actor,
        &EventPayload::CompensationEvaluated {
            spill_version_id: refs.spill.id.clone(),
            status: verdict.status.clone(),
            det: verdict.det,
            cond: verdict.cond,
            candidate_count: verdict.candidates.len(),
            chosen_index: verdict.chosen_index,
            rule: rule.into(),
        },
    )?;
    Repo::upsert_verdict_tx(tx, branch_id, verdict, &ev.id)?;

    let lineage: LineageSet =
        serde_json::from_value(refs.lineage.data.clone()).map_err(|e| e.to_string())?;
    let gates: Vec<PolygonGate> = refs
        .gates
        .iter()
        .map(|v| serde_json::from_value::<PolygonGate>(v.data.clone()).map_err(|e| e.to_string()))
        .collect::<Result<_, _>>()?;
    let tube = tube_from_conn(tx, tube_material_id)?;
    let channel_names = tube.channel_names;

    let order = topo_order(&gates, &lineage.links);
    let n_total = derived.derived.len() as i64;
    let universe: Vec<i64> = (0..n_total).collect();
    let mut recorded = Vec::new();
    let mut by_gate: std::collections::HashMap<String, GateResultRow> = std::collections::HashMap::new();

    for gid in order {
        let gate_version = refs
            .gates
            .iter()
            .find(|v| {
                serde_json::from_value::<PolygonGate>(v.data.clone())
                    .map(|g| g.gate_id == gid)
                    .unwrap_or(false)
            })
            .unwrap();
        let gate: PolygonGate = serde_json::from_value(gate_version.data.clone()).unwrap();
        let parent_id = lineage
            .links
            .iter()
            .find(|l| l.gate_id == gid)
            .and_then(|l| l.parent_gate_id.clone());
        let (parent_version_id, denominator_id, parent_members, denominator) = match &parent_id {
            None => (None, None, universe.clone(), n_total),
            Some(pgid) => {
                let pv = refs
                    .gates
                    .iter()
                    .find(|v| {
                        serde_json::from_value::<PolygonGate>(v.data.clone())
                            .map(|g| &g.gate_id == pgid)
                            .unwrap_or(false)
                    })
                    .map(|v| v.id.clone())
                    .unwrap();
                let pres = by_gate.get(pgid).expect("topo order");
                (
                    Some(pv),
                    Some(pres.id.clone()),
                    pres.included_indices.clone(),
                    pres.count.max(0),
                )
            }
        };

        let uses_fluor = FLUOR_CHANNELS
            .iter()
            .any(|f| gate.basis.x_channel == *f || gate.basis.y_channel == *f);
        let blocked = uses_fluor && derived.outcome.chosen_inverse.is_none();

        let (members, blocked_reason) = if blocked {
            (
                crate::compute::GateMembership {
                    gate_version_id: gate_version.id.clone(),
                    included_event_indices: vec![],
                    boundary_included: vec![],
                    boundary_excluded: vec![],
                },
                format!(
                    "补偿矩阵状态 {} 且未采纳逆矩阵候选；群体不确定，不给出百分比",
                    verdict.status
                ),
            )
        } else {
            let transformed: Vec<Vec<Option<f64>>> =
                derived.derived.iter().map(|d| d.transformed.clone()).collect();
            let m = evaluate_gate(
                &gate,
                &gate_version.id,
                &transformed,
                &channel_names,
                &parent_members,
            )
            .map_err(|e| crate::store::err("GATE_EVAL", e))?;
            (m, String::new())
        };

        let count = if blocked { -1 } else { members.included_event_indices.len() as i64 };
        let percent = if blocked {
            -1.0
        } else if denominator == 0 {
            0.0
        } else {
            100.0 * count as f64 / denominator as f64
        };

        let n = NewGateResult {
            gate_version_id: gate_version.id.clone(),
            gate_entity_id: gid.clone(),
            lineage_version_id: refs.lineage.id.clone(),
            transform_version_id: refs.transform.id.clone(),
            spill_version_id: refs.spill.id.clone(),
            parent_gate_version_id: parent_version_id,
            denominator_result_id: row_opt_id(denominator_id),
            count,
            denominator,
            percent,
            included_indices: members.included_event_indices,
            boundary_included: members.boundary_included,
            boundary_excluded: members.boundary_excluded,
            blocked,
            blocked_reason,
        };
        let row = Repo::insert_gate_result_tx(tx, branch_id, n)?;
        Repo::append_event_tx(
            tx,
            branch_id,
            actor,
            &EventPayload::GateResultRecorded {
                result_id: row.id.clone(),
                gate_version_id: row.gate_version_id.clone(),
                parent_gate_version_id: row.parent_gate_version_id.clone(),
                denominator_result_id: row.denominator_result_id.clone(),
                count: row.count,
                denominator: row.denominator,
                percent: row.percent,
            },
        )?;
        recorded.push(row.clone());
        by_gate.insert(gid, row);
    }
    Ok(recorded)
}

fn row_opt_id(v: Option<String>) -> Option<String> {
    v
}

pub struct CurrentRefs {
    pub spill: EntityVersion,
    pub transform: EntityVersion,
    pub gates: Vec<EntityVersion>,
    pub lineage: EntityVersion,
}

/// 解析当前版本引用，缺失任何一类都报错（基线导入后应齐备）。
pub fn current_refs(repo: &Repo, branch_id: &str) -> crate::store::StoreResult<CurrentRefs> {
    let spills = repo.list_current_versions(branch_id, EntityKind::Spillover)?;
    let transforms = repo.list_current_versions(branch_id, EntityKind::Transform)?;
    let gates = repo.list_current_versions(branch_id, EntityKind::Gate)?;
    let lineages = repo.list_current_versions(branch_id, EntityKind::Lineage)?;
    Ok(CurrentRefs {
        spill: spills
            .into_iter()
            .next()
            .ok_or_else(|| crate::store::err("MISSING_SPILLOVER", "分支尚无 spillover 版本"))?,
        transform: transforms
            .into_iter()
            .next()
            .ok_or_else(|| crate::store::err("MISSING_TRANSFORM", "分支尚无 transform 版本"))?,
        gates,
        lineage: lineages
            .into_iter()
            .next()
            .ok_or_else(|| crate::store::err("MISSING_LINEAGE", "分支尚无 lineage 版本"))?,
    })
}

pub fn chosen_index(repo: &Repo, branch_id: &str, spill_version_id: &str) -> Option<usize> {
    repo.active_decision(branch_id, spill_version_id)
        .ok()
        .flatten()
        .and_then(|(_id, decision, idx)| {
            if decision == "accepted" {
                idx.map(|v| v as usize)
            } else {
                None
            }
        })
}

/// 门拓扑序（父在前）。
pub fn topo_order(gates: &[PolygonGate], links: &[GateLink]) -> Vec<String> {
    let parent: std::collections::HashMap<&str, Option<&str>> = links
        .iter()
        .map(|l| (l.gate_id.as_str(), l.parent_gate_id.as_deref()))
        .collect();
    let mut order: Vec<String> = Vec::new();
    let ids: Vec<String> = gates.iter().map(|g| g.gate_id.clone()).collect();
    fn visit(
        id: &str,
        ids: &[String],
        parent: &std::collections::HashMap<&str, Option<&str>>,
        order: &mut Vec<String>,
    ) {
        if order.iter().any(|x| x == id) {
            return;
        }
        if let Some(Some(p)) = parent.get(id) {
            if ids.iter().any(|x| x == p) {
                visit(p, ids, parent, order);
            }
        }
        order.push(id.to_string());
    }
    for id in &ids {
        visit(id, &ids, &parent, &mut order);
    }
    order
}

/// 在一个事务中，依据当前版本计算并写入每个门的（不可变）结果。
/// 分母钉住父群体版本（parent 结果）。补偿阻断时荧光门写 blocked 行（percent=-1）。
pub fn compute_and_record(
    tx: &rusqlite::Transaction,
    repo_for_derive: &Repo,
    branch_id: &str,
    refs: &CurrentRefs,
    tube_material_id: &str,
    actor: &str,
) -> crate::store::StoreResult<Vec<GateResultRow>> {
    let spill = spill_from_version(&refs.spill)?;
    let transform = transform_from_version(&refs.transform)?;
    let chosen = chosen_index(repo_for_derive, branch_id, &refs.spill.id);
    let derived = load_derived(
        repo_for_derive,
        tube_material_id,
        &spill,
        &refs.spill.id,
        chosen,
        &transform,
    )?;

    // 写补偿判定。
    let verdict = &derived.outcome.verdict;
    let auto_rule = match verdict.status.as_str() {
        "singular" => {
            "near_singular_or_singular_matrix_must_not_be_silently_inverted:singular,no_direct_inverse"
        }
        "near_singular" => {
            "near_singular_matrix_must_not_be_silently_inverted:all_candidates_retained"
        }
        _ => "well_conditioned_direct_inverse",
    };
    let ev = Repo::append_event_tx(
        tx,
        branch_id,
        actor,
        &EventPayload::CompensationEvaluated {
            spill_version_id: refs.spill.id.clone(),
            status: verdict.status.clone(),
            det: verdict.det,
            cond: verdict.cond,
            candidate_count: verdict.candidates.len(),
            chosen_index: verdict.chosen_index,
            rule: auto_rule.into(),
        },
    )?;
    Repo::upsert_verdict_tx(tx, branch_id, verdict, &ev.id)?;

    let lineage: LineageSet =
        serde_json::from_value(refs.lineage.data.clone()).map_err(|e| e.to_string())?;
    let gates: Vec<PolygonGate> = refs
        .gates
        .iter()
        .map(|v| serde_json::from_value::<PolygonGate>(v.data.clone()).map_err(|e| e.to_string()))
        .collect::<Result<_, _>>()?;
    let channel_names = parse_tube(&repo_for_derive.material_content(tube_material_id)?)
        .channel_names;

    let order = topo_order(&gates, &lineage.links);
    let n_total = derived.derived.len() as i64;
    let universe: Vec<i64> = (0..n_total).collect();
    let mut recorded = Vec::new();
    let mut by_gate: std::collections::HashMap<String, GateResultRow> = std::collections::HashMap::new();

    for gid in order {
        let gate_version = refs
            .gates
            .iter()
            .find(|v| {
                serde_json::from_value::<PolygonGate>(v.data.clone())
                    .map(|g| g.gate_id == gid)
                    .unwrap_or(false)
            })
            .unwrap();
        let gate: PolygonGate = serde_json::from_value(gate_version.data.clone()).unwrap();
        let parent_id = lineage
            .links
            .iter()
            .find(|l| l.gate_id == gid)
            .and_then(|l| l.parent_gate_id.clone());
        let (parent_version_id, denominator_id, parent_members, denominator) = match &parent_id {
            None => (None, None, universe.clone(), n_total),
            Some(pgid) => {
                let pv = refs
                    .gates
                    .iter()
                    .find(|v| {
                        serde_json::from_value::<PolygonGate>(v.data.clone())
                            .map(|g| &g.gate_id == pgid)
                            .unwrap_or(false)
                    })
                    .map(|v| v.id.clone())
                    .unwrap();
                let pres = by_gate.get(pgid).expect("topo order");
                (
                    Some(pv),
                    Some(pres.id.clone()),
                    pres.included_indices.clone(),
                    pres.count.max(0),
                )
            }
        };

        // 补偿阻断：只要任一门轴通道是荧光通道且未裁定/非良态，整个门 blocked。
        let uses_fluor = FLUOR_CHANNELS
            .iter()
            .any(|f| gate.basis.x_channel == *f || gate.basis.y_channel == *f);
        let blocked = uses_fluor && derived.outcome.chosen_inverse.is_none();

        let (members, blocked_reason) = if blocked {
            (
                crate::compute::GateMembership {
                    gate_version_id: gate_version.id.clone(),
                    included_event_indices: vec![],
                    boundary_included: vec![],
                    boundary_excluded: vec![],
                },
                format!(
                    "补偿矩阵状态 {} 且未采纳逆矩阵候选；群体不确定，不给出百分比",
                    verdict.status
                ),
            )
        } else {
            let transformed: Vec<Vec<Option<f64>>> =
                derived.derived.iter().map(|d| d.transformed.clone()).collect();
            let m = evaluate_gate(
                &gate,
                &gate_version.id,
                &transformed,
                &channel_names,
                &parent_members,
            )
            .map_err(|e| crate::store::err("GATE_EVAL", e))?;
            (m, String::new())
        };

        let count = if blocked { -1 } else { members.included_event_indices.len() as i64 };
        let percent = if blocked {
            -1.0
        } else if denominator == 0 {
            0.0
        } else {
            100.0 * count as f64 / denominator as f64
        };

        let n = NewGateResult {
            gate_version_id: gate_version.id.clone(),
            gate_entity_id: gid.clone(),
            lineage_version_id: refs.lineage.id.clone(),
            transform_version_id: refs.transform.id.clone(),
            spill_version_id: refs.spill.id.clone(),
            parent_gate_version_id: parent_version_id,
            denominator_result_id: denominator_id,
            count,
            denominator,
            percent,
            included_indices: members.included_event_indices,
            boundary_included: members.boundary_included,
            boundary_excluded: members.boundary_excluded,
            blocked,
            blocked_reason,
        };
        let row = Repo::insert_gate_result_tx(tx, branch_id, n)?;
        let ev = Repo::append_event_tx(
            tx,
            branch_id,
            actor,
            &EventPayload::GateResultRecorded {
                result_id: row.id.clone(),
                gate_version_id: row.gate_version_id.clone(),
                parent_gate_version_id: row.parent_gate_version_id.clone(),
                denominator_result_id: row.denominator_result_id.clone(),
                count: row.count,
                denominator: row.denominator,
                percent: row.percent,
            },
        )?;
        let _ = ev;
        recorded.push(row.clone());
        by_gate.insert(gid, row);
    }
    Ok(recorded)
}
