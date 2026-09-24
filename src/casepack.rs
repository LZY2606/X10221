//! 离线案例包「流式边界室」的加载与重放。
//! 重放实际经过：矩阵奇异、极端信号、边界点、门嵌套改名、上游过期传播、同版本并发修改。

use crate::model::*;
use crate::store::{ImportArtifact, Store, StoreError};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;

#[derive(Debug, Deserialize)]
pub struct CasePack {
    pub id: String,
    pub title: String,
    pub narrative: String,
    pub artifacts: Vec<CaseArtifact>,
    pub baseline: BTreeMap<String, Value>,
    pub revisions: Vec<CaseRevision>,
}

#[derive(Debug, Deserialize)]
pub struct CaseArtifact {
    pub key: String,
    pub kind: String,
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct CaseRevision {
    pub key: String,
    pub note: String,
    pub payload: Value,
}

pub fn casepack_source() -> &'static str {
    include_str!("../casepack/boundary-lab/case.json")
}

pub fn load_casepack() -> Result<CasePack, StoreError> {
    serde_json::from_str(casepack_source()).map_err(|e| StoreError::Msg(format!("casepack parse: {e}")))
}

pub fn casepack_meta() -> Value {
    let cp = load_casepack().expect("embedded casepack must parse");
    json!({"id": cp.id, "title": cp.title, "narrative": cp.narrative})
}

/// 完整重放：导入基线 → 建草案 → 派生 → 裁定 → 四次单源修订 → 冲突决定。
/// 返回每个情形的机器可读结果，供界面展示与测试断言。
pub fn replay(store: &mut Store) -> Result<Value, StoreError> {
    let cp = load_casepack()?;

    // 1. 原子导入基线（六类材料分开版本化）
    let artifacts: Vec<ImportArtifact> = cp
        .artifacts
        .iter()
        .map(|a| ImportArtifact {
            kind: a.kind.clone(),
            name: a.name.clone(),
            payload: cp.baseline.get(&a.key).cloned().unwrap_or(Value::Null),
            raw_repr: serde_json::to_string_pretty(&cp.baseline.get(&a.key)).unwrap_or_default(),
            source: format!("casepack:{}/baseline/{}", cp.id, a.key),
        })
        .collect();
    let import = store.import_bundle(&artifacts)?;
    let artifact_ids: BTreeMap<String, String> = import
        .artifacts
        .iter()
        .map(|a| (a["key"].as_str().unwrap().to_string(), a["id"].as_str().unwrap().to_string()))
        .collect();

    // 2. 两个草案：main 跟随最新，alt-baseline 钉在基线事件上（并排比较用）
    let main = store.create_draft("main", None)?;
    let baseline_head = store.last_event_seq();
    let alt = store.create_draft("alt-baseline", None)?;
    store.conn.execute(
        "UPDATE drafts SET head_event=?1 WHERE id=?2",
        rusqlite::params![baseline_head, alt],
    )?;

    // 3. 基线派生：边界点 → G1/G2 各一对候选；极端信号/无法转换 → 诊断
    let d1 = store.derive_draft(&main)?;
    let cands = store.candidates_of(&main);
    let g1_incl = find_candidate(&cands, "G1", "boundary-inclusive")?;
    let boundary_pair: Vec<String> = cands
        .iter()
        .filter(|c| c.population == "G1" && c.label.starts_with("boundary"))
        .map(|c| c.id.clone())
        .collect();
    let diag = store.diagnostics();
    let extreme: Vec<&crate::store::DiagnosticRow> =
        diag.iter().filter(|d| d.code == "extreme-signal").collect();
    let unconvertible: Vec<&crate::store::DiagnosticRow> =
        diag.iter().filter(|d| d.code == "unconvertible-value").collect();

    // 4. 人工决定 A：采纳 G1 boundary-inclusive（边界点计入）
    let head_a = store.get_draft(&main).unwrap()["head_event"].as_i64().unwrap();
    let adj_a = store.adjudicate(&main, &g1_incl, "accept", head_a, "边界点计入 Lymph")?;

    // 5. 修订 1：上游门多边形 → 上游过期传播（G1、G2 都过期，旧结论保留）
    let poly_id = artifact_ids["polygon"].clone();
    let rev_poly = &cp.revisions[0];
    let inv1 = store.add_version(
        &poly_id,
        rev_poly.payload.clone(),
        &serde_json::to_string_pretty(&rev_poly.payload).unwrap(),
        &format!("casepack:{}/revisions/polygon", cp.id),
    )?;

    // 6. 修订 2：门嵌套改名（gate_id 稳定）
    let hier_id = artifact_ids["hierarchy"].clone();
    let rev_hier = &cp.revisions[1];
    let inv2 = store.add_version(
        &hier_id,
        rev_hier.payload.clone(),
        &serde_json::to_string_pretty(&rev_hier.payload).unwrap(),
        &format!("casepack:{}/revisions/hierarchy", cp.id),
    )?;

    // 7. 修订 3：局部源数据改动（只波及荧光补偿依赖）
    let meta_id = artifact_ids["meta"].clone();
    let rev_meta = &cp.revisions[2];
    let inv3 = store.add_version(
        &meta_id,
        rev_meta.payload.clone(),
        &serde_json::to_string_pretty(&rev_meta.payload).unwrap(),
        &format!("casepack:{}/revisions/spillover-meta", cp.id),
    )?;
    store.derive_draft(&main)?;

    // 8. 修订 4：近奇异补偿矩阵 → 拒绝求逆，blocked + 未补偿回退候选并存
    let comp_id = artifact_ids["comp"].clone();
    let rev_comp = &cp.revisions[3];
    let inv4 = store.add_version(
        &comp_id,
        rev_comp.payload.clone(),
        &serde_json::to_string_pretty(&rev_comp.payload).unwrap(),
        &format!("casepack:{}/revisions/compensation-matrix", cp.id),
    )?;
    let d_block = store.derive_draft(&main)?;

    // 9. 冲突决定：同一 base_head 上两次裁定，后到者收到版本冲突
    let cands2 = store.candidates_of(&main);
    let fallback = find_candidate(&cands2, "G2", "uncompensated-fallback")?;
    let blocked_g2 = find_candidate(&cands2, "G2", "blocked:base")?;
    let head_b = store.get_draft(&main).unwrap()["head_event"].as_i64().unwrap();
    let adj_b = store.adjudicate(&main, &fallback, "accept", head_b, "采纳未补偿回退候选")?;
    let conflict = match store.adjudicate(&main, &blocked_g2, "reject", head_b, "否定 blocked 候选") {
        Ok(_) => json!({"unexpected": "second adjudication should have conflicted"}),
        Err(StoreError::Conflict(c)) => {
            let payload = json!({
                "rule_refs": ["optimistic-concurrency"],
                "judgment": "version-conflict",
                "draft_id": main,
                "your_base": c.your_base,
                "current_head": c.current_head,
                "their_diff": c.their_diff,
                "your_diff": c.your_diff,
                "affected": [fallback, blocked_g2],
            });
            let seq = store.record_event("conflict", payload.clone())?;
            json!({"event_seq": seq, "detail": payload})
        }
        Err(e) => return Err(e),
    };

    // 10. alt-baseline 用基线版本派生，供并排比较
    let d_alt = store.derive_draft(&alt)?;

    Ok(json!({
        "casepack": {"id": cp.id, "title": cp.title},
        "drafts": {"main": main, "alt_baseline": alt},
        "import": import,
        "scenarios": {
            "boundary-points": {"derivation_event": d1["event_seq"], "g1_candidate_pair": boundary_pair},
            "extreme-signal": {"diagnostics": extreme.iter().map(|d| &d.message).collect::<Vec<_>>()},
            "unconvertible-value": {"diagnostics": unconvertible.iter().map(|d| &d.message).collect::<Vec<_>>()},
            "decision-accepted": {"adjudication": adj_a},
            "upstream-stale-propagation": {"invalidation": inv1},
            "gate-nesting-rename": {"invalidation": inv2},
            "local-source-change": {"invalidation": inv3},
            "near-singular-matrix": {"invalidation": inv4, "derivation": d_block},
            "concurrent-modification": {"accepted": adj_b, "conflict": conflict},
            "alt-baseline-derivation": d_alt,
        }
    }))
}

fn find_candidate(
    cands: &[crate::store::CandidateRec],
    population: &str,
    label: &str,
) -> Result<String, StoreError> {
    cands
        .iter()
        .find(|c| c.population == population && c.label == label)
        .map(|c| c.id.clone())
        .ok_or_else(|| StoreError::Msg(format!("candidate {population}/{label} not found")))
}
