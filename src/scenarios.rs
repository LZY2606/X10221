//! 六个边界场景动作。每个动作追加新事件，绝不改写旧事件/原始数据。

use crate::db::Store;
use crate::engine::downstream_gates;
use crate::events::{EventKind, Refs};
use crate::ingest;
use crate::matrix::{assess as assess_matrix, KAPPA_LIMIT};
use rusqlite::params;
use serde_json::{json, Value};

fn case_json() -> Value {
    serde_json::from_str(crate::service::CASE_JSON).unwrap()
}

impl Store {
    /// 场景2：尝试引入近奇异补偿矩阵。拒绝静默求逆，登记全部合法候选。
    pub fn scenario_near_singular(&self, branch: &str, expected_parent: Option<&str>) -> Result<Value, String> {
        let refs = self.refs_at(branch, None);
        let loaded = self.load_materials();
        let mut new_refs = refs.clone();
        new_refs.compensation = Some("mat-compensation/v2".into());
        let bundle = crate::engine::bundle_for(&loaded, &new_refs).ok_or("materials missing")?;
        let report = assess_matrix(&bundle.comp.rows);
        let tip = self.tip(branch);
        let exp = expected_parent.map(String::from).or(tip.clone());
        // 事件先记录“尝试评估并被规则拒绝”
        let ev = self
            .append_event(
                branch,
                EventKind::InversionRefused,
                json!({
                    "rule": "near-singular compensation matrix must not be silently inverted",
                    "compensation_ref": "mat-compensation/v2",
                    "kappa_limit": KAPPA_LIMIT,
                    "status": format!("{:?}", report.status),
                }),
                &new_refs,
                json!({
                    "kappa": report.kappa,
                    "determinant": report.determinant,
                    "candidate_count": report.candidates.len(),
                    "decision_required": true,
                }),
                exp.as_deref(),
            )
            .map_err(|e| e.message)?;
        {
            let mut c = self.conn.lock().unwrap();
            for cand in &report.candidates {
                let uid = format!("cand-{}-{}", ev.event_id, cand.candidate_id);
                c.execute(
                    "INSERT INTO candidates(candidate_uid,branch_id,group_id,candidate_id,kind,status,detail_json,event_id)
                     VALUES(?1,?2,?3,?4,?5,'proposed',?6,?7)",
                    params![
                        uid, branch, "compensation-v2", cand.candidate_id, cand.kind,
                        json!({"rationale": cand.rationale, "matrix": cand.matrix}).to_string(),
                        ev.event_id
                    ],
                )
                .unwrap();
            }
        }
        Ok(json!({"event_id": ev.event_id, "refused": true, "kappa": report.kappa,
                  "candidates": report.candidates.iter().map(|c| c.candidate_id.clone()).collect::<Vec<_>>()}))
    }

    /// 采纳/拒绝某个候选（近奇异补偿时显式人工裁定）。
    pub fn decide_candidate(
        &self,
        branch: &str,
        candidate_id: &str,
        adopt: bool,
        expected_parent: Option<&str>,
    ) -> Result<Value, String> {
        let refs = self.refs_at(branch, None);
        let tip = self.tip(branch);
        let exp = expected_parent.map(String::from).or(tip);
        // 采纳岭回归候选：把其矩阵作为新补偿版本写入 materials（人工裁定结果可追溯）
        let mut result_refs = refs.clone();
        let mut payload = json!({"candidate_id": candidate_id, "adopted": adopt});
        if adopt && candidate_id == "fb-ridge-1e-4" {
            let loaded = self.load_materials();
            let near = loaded
                .comp
                .get(&("mat-compensation".to_string(), 2))
                .ok_or("near-singular matrix missing")?;
            let report = assess_matrix(&near.rows);
            let ridge = report
                .candidates
                .iter()
                .find(|c| c.candidate_id == candidate_id)
                .and_then(|c| c.matrix.clone())
                .ok_or("candidate matrix missing")?;
            let mut chosen = near.clone();
            chosen.version = 3;
            chosen.rows = ridge;
            let v = serde_json::to_value(&chosen).unwrap();
            let hash = crate::repository::sha256_hex(&v);
            {
                let mut c = self.conn.lock().unwrap();
                c.execute(
                    "INSERT OR IGNORE INTO materials(material_type,material_id,version,payload_json,sha256,ingest_seq)
                     VALUES('compensation','mat-compensation',3,?1,?2,900)",
                    params![v.to_string(), hash],
                )
                .unwrap();
            }
            result_refs.compensation = Some("mat-compensation/v3".into());
            payload["adopted_matrix_ref"] = json!("mat-compensation/v3");
        }
        let kind = if adopt { EventKind::CandidateAdopted } else { EventKind::CandidateRejected };
        let ev = self
            .append_event(branch, kind, payload, &result_refs,
                json!({"note": "human adjudication; other candidates retained, not deleted"}),
                exp.as_deref())
            .map_err(|e| e.message)?;
        {
            let mut c = self.conn.lock().unwrap();
            let status = if adopt { "adopted" } else { "rejected" };
            c.execute(
                "UPDATE candidates SET status=?1 WHERE branch_id=?2 AND candidate_id=?3 AND group_id='compensation-v2'",
                params![status, branch, candidate_id],
            )
            .unwrap();
        }
        if adopt {
            let id = self.commit_appraisal(branch, &result_refs, &ev.event_id, "post-adjudication")?;
            return Ok(json!({"event_id": ev.event_id, "appraisal_event_id": id}));
        }
        Ok(json!({"event_id": ev.event_id}))
    }

    /// 场景3：变换参数修订（FITC cofactor 150->300）。失效范围=荧光门及其子孙。
    pub fn scenario_transform_revision(&self, branch: &str, expected_parent: Option<&str>) -> Result<Value, String> {
        self.parameter_revision(branch, "mat-transform/v2", EventKind::TransformRevised,
            "transform", "transform cofactor revised (FITC 150->300)", expected_parent)
    }

    /// 场景4：只改一个直接来源 e02/PerCP。失效范围按依赖图精确限定。
    pub fn scenario_source_patch(&self, branch: &str, expected_parent: Option<&str>) -> Result<Value, String> {
        self.parameter_revision(branch, "mat-raw-events/v2", EventKind::SourcePatched,
            "raw_events", "single direct source revised: event e02 channel PerCP-A raw value",
            expected_parent)
    }

    /// 场景5：门嵌套改名（gate-t v1->v2，几何不变）。
    pub fn scenario_gate_rename(&self, branch: &str, expected_parent: Option<&str>) -> Result<Value, String> {
        let refs = self.refs_at(branch, None);
        let mut new_refs = refs.clone();
        new_refs.gates.insert("gate-t".into(), "gate-t/v2".into());
        let tip = self.tip(branch);
        let exp = expected_parent.map(String::from).or(tip);
        let ev = self
            .append_event(
                branch, EventKind::GateRenamed,
                json!({"gate_id": "gate-t", "old_ref": "gate-t/v1", "new_ref": "gate-t/v2",
                       "geometry_unchanged": true}),
                &new_refs,
                json!({"note": "rename in hierarchy; polygon and lineage preserved, old version still readable"}),
                exp.as_deref(),
            )
            .map_err(|e| e.message)?;
        let id = self.commit_appraisal(branch, &new_refs, &ev.event_id, "after-gate-rename")?;
        Ok(json!({"event_id": ev.event_id, "appraisal_event_id": id}))
    }

    /// 场景6：修订上游 lymph 门；下游行标记 stale 而不覆盖，并提出重估候选。
    pub fn scenario_upstream_revision(&self, branch: &str, expected_parent: Option<&str>) -> Result<Value, String> {
        let refs = self.refs_at(branch, None);
        let mut new_refs = refs.clone();
        new_refs.gates.insert("gate-lymph".into(), "gate-lymph/v2".into());
        let tip = self.tip(branch);
        let exp = expected_parent.map(String::from).or(tip);
        let loaded = self.load_materials();
        let bundle = crate::engine::bundle_for(&loaded, &new_refs).ok_or("materials missing")?;
        let affected = downstream_gates(&["gate:gate-lymph".into()], &bundle);
        let ev = self
            .append_event(
                branch, EventKind::GateRevised,
                json!({"gate_id": "gate-lymph", "old_ref": "gate-lymph/v1", "new_ref": "gate-lymph/v2"}),
                &new_refs,
                json!({"stale_scope": affected, "policy": "downstream marked stale, never overwritten"}),
                exp.as_deref(),
            )
            .map_err(|e| e.message)?;
        // 标记受影响群的“最近 current 行”为 stale（旧行保留，分数仍可见）
        {
            let mut c = self.conn.lock().unwrap();
            for gid in &affected {
                c.execute(
                    "UPDATE appraisal_rows SET status='stale',
                        stale_reason='upstream gate-lymph/v2 revised; denominator/geometry may change'
                     WHERE row_uid IN (
                        SELECT row_uid FROM appraisal_rows
                        WHERE branch_id=?1 AND gate_id=?2 AND status='current'
                        ORDER BY event_id DESC LIMIT 1
                     )",
                    params![branch, gid],
                )
                .unwrap();
            }
            // 为每个受影响群提出“可重估候选”，保留无法唯一决定时的全部合法候选
            for gid in &affected {
                let uid = format!("cand-{}-reeval-{}", ev.event_id, gid);
                c.execute(
                    "INSERT INTO candidates(candidate_uid,branch_id,group_id,candidate_id,kind,status,detail_json,event_id)
                     VALUES(?1,?2,?3,'reevaluate','re-evaluation','proposed',?4,?5)",
                    params![uid, branch, format!("stale:{gid}"),
                        json!({"gate_id": gid, "upstream_ref": "gate-lymph/v2"}).to_string(),
                        ev.event_id],
                )
                .unwrap();
            }
        }
        let _ = self.append_event(
            branch, EventKind::StaleMarked,
            json!({"stale_gates": affected}), &new_refs,
            json!({"note": "old conclusions retained and labeled stale; no global replacement"}),
            Some(&ev.event_id),
        ).map_err(|e| e.message)?;
        Ok(json!({"event_id": ev.event_id, "stale_gates": affected}))
    }

    fn parameter_revision(
        &self,
        branch: &str,
        new_ref: &str,
        kind: EventKind,
        material_type: &str,
        note: &str,
        expected_parent: Option<&str>,
    ) -> Result<Value, String> {
        let refs = self.refs_at(branch, None);
        let mut new_refs = refs.clone();
        let key = match material_type {
            "transform" => { new_refs.transform = Some(new_ref.into()); "transform" }
            "raw_events" => { new_refs.raw_events = Some(new_ref.into()); "raw_events" }
            _ => return Err("unsupported".into()),
        };
        let tip = self.tip(branch);
        let exp = expected_parent.map(String::from).or(tip);
        let loaded = self.load_materials();
        let bundle = crate::engine::bundle_for(&loaded, &new_refs).ok_or("materials missing")?;
        let affected = downstream_gates(&[key.to_string()], &bundle);
        let ev = self
            .append_event(branch, kind, json!({"new_ref": new_ref, "note": note}),
                &new_refs, json!({"invalidated_scope": affected}),
                exp.as_deref())
            .map_err(|e| e.message)?;
        let id = self.commit_appraisal(branch, &new_refs, &ev.event_id, note)?;
        Ok(json!({"event_id": ev.event_id, "appraisal_event_id": id, "invalidated_scope": affected}))
    }
}

// 触发 ingest 模块链接（校验函数在测试中直接使用）
#[allow(dead_code)]
fn _link() { let _ = ingest::validate_case(&case_json()); }
