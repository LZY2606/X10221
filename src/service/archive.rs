//! 导出/导入归档：身份、顺序、来源关系跨删除数据库后保持。
//!
//! 归档为 JSON：所有行按固定表顺序、固定顺序导出；id 为内容/链派生，保持稳定。

use crate::store::Repo;
use base64::Engine;
use serde_json::{json, Value};
use rusqlite::OptionalExtension;

pub fn export_all(repo: &Repo) -> crate::store::StoreResult<Value> {
    let c = &repo.conn;
    let table = |sql: &str, params: &[&dyn rusqlite::ToSql]| -> rusqlite::Result<Vec<Vec<Value>>> {
        let mut s = c.prepare(sql)?;
        let ncol = s.column_count();
        let rows = s.query_map(params, |r| {
            let mut out = Vec::with_capacity(ncol);
            for i in 0..ncol {
                let v = cell_value(r, i)?;
                out.push(v);
            }
            Ok(out)
        })?;
        rows.collect()
    };
    let branches = table("SELECT id,name,head_event_id,created_at FROM branches ORDER BY created_at,id", &[])?;
    let materials = {
        let mut s = c.prepare("SELECT id,filename,kind,sha256,size,content,imported_at,label FROM materials ORDER BY imported_at,id")?;
        let ncol = s.column_count();
        let rows = s.query_map([], |r| {
            let mut out = Vec::with_capacity(ncol);
            for i in 0..5 { out.push(cell_value(r, i)?); }
            let bytes: Vec<u8> = r.get(5)?;
            out.push(Value::String(base64::engine::general_purpose::STANDARD.encode(&bytes)));
            for i in 6..ncol { out.push(cell_value(r, i)?); }
            Ok(out)
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    let events = table("SELECT id,seq,branch_id,timestamp,actor,payload,prev_event_id FROM events ORDER BY branch_id,seq", &[])?;
    let versions = table("SELECT id,kind,entity_id,version,data,source_material_id,branch_id,parent_version_id,created_at,note FROM entity_versions ORDER BY branch_id,kind,entity_id,version", &[])?;
    let verdicts = table("SELECT id,spill_version_id,branch_id,status,det,cond,candidates,chosen_index,event_id FROM compensation_verdicts ORDER BY branch_id,rowid", &[])?;
    let results = table("SELECT id,gate_version_id,gate_entity_id,branch_id,lineage_version_id,transform_version_id,spill_version_id,parent_gate_version_id,denominator_result_id,count,denominator,percent,included_indices,boundary_included,boundary_excluded,stale,stale_reason,blocked,blocked_reason,created_at FROM gate_results ORDER BY branch_id,created_at,rowid", &[])?;
    let decisions = table("SELECT id,branch_id,target_version_id,decision,chosen_index,event_id,active,created_at FROM decisions ORDER BY branch_id,created_at,rowid", &[])?;
    let config = table("SELECT branch_id,key,value FROM branch_config ORDER BY branch_id,key", &[])?;
    let diags = table("SELECT material_id,seq,code,detail FROM material_diagnostics ORDER BY material_id,seq", &[])?;

    Ok(json!({
        "format": "flowline.archive/1",
        "schema_version": crate::store::db::CURRENT_SCHEMA,
        "exported_at": crate::store::ids::now_ms(),
        "tables": {
            "branches": branches,
            "materials": materials,
            "events": events,
            "entity_versions": versions,
            "compensation_verdicts": verdicts,
            "gate_results": results,
            "decisions": decisions,
            "branch_config": config,
            "material_diagnostics": diags,
        }
    }))
}

fn cell_value(r: &rusqlite::Row, i: usize) -> rusqlite::Result<Value> {
    use rusqlite::types::ValueRef;
    Ok(match r.get_ref(i)? {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(v) => json!(v),
        ValueRef::Real(v) => json!(v),
        ValueRef::Text(t) => Value::String(String::from_utf8_lossy(t).to_string()),
        ValueRef::Blob(b) => Value::String(base64::engine::general_purpose::STANDARD.encode(b)),
    })
}

/// 从归档导入到一个空库（要求无分支，避免覆盖）。严格按顺序插入以保留身份与链。
pub fn import_archive(repo: &mut Repo, archive: &Value) -> crate::store::StoreResult<()> {
    if archive.get("format").and_then(|v| v.as_str()) != Some("flowline.archive/1") {
        return Err(crate::store::err("BAD_ARCHIVE", "归档格式不是 flowline.archive/1"));
    }
    let count: i64 = repo.conn.query_row("SELECT COUNT(*) FROM branches", [], |r| r.get(0))?;
    if count != 0 {
        return Err(crate::store::err("NONEMPTY", "导入目标库必须为空"));
    }
    let tables = archive.get("tables").ok_or_else(|| crate::store::err("BAD_ARCHIVE", "缺少 tables"))?;
    let tx = repo.conn.transaction()?;

    for row in rows(tables, "branches")? {
        tx.execute("INSERT INTO branches(id,name,head_event_id,created_at) VALUES(?1,?2,?3,?4)",
            rusqlite::params![s(&row,0), s(&row,1), opt_s(&row,2), i(&row,3)])?;
    }
    for row in rows(tables, "materials")? {
        let b64 = s(&row, 5);
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64.as_bytes()).map_err(|e| crate::store::err("BAD_BASE64", e.to_string()))?;
        tx.execute("INSERT INTO materials(id,filename,kind,sha256,size,content,imported_at,label)
                    VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            rusqlite::params![s(&row,0), s(&row,1), s(&row,2), s(&row,3), i(&row,4), bytes, i(&row,6), s(&row,7)])?;
    }
    for row in rows(tables, "material_diagnostics")? {
        tx.execute("INSERT INTO material_diagnostics(material_id,seq,code,detail) VALUES(?1,?2,?3,?4)",
            rusqlite::params![s(&row,0), i(&row,1), s(&row,2), s(&row,3)])?;
    }
    for row in rows(tables, "events")? {
        tx.execute("INSERT INTO events(id,seq,branch_id,timestamp,actor,payload,prev_event_id)
                    VALUES(?1,?2,?3,?4,?5,?6,?7)",
            rusqlite::params![s(&row,0), i(&row,1), s(&row,2), i(&row,3), s(&row,4), s(&row,5), opt_s(&row,6)])?;
    }
    for row in rows(tables, "entity_versions")? {
        tx.execute("INSERT INTO entity_versions(id,kind,entity_id,version,data,source_material_id,branch_id,parent_version_id,created_at,note)
                    VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            rusqlite::params![s(&row,0),s(&row,1),s(&row,2),i(&row,3),s(&row,4),opt_s(&row,5),s(&row,6),opt_s(&row,7),i(&row,8),s(&row,9)])?;
    }
    for row in rows(tables, "compensation_verdicts")? {
        tx.execute("INSERT INTO compensation_verdicts(id,spill_version_id,branch_id,status,det,cond,candidates,chosen_index,event_id)
                    VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            rusqlite::params![s(&row,0),s(&row,1),s(&row,2),s(&row,3),f(&row,4),opt_f(&row,5),s(&row,6),opt_i(&row,7),s(&row,8)])?;
    }
    for row in rows(tables, "gate_results")? {
        tx.execute("INSERT INTO gate_results(id,gate_version_id,gate_entity_id,branch_id,lineage_version_id,transform_version_id,
                    spill_version_id,parent_gate_version_id,denominator_result_id,count,denominator,percent,
                    included_indices,boundary_included,boundary_excluded,stale,stale_reason,blocked,blocked_reason,created_at)
                    VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20)",
            rusqlite::params![
                s(&row,0),s(&row,1),s(&row,2),s(&row,3),s(&row,4),s(&row,5),s(&row,6),
                opt_s(&row,7),opt_s(&row,8),i(&row,9),i(&row,10),f(&row,11),s(&row,12),s(&row,13),s(&row,14),
                i(&row,15),s(&row,16),i(&row,17),s(&row,18),i(&row,19)])?;
    }
    for row in rows(tables, "decisions")? {
        tx.execute("INSERT INTO decisions(id,branch_id,target_version_id,decision,chosen_index,event_id,active,created_at)
                    VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            rusqlite::params![s(&row,0),s(&row,1),s(&row,2),s(&row,3),opt_i(&row,4),s(&row,5),i(&row,6),i(&row,7)])?;
    }
    for row in rows(tables, "branch_config")? {
        tx.execute("INSERT INTO branch_config(branch_id,key,value) VALUES(?1,?2,?3)",
            rusqlite::params![s(&row,0),s(&row,1),s(&row,2)])?;
    }
    tx.commit()?;
    Ok(())
}

fn rows<'a>(t: &'a Value, name: &str) -> crate::store::StoreResult<&'a Vec<Value>> {
    t.get(name).and_then(|v| v.as_array()).ok_or_else(|| crate::store::err("BAD_ARCHIVE", format!("缺少表 {name}")))
}
fn s(r: &[Value], i: usize) -> String { r[i].as_str().unwrap_or("").to_string() }
fn opt_s(r: &[Value], i: usize) -> Option<String> { r[i].as_str().map(|x| x.to_string()) }
fn i(r: &[Value], i: usize) -> i64 { r[i].as_i64().unwrap_or(0) }
fn opt_i(r: &[Value], i: usize) -> Option<i64> { r[i].as_i64() }
fn f(r: &[Value], i: usize) -> f64 { r[i].as_f64().unwrap_or(0.0) }
fn opt_f(r: &[Value], i: usize) -> Option<f64> { r[i].as_f64() }

#[allow(dead_code)]
fn _opt() { let _ = Option::<String>::None; let _: Option<i64> = rusqlite::Connection::open_in_memory().ok().and(None); let _ = OptionalExtension::optional; }
