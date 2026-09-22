//! 读模型：从分支事件流重建“当时 refs 与可见结论”，不拿最新结果覆盖历史。

use crate::db::Store;
use crate::engine::*;

use crate::engine::assess_compensation;
use crate::events::{Refs, StoredEvent};
use crate::matrix::InvertStatus;
use crate::service::*;
use rusqlite::params;
use serde_json::{json, Value};

impl Store {
    /// 分支在指定事件处的 refs（回到旧事件即恢复当时依赖版本）。
    pub fn refs_at(&self, branch: &str, at_event: Option<&str>) -> Refs {
        let events = self.events_of(branch);
        let mut refs = Refs::default();
        for ev in &events {
            if refs_is_set(&ev.refs) {
                refs = ev.refs.clone();
            }
            if Some(ev.event_id.as_str()) == at_event {
                break;
            }
        }
        refs
    }

    pub fn branch_view(&self, branch: &str, at_event: Option<&str>) -> BranchView {
        let events = self.events_of(branch);
        let mut refs = Refs::default();
        let mut stop = None;
        for ev in &events {
            if refs_is_set(&ev.refs) {
                refs = ev.refs.clone();
            }
            if Some(ev.event_id.as_str()) == at_event {
                stop = Some(ev.event_id.clone());
                break;
            }
        }
        let loaded = self.load_materials();
        let bundle = service_bundle_at(&loaded, &refs);
        let label = self
            .list_branches()
            .into_iter()
            .find(|b| b.branch_id == branch)
            .map(|b| b.label)
            .unwrap_or_else(|| branch.to_string());
        let tip = self.tip(branch);

        let mut comp_report = None;
        let mut points = Vec::new();
        let mut pops = Vec::new();
        if let Some(b) = &bundle {
            let report = assess_compensation(b);
            comp_report = Some(report_to_json(&report));
            if matches!(report.status, InvertStatus::Ok) {
                let pts = evaluate_points(b, &report);
                let pmap = evaluate_populations(b, &pts);
                points = json!(pts).as_array().cloned().unwrap_or_default();
                pops = pmap.values().map(|p| json!(p)).collect();
            }
        }
        // 当时（或当前）结论行：来自 appraisal_rows，stale 行原样保留
        let rows = self.appraisal_rows(branch, stop.as_deref());
        let candidates = self.candidates(branch);
        let gate_json: Vec<Value> = bundle
            .as_ref()
            .map(|b| b.gates.iter().map(|g| json!(g)).collect())
            .unwrap_or_default();
        let hier_json = bundle.map(|b| json!(b.hierarchy)).unwrap_or(json!(null));
        let diag: Vec<Value> = points
            .iter()
            .filter_map(|p| {
                let d = p.get("diagnostics")?.as_object()?;
                if d.is_empty() { None } else { Some(json!({"event_id": p["event_id"], "diagnostics": d})) }
            })
            .collect();
        BranchView {
            branch_id: branch.to_string(),
            label,
            tip_event_id: tip,
            refs,
            compensation_report: comp_report,
            points,
            populations: rows,
            candidates,
            events: events.iter().map(event_json).collect(),
            gates: gate_json,
            hierarchy: hier_json,
            diagnostics: diag,
        }
    }

    fn appraisal_rows(
        &self,
        branch: &str,
        stop: Option<&str>,
    ) -> Vec<AppraisalRowOut> {
        let c = self.conn.lock().unwrap();
        let mut sql = String::from(
            "SELECT row_uid,gate_id,gate_ref,parent_ref,denominator_ref,numerator,denominator,
                    fraction,status,detail_json,stale_reason,event_id
             FROM appraisal_rows WHERE branch_id=?1",
        );
        if stop.is_some() {
            sql.push_str(" AND event_id IN (SELECT event_id FROM events WHERE branch_id=?1 AND seq <= (SELECT seq FROM events WHERE event_id=?2))");
        }
        sql.push_str(" ORDER BY event_id, gate_id");
        let stop = stop.unwrap_or("");
        let mut stmt = c.prepare(&sql).unwrap();
        let mapper = |r: &rusqlite::Row<'_>| {
            let detail: Value = serde_json::from_str(&r.get::<_, String>(9)?).unwrap_or(json!({}));
            let numerator: i64 = r.get(5)?;
            let denominator: i64 = r.get(6)?;
            let fraction: f64 = r.get(7)?;
            Ok(AppraisalRowOut {
                row_uid: r.get(0)?,
                gate_id: r.get(1)?,
                gate_ref: r.get(2)?,
                gate_name: r.get::<_,Option<String>>(2)?.unwrap_or_default(),
                parent_ref: r.get(3)?,
                denominator_ref: r.get(4)?,
                numerator,
                denominator,
                fraction,
                percent: if denominator == 0 { 0.0 } else { fraction * 100.0 },
                status: r.get(8)?,
                stale_reason: r.get(10)?,
                members: detail.get("members").and_then(|v| v.as_array()).map(|a| {
                    a.iter().filter_map(|x| x.as_str().map(String::from)).collect()
                }).unwrap_or_default(),
                event_id: r.get(11)?,
            })
        };
        if stop.is_empty() {
            stmt.query_map(params![branch], mapper).unwrap().map(|r| r.unwrap()).collect()
        } else {
            stmt.query_map(params![branch, stop], mapper).unwrap().map(|r| r.unwrap()).collect()
        }
    }

    fn candidates(&self, branch: &str) -> Vec<CandidateOut> {
        let c = self.conn.lock().unwrap();
        let mut stmt = c
            .prepare(
                "SELECT candidate_uid,group_id,candidate_id,kind,status,detail_json,event_id
                 FROM candidates WHERE branch_id=?1 ORDER BY rowid",
            )
            .unwrap();
        stmt.query_map([branch], |r| {
            Ok(CandidateOut {
                candidate_uid: r.get(0)?,
                group_id: r.get(1)?,
                candidate_id: r.get(2)?,
                kind: r.get(3)?,
                status: r.get(4)?,
                detail: serde_json::from_str(&r.get::<_, String>(5)?).unwrap(),
                event_id: r.get(6)?,
            })
        })
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
    }
}

fn refs_is_set(r: &Refs) -> bool {
    r.raw_events.is_some() || r.compensation.is_some() || !r.gates.is_empty()
}

pub(crate) fn service_bundle_at(
    loaded: &LoadedMaterials,
    refs: &Refs,
) -> Option<MaterialBundle> {
    crate::engine::bundle_for(loaded, refs)
}

fn event_json(ev: &StoredEvent) -> Value {
    json!({
        "event_id": ev.event_id,
        "seq": ev.seq,
        "parent_event_id": ev.parent_event_id,
        "kind": ev.kind,
        "payload": ev.payload,
        "refs": ev.refs,
        "result": ev.result,
    })
}

fn report_to_json(r: &crate::matrix::InvertReport) -> Value {
    json!({
        "status": format!("{:?}", r.status),
        "kappa": r.kappa,
        "determinant": r.determinant,
        "candidate_count": r.candidates.len(),
        "candidates": r.candidates.iter().map(|c| json!({
            "candidate_id": c.candidate_id, "kind": c.kind, "rationale": c.rationale,
            "has_matrix": c.matrix.is_some(),
        })).collect::<Vec<_>>(),
    })
}
