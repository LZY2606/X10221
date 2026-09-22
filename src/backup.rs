//! 导出/导入：身份（event_id/row_uid/candidate_uid/材料版本）、顺序与来源关系保持。
//! 删除本地数据库再导入后仍一致。导入在单事务内完成，失败回滚不留半份记录。

use crate::db::Store;
use rusqlite::params;
use serde_json::{json, Value};

impl Store {
    pub fn export_all(&self) -> Value {
        let c = self.conn.lock().unwrap();
        json!({
            "format": "flowline-archive/v1",
            "materials": dump(&c, "SELECT material_type,material_id,version,payload_json,sha256,ingest_seq FROM materials ORDER BY ingest_seq"),
            "branches": dump(&c, "SELECT branch_id,parent_branch,base_event_id,tip_event_id,label,created_seq FROM branches ORDER BY created_seq"),
            "events": dump(&c, "SELECT event_id,seq,branch_id,parent_event_id,kind,payload_json,refs_json,result_json FROM events ORDER BY branch_id,seq"),
            "candidates": dump(&c, "SELECT candidate_uid,branch_id,group_id,candidate_id,kind,status,detail_json,event_id FROM candidates ORDER BY rowid"),
            "appraisal_rows": dump(&c, "SELECT row_uid,branch_id,event_id,gate_id,gate_ref,parent_ref,denominator_ref,numerator,denominator,fraction,status,detail_json,stale_reason FROM appraisal_rows ORDER BY branch_id,event_id,gate_id"),
            "conflict_log": dump(&c, "SELECT conflict_uid,branch_id,attempted_kind,expected_parent,actual_parent,diff_json,created_seq FROM conflict_log ORDER BY created_seq"),
        })
    }

    pub fn import_all(&self, archive: &Value, wipe: bool) -> Result<usize, String> {
        if archive.get("format").and_then(|v| v.as_str()) != Some("flowline-archive/v1") {
            return Err("unsupported archive format".into());
        }
        let mut c = self.conn.lock().unwrap();
        let tx = c.transaction().map_err(|e| e.to_string())?;
        if wipe {
            for t in ["appraisal_rows", "candidates", "events", "branches", "materials", "conflict_log", "seq_counters"] {
                tx.execute(&format!("DELETE FROM {t}"), []).map_err(|e| e.to_string())?;
            }
        }
        let mut count = 0;
        for row in arr(&archive["materials"]) {
            tx.execute(
                "INSERT OR IGNORE INTO materials(material_type,material_id,version,payload_json,sha256,ingest_seq)
                 VALUES(?1,?2,?3,?4,?5,?6)",
                params![s(&row[0]), s(&row[1]), i(&row[2]), s(&row[3]), s(&row[4]), i(&row[5])],
            ).map_err(|e| e.to_string())?;
            count += 1;
        }
        for row in arr(&archive["branches"]) {
            tx.execute(
                "INSERT OR IGNORE INTO branches(branch_id,parent_branch,base_event_id,tip_event_id,label,created_seq)
                 VALUES(?1,?2,?3,?4,?5,?6)",
                params![s(&row[0]), os(&row[1]), os(&row[2]), os(&row[3]), s(&row[4]), i(&row[5])],
            ).map_err(|e| e.to_string())?;
            count += 1;
        }
        for row in arr(&archive["events"]) {
            tx.execute(
                "INSERT OR IGNORE INTO events(event_id,seq,branch_id,parent_event_id,kind,payload_json,refs_json,result_json)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
                params![s(&row[0]), i(&row[1]), s(&row[2]), os(&row[3]), s(&row[4]), s(&row[5]), s(&row[6]), s(&row[7])],
            ).map_err(|e| e.to_string())?;
            count += 1;
        }
        for row in arr(&archive["candidates"]) {
            tx.execute(
                "INSERT OR IGNORE INTO candidates(candidate_uid,branch_id,group_id,candidate_id,kind,status,detail_json,event_id)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
                params![s(&row[0]), s(&row[1]), s(&row[2]), s(&row[3]), s(&row[4]), s(&row[5]), s(&row[6]), s(&row[7])],
            ).map_err(|e| e.to_string())?;
            count += 1;
        }
        for row in arr(&archive["appraisal_rows"]) {
            tx.execute(
                "INSERT OR IGNORE INTO appraisal_rows
                 (row_uid,branch_id,event_id,gate_id,gate_ref,parent_ref,denominator_ref,numerator,denominator,fraction,status,detail_json,stale_reason)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
                params![s(&row[0]), s(&row[1]), s(&row[2]), s(&row[3]), s(&row[4]), os(&row[5]), s(&row[6]),
                    i(&row[7]), i(&row[8]), f(&row[9]), s(&row[10]), s(&row[11]), os(&row[12])],
            ).map_err(|e| e.to_string())?;
            count += 1;
        }
        for row in arr(&archive["conflict_log"]) {
            tx.execute(
                "INSERT OR IGNORE INTO conflict_log(conflict_uid,branch_id,attempted_kind,expected_parent,actual_parent,diff_json,created_seq)
                 VALUES(?1,?2,?3,?4,?5,?6,?7)",
                params![s(&row[0]), s(&row[1]), s(&row[2]), os(&row[3]), os(&row[4]), s(&row[5]), i(&row[6])],
            ).map_err(|e| e.to_string())?;
            count += 1;
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(count)
    }

    pub fn conflicts(&self) -> Vec<Value> {
        let c = self.conn.lock().unwrap();
        let mut stmt = c.prepare(
            "SELECT conflict_uid,branch_id,attempted_kind,expected_parent,actual_parent,diff_json,created_seq
             FROM conflict_log ORDER BY created_seq").unwrap();
        stmt.query_map([], |r| {
            let diff: String = r.get(5)?;
            Ok(json!({
                "conflict_uid": r.get::<_, String>(0)?,
                "branch_id": r.get::<_, String>(1)?,
                "attempted_kind": r.get::<_, String>(2)?,
                "expected_parent": r.get::<_, Option<String>>(3)?,
                "actual_parent": r.get::<_, Option<String>>(4)?,
                "diff": serde_json::from_str::<Value>(&diff).unwrap_or(Value::Null),
                "created_seq": r.get::<_, i64>(6)?,
            }))
        }).unwrap().map(|r| r.unwrap()).collect()
    }
}

fn arr(v: &Value) -> Vec<Value> {
    v.as_array().cloned().unwrap_or_default()
}
fn s(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_string()
}
fn os(v: &Value) -> Option<String> {
    v.as_str().map(String::from)
}
fn i(v: &Value) -> i64 {
    v.as_i64().or_else(|| v.as_f64().map(|x| x as i64)).unwrap_or(0)
}
fn f(v: &Value) -> f64 {
    v.as_f64().unwrap_or(0.0)
}

fn dump(conn: &rusqlite::Connection, sql: &str) -> Vec<Value> {
    let mut stmt = conn.prepare(sql).unwrap();
    let n = stmt.column_count();
    let cols: Vec<String> = (0..n)
        .map(|i| stmt.column_name(i).unwrap().to_string())
        .collect();
    let rows = stmt
        .query_map([], |r| {
            let mut obj = serde_json::Map::new();
            for (idx, name) in cols.iter().enumerate() {
                let v = match r.get_ref(idx).unwrap() {
                    rusqlite::types::ValueRef::Null => Value::Null,
                    rusqlite::types::ValueRef::Integer(x) => json!(x),
                    rusqlite::types::ValueRef::Real(x) => json!(x),
                    rusqlite::types::ValueRef::Text(t) => {
                        let st = std::str::from_utf8(t).unwrap();
                        serde_json::from_str(st).unwrap_or_else(|_| json!(st))
                    }
                    rusqlite::types::ValueRef::Blob(_) => Value::Null,
                };
                obj.insert(name.clone(), v);
            }
            Ok(Value::Object(obj))
        })
        .unwrap();
    rows.map(|r| r.unwrap()).collect()
}
