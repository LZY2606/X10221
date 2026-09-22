//! 门结果、补偿裁定、人工决定与失效标记。

use super::ids::{decision_id, now_ms, result_id};
use super::repo::{err, Repo, StoreResult};
use crate::domain::{CompensationVerdict, EventPayload};
use rusqlite::params;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateResultRow {
    pub id: String,
    pub gate_version_id: String,
    pub gate_entity_id: String,
    pub branch_id: String,
    pub lineage_version_id: String,
    pub transform_version_id: String,
    pub spill_version_id: String,
    pub parent_gate_version_id: Option<String>,
    pub denominator_result_id: Option<String>,
    pub count: i64,
    pub denominator: i64,
    pub percent: f64,
    pub included_indices: Vec<i64>,
    pub boundary_included: Vec<i64>,
    pub boundary_excluded: Vec<i64>,
    pub stale: bool,
    pub stale_reason: String,
    pub blocked: bool,
    pub blocked_reason: String,
    pub created_at: i64,
}

fn parse_indices(s: &str) -> Vec<i64> {
    if s.is_empty() {
        Vec::new()
    } else {
        s.split(',').filter_map(|x| x.parse().ok()).collect()
    }
}
fn join_indices(v: &[i64]) -> String {
    v.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",")
}

fn row_to_result(r: &rusqlite::Row) -> rusqlite::Result<GateResultRow> {
    let inc: String = r.get(13)?;
    let bi: String = r.get(14)?;
    let be: String = r.get(15)?;
    Ok(GateResultRow {
        id: r.get(0)?,
        gate_version_id: r.get(1)?,
        gate_entity_id: r.get(2)?,
        branch_id: r.get(3)?,
        lineage_version_id: r.get(4)?,
        transform_version_id: r.get(5)?,
        spill_version_id: r.get(6)?,
        parent_gate_version_id: r.get(7)?,
        denominator_result_id: r.get(8)?,
        count: r.get(9)?,
        denominator: r.get(10)?,
        percent: r.get(11)?,
        included_indices: parse_indices(&inc),
        boundary_included: parse_indices(&bi),
        boundary_excluded: parse_indices(&be),
        stale: r.get::<_, i64>(16)? != 0,
        stale_reason: r.get(17)?,
        blocked: r.get::<_, i64>(19)? != 0,
        blocked_reason: r.get(20)?,
        created_at: r.get(21)?,
    })
}

const RESULT_COLS: &str = "id,gate_version_id,gate_entity_id,branch_id,lineage_version_id,\
transform_version_id,spill_version_id,parent_gate_version_id,denominator_result_id,\
count,denominator,percent,included_indices,boundary_included,boundary_excluded,\
stale,stale_reason,blocked,blocked_reason,created_at";

#[derive(Debug, Clone)]
pub struct NewGateResult {
    pub gate_version_id: String,
    pub gate_entity_id: String,
    pub lineage_version_id: String,
    pub transform_version_id: String,
    pub spill_version_id: String,
    pub parent_gate_version_id: Option<String>,
    pub denominator_result_id: Option<String>,
    pub count: i64,
    pub denominator: i64,
    pub percent: f64,
    pub included_indices: Vec<i64>,
    pub boundary_included: Vec<i64>,
    pub boundary_excluded: Vec<i64>,
    pub blocked: bool,
    pub blocked_reason: String,
}

impl Repo {
    /// 在事务中插入一条新结果（绝不 UPDATE 旧结果）。
    pub fn insert_gate_result_tx(
        tx: &rusqlite::Transaction,
        branch_id: &str,
        n: NewGateResult,
    ) -> StoreResult<GateResultRow> {
        let ts = now_ms();
        let seq: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM gate_results WHERE branch_id=?1",
                params![branch_id],
                |r| r.get(0),
            )
            .unwrap_or(0);
        let id = result_id(branch_id, &n.gate_version_id, &n.lineage_version_id, seq + ts % 100_000);
        tx.execute(
            "INSERT INTO gate_results
             (id,gate_version_id,gate_entity_id,branch_id,lineage_version_id,transform_version_id,
              spill_version_id,parent_gate_version_id,denominator_result_id,count,denominator,
              percent,included_indices,boundary_included,boundary_excluded,stale,stale_reason,
              blocked,blocked_reason,created_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,0,'',?16,?17,?18)",
            params![
                id,
                n.gate_version_id,
                n.gate_entity_id,
                branch_id,
                n.lineage_version_id,
                n.transform_version_id,
                n.spill_version_id,
                n.parent_gate_version_id,
                n.denominator_result_id,
                n.count,
                n.denominator,
                n.percent,
                join_indices(&n.included_indices),
                join_indices(&n.boundary_included),
                join_indices(&n.boundary_excluded),
                n.blocked as i64,
                n.blocked_reason,
                ts
            ],
        )?;
        Ok(GateResultRow {
            id,
            gate_version_id: n.gate_version_id,
            gate_entity_id: n.gate_entity_id,
            branch_id: branch_id.into(),
            lineage_version_id: n.lineage_version_id,
            transform_version_id: n.transform_version_id,
            spill_version_id: n.spill_version_id,
            parent_gate_version_id: n.parent_gate_version_id,
            denominator_result_id: n.denominator_result_id,
            count: n.count,
            denominator: n.denominator,
            percent: n.percent,
            included_indices: n.included_indices,
            boundary_included: n.boundary_included,
            boundary_excluded: n.boundary_excluded,
            stale: false,
            stale_reason: String::new(),
            blocked: n.blocked,
            blocked_reason: n.blocked_reason,
            created_at: ts,
        })
    }

    /// 只把结果标记为过期（触发器保护计数/分母/百分比不被覆盖）。
    pub fn mark_stale_tx(
        tx: &rusqlite::Transaction,
        ids: &[String],
        reason: &str,
    ) -> StoreResult<usize> {
        let mut updated = 0;
        for id in ids {
            updated += tx.execute(
                "UPDATE gate_results SET stale=1, stale_reason=?1 WHERE id=?2 AND stale=0",
                params![reason, id],
            )?;
        }
        Ok(updated)
    }

    /// 某门最新（即使过期）结果。
    pub fn latest_result_for_gate(
        &self,
        branch_id: &str,
        gate_entity_id: &str,
    ) -> StoreResult<Option<GateResultRow>> {
        self.conn
            .query_row(
                &format!(
                    "SELECT {RESULT_COLS} FROM gate_results
                     WHERE branch_id=?1 AND gate_entity_id=?2
                     ORDER BY created_at DESC, rowid DESC LIMIT 1"
                ),
                params![branch_id, gate_entity_id],
                row_to_result,
            )
            .optional()
    }

    pub fn result_by_id(&self, id: &str) -> StoreResult<GateResultRow> {
        self.conn
            .query_row(
                &format!("SELECT {RESULT_COLS} FROM gate_results WHERE id=?1"),
                params![id],
                row_to_result,
            )
            .optional()?
            .ok_or_else(|| err("RESULT_NOT_FOUND", id))
    }

    pub fn list_results(&self, branch_id: &str) -> StoreResult<Vec<GateResultRow>> {
        let mut s = self.conn.prepare(&format!(
            "SELECT {RESULT_COLS} FROM gate_results WHERE branch_id=?1 ORDER BY created_at, rowid"
        ))?;
        let rows = s.query_map(params![branch_id], row_to_result)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn upsert_verdict_tx(
        tx: &rusqlite::Transaction,
        branch_id: &str,
        v: &CompensationVerdict,
        event_id: &str,
    ) -> StoreResult<()> {
        tx.execute(
            "INSERT INTO compensation_verdicts
             (id,spill_version_id,branch_id,status,det,cond,candidates,chosen_index,event_id)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)
             ON CONFLICT(branch_id,spill_version_id) DO UPDATE SET
               status=excluded.status,det=excluded.det,cond=excluded.cond,
               candidates=excluded.candidates,chosen_index=excluded.chosen_index,event_id=excluded.event_id",
            params![
                format!("cvd_{}", &v.spill_version_id[4..]),
                v.spill_version_id,
                branch_id,
                v.status,
                v.det,
                v.cond,
                serde_json::to_string(&v.candidates).unwrap(),
                v.chosen_index,
                event_id
            ],
        )?;
        Ok(())
    }

    pub fn list_verdicts(&self, branch_id: &str) -> StoreResult<Vec<serde_json::Value>> {
        let mut s = self.conn.prepare(
            "SELECT spill_version_id,status,det,cond,candidates,chosen_index,event_id
             FROM compensation_verdicts WHERE branch_id=?1 ORDER BY rowid",
        )?;
        let rows = s.query_map(params![branch_id], |r| {
            let cands: String = r.get(4)?;
            Ok(serde_json::json!({
                "spill_version_id": r.get::<_,String>(0)?,
                "status": r.get::<_,String>(1)?,
                "det": r.get::<_,f64>(2)?,
                "cond": r.get::<_,Option<f64>>(3)?,
                "candidates": serde_json::from_str::<serde_json::Value>(&cands).unwrap(),
                "chosen_index": r.get::<_,Option<i64>>(5)?,
                "event_id": r.get::<_,String>(6)?,
            }))
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// 写入一次人工裁定（active）。不覆盖原始数据，只新增决定。
    pub fn add_decision_tx(
        tx: &rusqlite::Transaction,
        branch_id: &str,
        target_version_id: &str,
        decision: &str,
        chosen_index: Option<i64>,
        event_id: &str,
    ) -> StoreResult<String> {
        let ts = now_ms();
        let id = decision_id(branch_id, target_version_id, ts);
        tx.execute(
            "INSERT INTO decisions(id,branch_id,target_version_id,decision,chosen_index,event_id,active,created_at)
             VALUES(?1,?2,?3,?4,?5,?6,1,?7)",
            params![id, branch_id, target_version_id, decision, chosen_index, event_id, ts],
        )?;
        Ok(id)
    }

    /// 撤销：把最近一次该目标的 active 决定置为非 active（不删除）。
    pub fn deactivate_latest_decision_tx(
        tx: &rusqlite::Transaction,
        branch_id: &str,
        target_version_id: &str,
    ) -> StoreResult<Option<String>> {
        let found: Option<String> = tx
            .query_row(
                "SELECT id FROM decisions WHERE branch_id=?1 AND target_version_id=?2 AND active=1
                 ORDER BY created_at DESC, rowid DESC LIMIT 1",
                params![branch_id, target_version_id],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(id) = &found {
            tx.execute("UPDATE decisions SET active=0 WHERE id=?1", params![id])?;
        }
        Ok(found)
    }

    pub fn active_decision(
        &self,
        branch_id: &str,
        target_version_id: &str,
    ) -> StoreResult<Option<(String, String, Option<i64>)>> {
        self.conn
            .query_row(
                "SELECT id,decision,chosen_index FROM decisions
                 WHERE branch_id=?1 AND target_version_id=?2 AND active=1
                 ORDER BY created_at DESC, rowid DESC LIMIT 1",
                params![branch_id, target_version_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
    }

    pub fn list_decisions(&self, branch_id: &str) -> StoreResult<Vec<serde_json::Value>> {
        let mut s = self.conn.prepare(
            "SELECT id,target_version_id,decision,chosen_index,event_id,active,created_at
             FROM decisions WHERE branch_id=?1 ORDER BY created_at, rowid",
        )?;
        let rows = s.query_map(params![branch_id], |r| {
            Ok(serde_json::json!({
                "id": r.get::<_,String>(0)?,
                "target_version_id": r.get::<_,String>(1)?,
                "decision": r.get::<_,String>(2)?,
                "chosen_index": r.get::<_,Option<i64>>(3)?,
                "event_id": r.get::<_,String>(4)?,
                "active": r.get::<_,i64>(5)? != 0,
                "created_at": r.get::<_,i64>(6)?,
            }))
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }
}

use rusqlite::OptionalExtension;

/// 让编译器确认事件载荷被使用。
#[allow(dead_code)]
fn _payload_use(_p: &EventPayload) {}
