//! 整库导出/导入：事件、分支、构件与结果行按确定顺序搬运，
//! 身份（event_id/artifact_id/result_id）、顺序与来源关系保持不变。

use crate::engine::Store;
use crate::Result;
use rusqlite::{params, Connection};
use serde_json::{json, Value};

const TABLES: [&str; 7] = [
    "studies", "raw_blobs", "events", "branches", "artifacts",
    "comp_evaluations", "comp_decisions",
];

pub fn export_all(conn: &Connection) -> Result<Value> {
    let mut dump = json!({"format": "flow-lineage-archive/1", "tables": {}});
    for table in TABLES {
        let rows = dump_table(conn, table)?;
        dump["tables"][table] = json!(rows);
    }
    let pops = dump_table(conn, "pop_results")?;
    dump["tables"]["pop_results"] = json!(pops);
    Ok(dump)
}

fn dump_table(conn: &Connection, table: &str) -> Result<Vec<Value>> {
    let sql = match table {
        "events" => "SELECT event_id,seq,branch_id,parent_event_id,kind,payload_json,basis_revision,created_at,COALESCE(source_fragment_id,'') FROM events ORDER BY branch_id,seq",
        "artifacts" => "SELECT artifact_id,version,kind,logical_id,content_json,sha256,introduced_event_id,COALESCE(superseded_event_id,'') FROM artifacts ORDER BY artifact_id,version",
        "branches" => "SELECT branch_id,study_id,name,COALESCE(fork_event_id,''),COALESCE(head_event_id,''),revision,created_at FROM branches ORDER BY branch_id",
        "studies" => "SELECT study_id,title,created_at,schema_version FROM studies",
        "raw_blobs" => "SELECT blob_id,rel_path,sha256,hex(bytes) FROM raw_blobs ORDER BY blob_id",
        "comp_evaluations" => "SELECT eval_id,matrix_artifact,matrix_version,status,rcond,analysis_json,event_id,undone FROM comp_evaluations ORDER BY eval_id",
        "comp_decisions" => "SELECT branch_id,eval_id,COALESCE(strategy,''),COALESCE(decided_event_id,''),COALESCE(undone_event_id,'') FROM comp_decisions ORDER BY branch_id,eval_id",
        "pop_results" => "SELECT result_id,gate_logical,branch_id,event_id,count,percent,COALESCE(denominator_result_id,''),COALESCE(denominator_event_id,''),deps_json,status,stale,COALESCE(superseded_by_event,'') FROM pop_results ORDER BY branch_id,rowid",
        _ => return Err(format!("未知表 {table}")),
    };
    let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
    let cols = stmt.column_count();
    let rows = stmt
        .query_map([], |r| {
            let mut arr = Vec::with_capacity(cols);
            for i in 0..cols {
                let v: Value = match r.get_ref(i).unwrap() {
                    rusqlite::types::ValueRef::Null => Value::Null,
                    rusqlite::types::ValueRef::Integer(x) => json!(x),
                    rusqlite::types::ValueRef::Real(x) => json!(x),
                    rusqlite::types::ValueRef::Text(t) => json!(String::from_utf8_lossy(t).to_string()),
                    rusqlite::types::ValueRef::Blob(b) => json!(String::from_utf8_lossy(b).to_string()),
                };
                arr.push(v);
            }
            Ok(json!(arr))
        })
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

/// 导入整个归档到一个全新数据库；任何一行失败整体回滚。
pub fn import_all(store: &mut Store, dump: &Value) -> Result<()> {
    store.conn.execute_batch("PRAGMA foreign_keys=OFF").ok();
    {
        let tx = store.conn.transaction().map_err(|e| e.to_string())?;
        load(&tx, "studies", "INSERT INTO studies VALUES(?,?,?,?)", &dump["tables"]["studies"])?;
        load_blob(&tx, &dump["tables"]["raw_blobs"])?;
        load(&tx, "events", "INSERT INTO events VALUES(?,?,?,?,?,?,?,?,?)", &dump["tables"]["events"])?;
        load(&tx, "branches", "INSERT INTO branches VALUES(?,?,?,?,?,?,?)", &dump["tables"]["branches"])?;
        load(&tx, "artifacts", "INSERT INTO artifacts VALUES(?,?,?,?,?,?,?,?)", &dump["tables"]["artifacts"])?;
        load(&tx, "comp_evaluations", "INSERT INTO comp_evaluations VALUES(?,?,?,?,?,?,?,?)", &dump["tables"]["comp_evaluations"])?;
        load(&tx, "comp_decisions", "INSERT INTO comp_decisions VALUES(?,?,?,?,?)", &dump["tables"]["comp_decisions"])?;
        load(&tx, "pop_results", "INSERT INTO pop_results(result_id,gate_logical,branch_id,event_id,count,percent,denominator_result_id,denominator_event_id,deps_json,status,stale,superseded_by_event) VALUES(?,?,?,?,?,?,?,?,?,?,?,?)", &dump["tables"]["pop_results"])?;
        tx.commit().map_err(|e| e.to_string())?;
    }
    store.conn.execute_batch("PRAGMA foreign_keys=ON").ok();
    Ok(())
}

fn load(tx: &rusqlite::Transaction, _label: &str, sql: &str, rows: &Value) -> Result<()> {
    let arr = rows.as_array().ok_or("归档表必须是数组")?;
    for row in arr {
        let vals: &Vec<Value> = row.as_array().ok_or("归档行必须是数组")?;
        let params: Vec<Box<dyn rusqlite::types::ToSql>> = vals
            .iter()
            .map(|v| -> Box<dyn rusqlite::types::ToSql> {
                match v {
                    Value::Null => Box::new(Option::<String>::None),
                    Value::Bool(b) => Box::new(*b as i64),
                    Value::Number(n) => Box::new(n.as_f64().unwrap_or(0.0)),
                    Value::String(s) => Box::new(s.clone()),
                    _ => Box::new(v.to_string()),
                }
            })
            .collect();
        let refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();
        tx.execute(sql, refs.as_slice()).map_err(|e| format!("导入失败({_label}): {e}"))?;
    }
    Ok(())
}

fn load_blob(tx: &rusqlite::Transaction, rows: &Value) -> Result<()> {
    for row in rows.as_array().unwrap() {
        let a = row.as_array().unwrap();
        let hex = a[3].as_str().unwrap_or("");
        let bytes = decode_hex(hex).ok_or("raw blob 不是十六进制")?;
        tx.execute(
            "INSERT INTO raw_blobs VALUES(?,?,?,?)",
            params![a[0].as_str().unwrap(), a[1].as_str().unwrap(), a[2].as_str().unwrap(), bytes],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn decode_hex(s: &str) -> Option<Vec<u8>> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok()).collect()
}
