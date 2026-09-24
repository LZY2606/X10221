//! 谱系推导：补偿矩阵采纳守卫、门修订的失效范围、群体计算（分母钉住父群版本）、
//! 多候选裁定与撤销。所有判定都落成机器可读事件。

use serde_json::json;

use crate::compensate;
use crate::gating;
use crate::model::Version;
use crate::store::{Store, StoreError};

pub const BRANCH_MAIN: &str = "main";

/// 从版本载荷里取依赖列表（群体版本记录自己依赖的全部上游版本 id）。
fn dependencies_of(v: &Version) -> Vec<String> {
    v.payload
        .get("dependencies")
        .and_then(|d| d.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
        .unwrap_or_default()
}

/// 失效传播：某条线出现新版本后，把依赖它的群体标记过期（传递闭包），不覆盖旧结论。
/// 返回被标记过期的版本 id。
pub fn propagate_stale(store: &Store, changed_line: &str, cause_event: &str) -> Result<Vec<String>, StoreError> {
    let all = store.all_versions()?;
    let changed_version_ids: std::collections::BTreeSet<String> = all
        .iter()
        .filter(|v| v.line_id == changed_line)
        .map(|v| v.id.clone())
        .collect();
    let mut stale: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    loop {
        let mut grew = false;
        for v in &all {
            if v.kind != "population" || v.status == "stale" || stale.contains(&v.id) {
                continue;
            }
            let deps = dependencies_of(v);
            let hit = deps.iter().any(|d| changed_version_ids.contains(d) || stale.contains(d));
            if hit {
                stale.insert(v.id.clone());
                grew = true;
            }
        }
        if !grew {
            break;
        }
    }
    let stale_ids: Vec<String> = stale.into_iter().collect();
    if !stale_ids.is_empty() {
        store.mark_stale(&stale_ids)?;
        store.record_event(
            "population.stale",
            json!({
                "cause": cause_event,
                "changed_line": changed_line,
                "stale_ids": stale_ids,
                "rule": "upstream_changed_mark_stale_not_overwrite",
            }),
        )?;
    }
    Ok(stale_ids)
}

/// 提交补偿矩阵新版本：先做求逆体检。
/// 近奇异 → 不提交、不静默求逆，记录事件并返回全部合法候选。
pub fn commit_matrix(
    store: &Store,
    line_id: &str,
    branch: &str,
    base_seq: Option<i64>,
    matrix: Vec<Vec<f64>>,
    source: &str,
) -> Result<serde_json::Value, StoreError> {
    match compensate::try_invert(&matrix) {
        Ok((inverse, check)) => {
            let v = store.commit(
                "matrix",
                line_id,
                branch,
                base_seq,
                json!({
                    "matrix": matrix,
                    "inverse": inverse,
                    "determinant": check.determinant,
                    "min_pivot_ratio": check.min_pivot_ratio,
                    "source": source,
                    "unit": "fraction_of_spillover",
                }),
                None,
            )?;
            store.record_event(
                "matrix.adopted",
                json!({
                    "version": v.id,
                    "line_id": line_id,
                    "determinant": check.determinant,
                    "source": source,
                }),
            )?;
            propagate_stale(store, line_id, "matrix_revised")?;
            Ok(json!({"status": "adopted", "version": v}))
        }
        Err(check) => {
            let ev = store.record_event(
                "matrix.rejected_near_singular",
                json!({
                    "line_id": line_id,
                    "source": source,
                    "determinant": check.determinant,
                    "min_pivot_ratio": check.min_pivot_ratio,
                    "diagnostics": check.diagnostics,
                    "candidates": check.candidates,
                    "rule": "near_singular_matrix_must_not_be_silently_inverted",
                }),
            )?;
            Ok(json!({
                "status": "rejected",
                "reason": check,
                "event": ev,
            }))
        }
    }
}

/// 提交门新版本。名称变化不影响几何依赖；几何变化才触发失效传播。
pub fn commit_gate(
    store: &Store,
    line_id: &str,
    branch: &str,
    base_seq: Option<i64>,
    gate: serde_json::Value,
) -> Result<serde_json::Value, StoreError> {
    let head = store.head(line_id, branch)?;
    let geometry_changed = head
        .as_ref()
        .map(|h| h.payload.get("polygon") != gate.get("polygon")
            || h.payload.get("x_channel") != gate.get("x_channel")
            || h.payload.get("y_channel") != gate.get("y_channel"))
        .unwrap_or(true);
    let name_changed = head
        .as_ref()
        .map(|h| h.payload.get("name") != gate.get("name"))
        .unwrap_or(true);
    let v = store.commit("gate", line_id, branch, base_seq, gate, None)?;
    if name_changed && !geometry_changed {
        // 门嵌套改名：身份按版本 id 延续，不触发下游失效。
        store.record_event(
            "gate.renamed",
            json!({
                "version": v.id,
                "line_id": line_id,
                "old_name": head.as_ref().and_then(|h| h.payload.get("name").cloned()),
                "new_name": v.payload.get("name").cloned(),
                "rule": "rename_keeps_geometry_dependencies",
            }),
        )?;
    }
    if geometry_changed {
        store.record_event(
            "gate.geometry_revised",
            json!({"version": v.id, "line_id": line_id}),
        )?;
        let stale = propagate_stale(store, line_id, "gate_geometry_revised")?;
        return Ok(json!({"version": v, "geometry_changed": true, "stale_ids": stale}));
    }
    Ok(json!({"version": v, "geometry_changed": false, "stale_ids": []}))
}

/// 计算群体：分母钉住父群版本（parent_population_version），
/// 依赖列表记录本次计算用到的全部上游版本 id。
#[allow(clippy::too_many_arguments)]
pub fn compute_population(
    store: &Store,
    pop_line: &str,
    branch: &str,
    gate_version: &Version,
    parent_population: Option<&Version>,
    xs: &[f64],
    ys: &[f64],
    extreme_events: &[usize],
    dependencies: Vec<String>,
) -> Result<Version, StoreError> {
    let polygon: Vec<[f64; 2]> = serde_json::from_value(
        gate_version.payload.get("polygon").cloned().unwrap_or(json!([])),
    )
    .map_err(|e| StoreError::BadInput(format!("门多边形非法: {e}")))?;
    let parent_members: Option<Vec<usize>> = parent_population.and_then(|p| {
        p.payload
            .get("member_indices")
            .and_then(|m| serde_json::from_value(m.clone()).ok())
    });
    let result = gating::count_population(
        xs,
        ys,
        &polygon,
        parent_members.as_deref(),
        extreme_events,
        gating::BOUNDARY_EPS,
    );
    let denominator = parent_population
        .map(|p| json!({
            "population_version": p.id,
            "count": p.payload.get("count").cloned().unwrap_or(json!(0)),
        }))
        .unwrap_or_else(|| json!({"population_version": null, "count": xs.len()}));
    let denom_count = denominator.get("count").as_f64().unwrap_or(0.0);
    let percent = if denom_count > 0.0 {
        result.count as f64 / denom_count * 100.0
    } else {
        0.0
    };
    let mut deps = dependencies;
    deps.push(gate_version.id.clone());
    if let Some(p) = parent_population {
        deps.push(p.id.clone());
    }
    store.commit(
        "population",
        pop_line,
        branch,
        None,
        json!({
            "gate_version": gate_version.id,
            "gate_name": gate_version.payload.get("name"),
            "parent_population_version": parent_population.map(|p| p.id.clone()),
            "count": result.count,
            "percent_of_parent": percent,
            "denominator": denominator,
            "boundary_count": result.boundary_count,
            "extreme_count": result.extreme_count,
            "member_indices": result.member_indices,
            "dependencies": deps,
            "rule": "percent_denominator_pinned_to_parent_population_version",
        }),
        None,
    )
}

/// 人工裁定（接受/否定/参数调整）：生成事件，并把裁定结果作为 decision 线的新版本。
/// 被采纳与未采纳的结果都保留在档案里。
pub fn adjudicate(
    store: &Store,
    subject: &str,
    decision: &str,
    rationale: &str,
    candidate: serde_json::Value,
) -> Result<serde_json::Value, StoreError> {
    let line = format!("decision:{subject}");
    let head = store.head(&line, BRANCH_MAIN)?;
    let v = store.commit(
        "decision",
        &line,
        BRANCH_MAIN,
        head.as_ref().map(|h| h.seq),
        json!({
            "subject": subject,
            "decision": decision,
            "rationale": rationale,
            "candidate": candidate,
            "supersedes": head.as_ref().map(|h| h.id.clone()),
        }),
        None,
    )?;
    let ev = store.record_event(
        &format!("adjudication.{decision}"),
        json!({
            "subject": subject,
            "decision_version": v.id,
            "rationale": rationale,
            "candidate": candidate,
        }),
    )?;
    Ok(json!({"decision_version": v, "event": ev}))
}

/// 撤销裁定：恢复前一分支状态（追加一个指回旧结论的新版本，不改写历史）。
pub fn undo_adjudication(store: &Store, subject: &str) -> Result<serde_json::Value, StoreError> {
    let line = format!("decision:{subject}");
    let head = store
        .head(&line, BRANCH_MAIN)?
        .ok_or_else(|| StoreError::NotFound(line.clone()))?;
    let prev_id = head
        .payload
        .get("supersedes")
        .and_then(|s| s.as_str())
        .map(String::from);
    let restored = match &prev_id {
        Some(id) => store.get(id)?.ok_or_else(|| StoreError::NotFound(id.clone()))?,
        None => return Err(StoreError::BadInput("没有可恢复的更早裁定".into())),
    };
    let v = store.commit(
        "decision",
        &line,
        BRANCH_MAIN,
        Some(head.seq),
        json!({
            "subject": subject,
            "decision": restored.payload.get("decision"),
            "rationale": format!("撤销 {}，恢复 {}", head.id, restored.id),
            "candidate": restored.payload.get("candidate"),
            "supersedes": head.id,
            "restored_from": restored.id,
        }),
        None,
    )?;
    let ev = store.record_event(
        "adjudication.undo",
        json!({
            "subject": subject,
            "undone": head.id,
            "restored": restored.id,
            "new_version": v.id,
        }),
    )?;
    Ok(json!({"restored_version": v, "event": ev}))
}
