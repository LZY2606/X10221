//! 应用服务：把“分支当前 refs”物化成可见视图，并把每次复核动作写成 append-only 事件。
//! 规则：
//! 1) 近奇异补偿矩阵不能被默默求逆 -> 只产出拒绝事件 + 全部候选，无 appraisal。
//! 2) 门百分比的分母钉住父群版本（denominator_ref 写入 appraisal_rows）。
//! 3) 修订上游门后，下游结果标记 stale 而不覆盖旧行。

use crate::db::Store;
use crate::domain::*;
use crate::engine::*;
use crate::events::{EventKind, Refs};
use crate::matrix::{assess as assess_matrix, InvertStatus};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;

pub const CASE_JSON: &str = include_str!("case/flow_case.json");
pub const FCS_TEXT: &str = include_str!("case/fcs_text.txt");
pub const MANIFEST: &str = include_str!("case/manifest.json");

#[derive(Debug, Serialize)]
pub struct AppraisalRowOut {
    pub row_uid: String,
    pub gate_id: String,
    pub gate_ref: String,
    pub gate_name: String,
    pub parent_ref: Option<String>,
    pub denominator_ref: String,
    pub numerator: i64,
    pub denominator: i64,
    pub fraction: f64,
    pub percent: f64,
    pub status: String,
    pub stale_reason: Option<String>,
    pub members: Vec<String>,
    pub event_id: String,
}

#[derive(Debug, Serialize)]
pub struct CandidateOut {
    pub candidate_uid: String,
    pub group_id: String,
    pub candidate_id: String,
    pub kind: String,
    pub status: String,
    pub detail: Value,
    pub event_id: String,
}

#[derive(Debug, Serialize)]
pub struct BranchView {
    pub branch_id: String,
    pub label: String,
    pub tip_event_id: Option<String>,
    pub refs: Refs,
    pub compensation_report: Option<Value>,
    pub points: Vec<Value>,
    pub populations: Vec<AppraisalRowOut>,
    pub candidates: Vec<CandidateOut>,
    pub events: Vec<Value>,
    pub gates: Vec<Value>,
    pub hierarchy: Value,
    pub diagnostics: Vec<Value>,
}

fn parse<T: serde::de::DeserializeOwned>(v: &Value) -> T {
    serde_json::from_value(v.clone()).unwrap()
}

fn bundle_at(loaded: &LoadedMaterials, refs: &Refs) -> Option<MaterialBundle> {
    let raw_key = ref_key(refs.raw_events.as_ref()?);
    let meta_key = ref_key(refs.spillover_meta.as_ref()?);
    let comp_key = ref_key(refs.compensation.as_ref()?);
    let tr_key = ref_key(refs.transform.as_ref()?);
    let hier_key = ref_key(refs.hierarchy.as_ref()?);
    let mut gates = Vec::new();
    for gref in refs.gates.values() {
        let key = ref_key(gref);
        gates.push(loaded.gates.get(&key)?.clone());
    }
    Some(MaterialBundle {
        raw: loaded.raw.get(&raw_key)?.clone(),
        meta: loaded.meta.get(&meta_key)?.clone(),
        comp: loaded.comp.get(&comp_key)?.clone(),
        transform: loaded.transform.get(&tr_key)?.clone(),
        gates,
        hierarchy: loaded.hierarchy.get(&hier_key)?.clone(),
    })
}

fn ref_key(r: &str) -> (String, u32) {
    let (id, v) = parse_ref(r);
    (id.to_string(), v)
}

impl Store {
    pub fn baseline_refs(case: &Value) -> Refs {
        let b = &case["baseline_refs"];
        let mut gates = BTreeMap::new();
        for (k, v) in b["gates"].as_object().unwrap() {
            gates.insert(k.clone(), v.as_str().unwrap().to_string());
        }
        Refs {
            raw_events: Some(b["raw_events"].as_str().unwrap().into()),
            spillover_meta: Some(b["spillover_meta"].as_str().unwrap().into()),
            compensation: Some(b["compensation"].as_str().unwrap().into()),
            transform: Some(b["transform"].as_str().unwrap().into()),
            gates,
            hierarchy: Some(b["hierarchy"].as_str().unwrap().into()),
        }
    }

    /// 载入仓库内示例（幂等）：事务写材料 + main 分支 + case.loaded 事件 + 基线评估。
    pub fn load_bundled_case(&self) -> Result<Value, String> {
        let case: Value = serde_json::from_str(CASE_JSON).map_err(|e| e.to_string())?;
        let already = self.list_branches().iter().any(|b| b.branch_id == "main");
        if already {
            return Ok(json!({"status": "already-loaded"}));
        }
        {
            let mut c = self.conn.lock().unwrap();
            let tx = c.transaction().map_err(|e| e.to_string())?;
            // 任何校验失败都会 drop tx 自动回滚，不留半份记录
            crate::ingest::ingest_case_locked(&tx, &case, 1)?;
            tx.execute(
                "INSERT INTO branches(branch_id,parent_branch,base_event_id,tip_event_id,label,created_seq)
                 VALUES('main',NULL,NULL,NULL,'main',0)",
                [],
            )
            .map_err(|e| e.to_string())?;
            tx.commit().map_err(|e| e.to_string())?;
        }
        let refs = Self::baseline_refs(&case);
        let event = self
            .append_event(
                "main",
                EventKind::CaseLoaded,
                json!({
                    "case_id": case["case_id"], "title": case["title"],
                    "fcs_text_sha256": sha_of_text(FCS_TEXT),
                    "manifest": serde_json::from_str::<Value>(MANIFEST).unwrap(),
                }),
                &refs,
                json!({"note": "baseline materials loaded, raw representations preserved"}),
                None,
            )
            .map_err(|e| e.message)?;
        self.commit_appraisal("main", &refs, &event.event_id, "baseline")?;
        Ok(json!({"status": "loaded", "event_id": event.event_id}))
    }

    /// 在指定 refs 上计算群体，并物化当前结论行（全新 event 下插入）。
    pub fn commit_appraisal(
        &self,
        branch: &str,
        refs: &Refs,
        parent_event: &str,
        reason: &str,
    ) -> Result<String, String> {
        let loaded = self.load_materials();
        let bundle = bundle_at(&loaded, refs).ok_or("refs do not resolve to materials")?;
        let report = assess_compensation(&bundle);
        if !matches!(report.status, InvertStatus::Ok) {
            return Err("refuse-appraisal: compensation matrix not safely invertible".into());
        }
        let points = evaluate_points(&bundle, &report);
        let pops = evaluate_populations(&bundle, &points);
        let result = json!({
            "reason": reason,
            "populations": pops.values().map(|p| json!({
                "gate_id": p.gate_id, "gate_ref": p.gate_ref,
                "denominator_ref": p.denominator_ref,
                "numerator": p.numerator, "denominator": p.denominator,
                "percent": round4(p.percent), "members": p.members,
            })).collect::<Vec<_>>(),
        });
        let ev = self
            .append_event(branch, EventKind::AppraisalCommitted, json!({"reason": reason}), refs, result, Some(parent_event))
            .map_err(|e| e.message)?;
        {
            let mut c = self.conn.lock().unwrap();
            for p in pops.values() {
                let uid = format!("row-{}@{}-{}", p.gate_id, ev.event_id, p.gate_ref);
                c.execute(
                    "INSERT INTO appraisal_rows
                     (row_uid,branch_id,event_id,gate_id,gate_ref,parent_ref,denominator_ref,
                      numerator,denominator,fraction,status,detail_json,stale_reason)
                     VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,'current',?11,NULL)",
                    params![
                        uid, branch, ev.event_id, p.gate_id, p.gate_ref, p.parent_ref,
                        p.denominator_ref, p.numerator as i64, p.denominator as i64,
                        round4(p.fraction),
                        json!({"members": p.members, "coord_basis": p.coord_basis}).to_string()
                    ],
                )
                .unwrap();
            }
        }
        Ok(ev.event_id)
    }
}

fn round4(x: f64) -> f64 {
    (x * 1e4).round() / 1e4
}

fn sha_of_text(t: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(t.as_bytes()))
}
